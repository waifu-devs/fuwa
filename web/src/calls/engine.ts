import { Code } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { createClient } from "@connectrpc/connect";
import { CallService, type GetCallSettingsResponse } from "@/gen/fuwa/v1/call_pb";
import { dmEngine } from "@/e2ee/engine";
import { toFuwaError, type FuwaError } from "@/fuwa/errors";
import { engine } from "@/fuwa/sync";
import { store } from "@/fuwa/store";
import { getPrefs, setPrefs, subscribePrefs } from "@/lib/prefs";
import { cue } from "@/lib/sounds";
import { reportTiming, reportUsage } from "@/lib/reports";
import { toast } from "@/lib/ui";
import { Mic, micProblem, Speakers } from "./audio";
import { canEncryptCalls, encryptedConfig, FrameCrypto } from "./frames";
import { watchQuality } from "./quality";
import { getCalls, sameTarget, setCalls, type CallTarget } from "./state";
import {
  cameraProblem,
  clearRemoteVideos,
  ENCODINGS,
  getVideos,
  onWantsChange,
  openCamera,
  isScreen,
  openScreen,
  ownerOf,
  SCREEN_ENCODINGS,
  screenProblem,
  setLocalScreen,
  setLocalVideo,
  setRemoteVideo,
  wanted,
} from "./video";

/**
 * This browser's call: one RTCPeerConnection to the instance's media
 * server, sending your microphone (and camera, while it's on) and getting
 * each other person's as tracks of their own (each track's stream id is
 * their user id, so each voice and camera is theirs).
 *
 * Joining goes through the instance (JoinVoice or JoinDmCall), which keeps
 * your place in the call and hands the media server your offer. After that
 * the media server talks to this browser directly on the "fuwa" data
 * channel: it offers again whenever someone's sound comes or goes, says
 * "restarting" before a deploy stops it, "replaced" when you joined from
 * somewhere else, and "closed" when you were taken out of the call.
 *
 * The place lasts while this browser keeps it (every 5 seconds); if the
 * connection drops, or the media server restarts, it joins again with the
 * same session, so nobody sees you leave.
 */

const KEEP_MS = 5_000;
/** How long a broken connection gets to come back by itself before joining again. */
const GRACE_MS = 2_500;
const CHANNEL = "fuwa";
const FULL = "This server's recordings are full. Delete some in Recordings to record again.";

type Signal =
  | { type: "offer" | "answer"; sdp: string }
  | { type: "replaced" | "restarting" | "closed" }
  | { type: "layers"; layers: Record<string, string> };

/** Asks a voice place or call to end without waiting for the answer: when the page goes away. */
function leaveOnUnload(url: string, token: string | null, target: CallTarget, sessionId: string) {
  const transport = createGrpcWebTransport({
    baseUrl: url,
    fetch: (input, init) => fetch(input, { ...init, keepalive: true }),
    interceptors: [
      (next) => (req) => {
        if (token) req.header.set("authorization", `Bearer ${token}`);
        return next(req);
      },
    ],
  });
  const calls = createClient(CallService, transport);
  if (target.kind === "voice") void calls.leaveVoice({ serverId: target.serverId, sessionId }).catch(() => {});
  else void calls.leaveDmCall({ conversationId: target.conversationId, sessionId }).catch(() => {});
}

class Session {
  private pc: RTCPeerConnection | null = null;
  private channel: RTCDataChannel | null = null;
  private mic: Mic | null = null;
  private camera: MediaStreamTrack | null = null;
  private video: RTCRtpTransceiver | null = null;
  private screen: MediaStreamTrack | null = null;
  private screenVideo: RTCRtpTransceiver | null = null;
  /** The shared screen's sound, and its place in the connection. */
  private screenAudio: MediaStreamTrack | null = null;
  private screenSound: RTCRtpTransceiver | null = null;
  /** Shared screens whose sound is coming in, by feed. */
  private screenFeeds = new Set<string>();
  private recorder: MediaRecorder | null = null;
  /** The channel doesn't allow recording (no RECORD there). */
  recordSuppressed = false;
  private layersTimer: ReturnType<typeof setTimeout> | null = null;
  private unwant: (() => void) | null = null;
  private unprefs: (() => void) | null = null;
  /** The channel doesn't allow cameras or shared screens (no VIDEO there). */
  videoSuppressed = false;
  /** A moderator turned your camera and shared screen off in this server. */
  videoOff = false;
  private unmoderated: (() => void) | null = null;
  private speakers: Speakers;
  private frames: FrameCrypto | null = null;
  private sessionId = "";
  private settings: GetCallSettingsResponse | null = null;
  private keeper: ReturnType<typeof setInterval> | null = null;
  private secretPoll: ReturnType<typeof setInterval> | null = null;
  private unwatch: (() => void) | null = null;
  private broken: ReturnType<typeof setTimeout> | null = null;
  /** When joining (or joining again) started, for anonymous reports of how long it takes. */
  private waitingSince = performance.now();
  private connecting: Promise<void> | null = null;
  private attempt = 0;
  private stopped = false;
  private epoch = -1;
  private heard = new Set<string>();
  private readonly me: string;

  constructor(readonly target: CallTarget) {
    this.me = store.get().instances[target.instance]?.me?.id ?? "";
    this.speakers = new Speakers(
      (userId) => {
        // A shared screen's sound has its own volume, and can be turned off.
        if (isScreen(userId) && getCalls().quietScreens[ownerOf(userId)]) return 0;
        const p = getPrefs();
        return (p.userVolumes[`${target.instance}/${userId}`] ?? 100) / 100;
      },
      (userId, talking) => setCalls((s) => ({ speaking: { ...s.speaking, [userId]: talking } })),
    );
    this.speakers.deafened = getCalls().selfDeaf;
    this.speakers.apply();
  }

  private get api() {
    return engine(this.target.instance).api;
  }

  async start() {
    const settings = await this.api.calls.getCallSettings({});
    if (!settings.enabled) throw new Error("Calls are switched off on this instance.");
    this.settings = settings;
    setCalls(() => ({ serverRecordings: settings.recordings, screenSoundOffered: settings.screenSound }));
    if (this.target.kind === "dm") {
      if (!canEncryptCalls()) throw new Error("This browser can't encrypt calls. Try a recent Chrome, Edge, Firefox or Safari.");
      const dms = dmEngine(this.target.instance);
      if (!dms) throw new Error("Encrypted messages aren't ready here yet, and calls need them.");
      this.frames = new FrameCrypto((epoch) => void this.refreshSecret(epoch));
      await this.refreshSecret();
      // A device joining or leaving the conversation moves its group to a new epoch, with a new secret.
      this.secretPoll = setInterval(() => void this.refreshSecret(), 4_000);
    }
    try {
      this.mic = await Mic.open(({ open }) => this.setSpeaking(this.me, open));
    } catch (err) {
      // Joining anyway, to listen: you can fix the microphone and unmute.
      setCalls((s) => ({ selfMute: true, call: s.call && { ...s.call, problem: micProblem(err) } }));
    }
    this.applyMute();
    this.unwatch = watchQuality(() => this.pc);
    this.unwant = onWantsChange(() => this.sayLayers());
    // Another camera picked in settings while yours is on: switch to it.
    let device = getPrefs().videoDevice;
    this.unprefs = subscribePrefs(() => {
      if (getPrefs().videoDevice === device) return;
      device = getPrefs().videoDevice;
      if (this.camera) void this.reopenCamera();
    });
    this.unmoderated = store.subscribe(() => this.watchModeration());
    await this.connect();
    this.keeper = setInterval(() => void this.keep(), KEEP_MS);
  }

  private async refreshSecret(epoch?: number) {
    if (this.target.kind !== "dm" || !this.frames) return;
    const dms = dmEngine(this.target.instance);
    if (!dms) return;
    try {
      const got = await dms.callSecret(this.target.conversationId, epoch);
      if (got.epoch !== this.epoch) {
        this.epoch = got.epoch;
        this.frames.setKey(got.epoch, got.secret);
      }
    } catch (err) {
      if (this.epoch < 0) throw err;
    }
  }

  private setSpeaking(userId: string, talking: boolean) {
    if (!userId || !!getCalls().speaking[userId] === talking) return;
    setCalls((s) => ({ speaking: { ...s.speaking, [userId]: talking } }));
  }

  /** Mute and deafen, applied to the microphone, the speakers and the instance. */
  applyMute() {
    const { selfMute, selfDeaf, talking } = getCalls();
    if (this.mic) {
      this.mic.muted = selfMute || selfDeaf;
      this.mic.pushing = talking;
    }
    this.speakers.deafened = selfDeaf;
    this.speakers.apply();
  }

  applyVolumes() {
    this.speakers.apply();
  }

  /** Connects (or connects again) to the media server, with the place this session has. */
  private connect(): Promise<void> {
    this.connecting ??= this.open().finally(() => (this.connecting = null));
    return this.connecting;
  }

  private async open() {
    const attempt = ++this.attempt;
    this.teardown();
    const pc = new RTCPeerConnection({
      iceServers: (this.settings?.iceServers ?? []).map((s) => ({
        urls: s.urls,
        username: s.username || undefined,
        credential: s.credential || undefined,
      })),
      bundlePolicy: "max-bundle",
      rtcpMuxPolicy: "require",
      ...(this.frames ? encryptedConfig() : {}),
    });
    this.pc = pc;
    const track = this.mic?.track ?? silentTrack();
    const transceiver = pc.addTransceiver(track, { direction: "sendonly", streams: [new MediaStream([track])] });
    this.frames?.send(transceiver.sender, this.me);
    preferOpus(transceiver);
    // The camera's place is there from the start, empty until it's on, so
    // turning it on and off never needs a new offer. Nobody gets it until
    // its first frame.
    const video = pc.addTransceiver("video", { direction: "sendonly", sendEncodings: ENCODINGS.map((e) => ({ ...e })) });
    this.video = video;
    this.frames?.send(video.sender, this.me, "video");
    preferVp8(video);
    if (this.camera) void video.sender.replaceTrack(this.camera).catch(() => {});
    // The screen's place comes second, the same way: the media server
    // takes an app's first video track as its camera, the second as its screen.
    const screenVideo = pc.addTransceiver("video", { direction: "sendonly", sendEncodings: SCREEN_ENCODINGS.map((e) => ({ ...e })) });
    this.screenVideo = screenVideo;
    this.frames?.send(screenVideo.sender, this.me, "video");
    preferVp8(screenVideo);
    if (this.screen) void screenVideo.sender.replaceTrack(this.screen).catch(() => {});
    // And its sound, the second audio track (where the instance takes one):
    // empty until a share brings sound, and nobody gets it until then.
    if (this.settings?.screenSound) {
      const screenSound = pc.addTransceiver("audio", { direction: "sendonly", sendEncodings: [{ maxBitrate: 128_000 }] });
      this.screenSound = screenSound;
      this.frames?.send(screenSound.sender, this.me);
      preferOpus(screenSound);
      if (this.screenAudio) void screenSound.sender.replaceTrack(this.screenAudio).catch(() => {});
    }
    const channel = pc.createDataChannel(CHANNEL, { ordered: true });
    this.channel = channel;
    channel.onmessage = (e) => void this.onSignal(pc, e.data);
    channel.onopen = () => this.sayLayers();
    pc.ontrack = (e) => {
      // The stream names the feed: whose it is, and whether it's their screen.
      const userId = e.streams[0]?.id;
      if (!userId) return;
      const stream = e.streams[0]!;
      stream.onremovetrack = (ev) => this.trackGone(userId, stream, ev.track);
      if (e.track.kind === "video") {
        this.frames?.receive(e.receiver, ownerOf(userId), "video");
        setRemoteVideo(userId, { track: e.track, mid: e.transceiver.mid ?? "" });
        this.sayLayers();
        return;
      }
      if (isScreen(userId)) {
        // A shared screen's sound: beside its sharer's voice, never mixed into it.
        this.frames?.receive(e.receiver, ownerOf(userId));
        this.speakers.add(userId, new MediaStream([e.track]));
        this.screenFeeds.add(userId);
        setCalls((s) => ({ screenSounds: { ...s.screenSounds, [ownerOf(userId)]: true } }));
        return;
      }
      this.frames?.receive(e.receiver, userId);
      this.speakers.add(userId, new MediaStream([e.track]));
      if (!this.heard.has(userId) && getCalls().call?.status === "connected") cue("someoneJoined");
      this.heard.add(userId);
    };
    pc.onconnectionstatechange = () => {
      if (pc !== this.pc || this.stopped) return;
      const state = pc.connectionState;
      if (state === "connected") {
        if (this.broken) clearTimeout(this.broken);
        this.broken = null;
        const before = getCalls().call;
        if (before?.status === "connecting" || before?.status === "reconnecting") {
          reportTiming(before.status === "connecting" ? "call.connect" : "call.reconnect", performance.now() - this.waitingSince);
        }
        setCalls((s) => ({ call: s.call && { ...s.call, status: "connected", since: s.call.since ?? Date.now() } }));
        if (before?.status === "connecting") cue("connect");
      } else if (state === "failed") {
        this.reconnect();
      } else if (state === "disconnected") {
        this.broken ??= setTimeout(() => this.reconnect(), GRACE_MS);
      }
    };

    const offer = await pc.createOffer();
    await pc.setLocalDescription(offer);
    await gathered(pc);
    if (attempt !== this.attempt || this.stopped) return;
    const { selfMute, selfDeaf, selfVideo, selfStream, selfRecord, serverRecord } = getCalls();
    const sdp = pc.localDescription?.sdp ?? offer.sdp ?? "";
    const t = this.target;
    const selves = { selfMute, selfDeaf, selfVideo, selfStream, selfRecord, sessionId: this.sessionId };
    const joined =
      t.kind === "voice"
        ? await this.api.calls.joinVoice({ serverId: t.serverId, channelId: t.channelId, offer: sdp, serverRecord, ...selves })
        : await this.api.calls.joinDmCall({ conversationId: t.conversationId, offer: sdp, ...selves });
    if (attempt !== this.attempt || this.stopped) return;
    this.sessionId = joined.sessionId;
    this.videoSuppressed = !!joined.state?.videoSuppress;
    this.videoOff = !!joined.state?.serverVideoOff;
    this.recordSuppressed = !!joined.state?.recordSuppress;
    if ("recordingsFull" in joined && joined.recordingsFull && getCalls().serverRecord) {
      toast(FULL);
      setCalls(() => ({ serverRecord: false }));
    }
    await pc.setRemoteDescription({ type: "answer", sdp: joined.answer });
  }

  /** Someone's sound or camera went away: they left, or turned it off for good. */
  private trackGone(userId: string, stream: MediaStream, track: MediaStreamTrack) {
    if (track.kind === "video") {
      if (getVideos().remote[userId]?.track === track) setRemoteVideo(userId, null);
      return;
    }
    if (isScreen(userId)) {
      this.dropScreenSound(userId);
      return;
    }
    if (stream.getAudioTracks().length) return;
    this.speakers.remove(userId);
    this.heard.delete(userId);
    this.setSpeaking(userId, false);
    if (getCalls().call?.status === "connected") cue("someoneLeft");
  }

  /**
   * Tells the media server which size of each camera this browser shows,
   * a moment after anything changes, so a burst of resizes is one message.
   */
  private sayLayers() {
    if (this.layersTimer) return;
    this.layersTimer = setTimeout(() => {
      this.layersTimer = null;
      const layers: Record<string, string> = {};
      for (const [userId, video] of Object.entries(getVideos().remote)) if (video.mid) layers[video.mid] = wanted(userId);
      if (Object.keys(layers).length) this.say({ type: "layers", layers });
    }, 120);
  }

  private async reopenCamera() {
    try {
      const track = await openCamera();
      if (this.stopped || !this.camera) return track.stop();
      this.camera.stop();
      this.camera = track;
      track.onended = () => {
        if (this.camera === track) void setCamera(false);
      };
      setLocalVideo(track);
      await this.video?.sender.replaceTrack(track).catch(() => {});
    } catch (err) {
      toast(cameraProblem(err));
    }
  }

  /** Turns your camera on or off, without a new offer: its place in the connection stays. */
  async setCamera(on: boolean) {
    if (on && !this.camera) {
      const track = await openCamera();
      if (this.stopped || !getCalls().selfVideo) return track.stop();
      this.camera = track;
      // Unplugged, or taken away in the browser's own controls.
      track.onended = () => {
        if (this.camera === track) void setCamera(false);
      };
    } else if (!on && this.camera) {
      this.camera.stop();
      this.camera = null;
    }
    setLocalVideo(this.camera);
    await this.video?.sender.replaceTrack(this.camera).catch(() => {});
  }

  /**
   * Records the call's sound, everyone's and yours, to a file saved when
   * it stops (or when you hang up). Nothing goes anywhere but this device.
   */
  setRecording(on: boolean) {
    if (on && !this.recorder) {
      const stream = this.speakers.record(this.mic?.track ?? null);
      const type = RECORDING_TYPES.find((t) => MediaRecorder.isTypeSupported(t));
      const recorder = new MediaRecorder(stream, { ...(type ? { mimeType: type } : {}), audioBitsPerSecond: 128_000 });
      const chunks: Blob[] = [];
      const started = new Date();
      recorder.ondataavailable = (e) => {
        if (e.data.size) chunks.push(e.data);
      };
      recorder.onstop = () => saveRecording(new Blob(chunks, { type: recorder.mimeType }), started);
      // A piece every few seconds, so a long call never waits on one huge buffer.
      recorder.start(5_000);
      this.recorder = recorder;
    } else if (!on && this.recorder) {
      if (this.recorder.state !== "inactive") this.recorder.stop();
      this.recorder = null;
      this.speakers.stopRecording();
      toast("Recording saved to your downloads.");
    }
  }

  private dropScreenSound(feed: string) {
    if (!this.screenFeeds.delete(feed)) return;
    this.speakers.remove(feed);
    this.setSpeaking(feed, false);
    setCalls((s) => {
      const { [ownerOf(feed)]: _, ...rest } = s.screenSounds;
      return { screenSounds: rest };
    });
  }

  /**
   * Shares a screen or stops, the same way as the camera. With `sound`, its
   * sound goes too, where the browser and the instance can; where they
   * can't, it says why rather than sharing in silence without a word.
   */
  async setScreen(on: boolean, sound = false) {
    if (on && !this.screen) {
      const offered = !!this.settings?.screenSound;
      const shared = await openScreen(sound && offered);
      if (this.stopped || !getCalls().selfStream) {
        shared.video.stop();
        shared.audio?.stop();
        return;
      }
      this.screen = shared.video;
      this.screenAudio = shared.audio;
      const track = shared.video;
      // Stopped in the browser's own "Stop sharing" bar.
      track.onended = () => {
        if (this.screen === track) void setScreen(false);
      };
      if (sound && !offered) toast("This server passes the picture on, not the sound: it needs a newer fuwa for that.");
      else if (shared.silent) toast(shared.silent);
      if (shared.audio) reportUsage("call.screen_sound");
      else if (shared.silent) reportUsage("call.screen_sound_missing");
    } else if (!on && this.screen) {
      this.screen.stop();
      this.screen = null;
      this.screenAudio?.stop();
      this.screenAudio = null;
    }
    setLocalScreen(this.screen);
    setCalls(() => ({ screenSound: this.screenAudio ? this.screenAudio.enabled : null }));
    await Promise.all([
      this.screenVideo?.sender.replaceTrack(this.screen).catch(() => {}),
      this.screenSound?.sender.replaceTrack(this.screenAudio).catch(() => {}),
    ]);
  }

  /** Turns your shared screen's sound off or back on, without stopping the share. */
  setScreenSound(on: boolean) {
    if (!this.screenAudio) return;
    this.screenAudio.enabled = on;
    setCalls(() => ({ screenSound: on }));
  }

  private async onSignal(pc: RTCPeerConnection, data: unknown) {
    if (pc !== this.pc || typeof data !== "string") return;
    let signal: Signal;
    try {
      signal = JSON.parse(data) as Signal;
    } catch {
      return;
    }
    switch (signal.type) {
      case "offer": {
        // The media server leads: someone's sound came or went.
        await pc.setRemoteDescription({ type: "offer", sdp: signal.sdp });
        const answer = await pc.createAnswer();
        await pc.setLocalDescription(answer);
        this.say({ type: "answer", sdp: pc.localDescription?.sdp ?? answer.sdp ?? "" });
        break;
      }
      case "layers":
        break;
      case "restarting":
        // A deploy: join the media server that takes over, keeping the place.
        this.reconnect(true);
        break;
      case "replaced":
        void hangUp("You joined this call somewhere else.", false);
        break;
      case "closed":
        void hangUp("You were disconnected from the call.");
        break;
    }
  }

  private say(signal: Signal) {
    if (this.channel?.readyState === "open") this.channel.send(JSON.stringify(signal));
  }

  /** Joins again with the same place, after a drop or a restart; keeps trying until it works or the place is gone. */
  private reconnect(soon = false) {
    if (this.stopped || this.connecting) return;
    if (this.broken) clearTimeout(this.broken);
    this.broken = null;
    setCalls((s) => ({ call: s.call && { ...s.call, status: "reconnecting" } }));
    this.waitingSince = performance.now();
    const tryAgain = async (delay: number) => {
      if (this.stopped) return;
      try {
        // Fresh TURN credentials: they're short-lived, and a long call may have outlived them.
        const settings = await this.api.calls.getCallSettings({});
        if (settings.enabled) this.settings = settings;
        await this.connect();
      } catch (err) {
        const e = toFuwaError(err);
        if (gone(e)) return void hangUp("You're no longer in this call.");
        setTimeout(() => void tryAgain(Math.min(delay * 2, 8_000)), delay * (0.75 + Math.random() / 2));
      }
    };
    // After a restart the next media server may need a moment; jitter keeps everyone from arriving at once.
    setTimeout(() => void tryAgain(500), soon ? 150 + Math.random() * 450 : 0);
  }

  /**
   * Notices a moderator turning your camera and shared screen off (or letting
   * you have them again) the moment it happens, not at the next keep.
   */
  private watchModeration() {
    const t = this.target;
    if (t.kind !== "voice" || this.stopped) return;
    const mine = store.get().instances[t.instance]?.voice[t.serverId]?.find((v) => v.userId === this.me);
    if (!mine || mine.channelId !== t.channelId) return;
    this.moderated(mine.serverVideoOff);
  }

  private moderated(off: boolean) {
    if (off === this.videoOff) return;
    this.videoOff = off;
    if (!off) return void toast("A moderator let you turn your camera on again.");
    const { selfVideo, selfStream } = getCalls();
    toast(
      selfVideo && selfStream
        ? "A moderator turned your camera and screen share off."
        : selfStream
          ? "A moderator stopped your screen share."
          : selfVideo
            ? "A moderator turned your camera off."
            : "A moderator turned off cameras and screen sharing for you here.",
    );
    if (selfVideo) void setCamera(false);
    if (selfStream) void setScreen(false);
  }

  /** Keeps the place in the call, and tells the instance how you sound. */
  async keep() {
    if (this.stopped || !this.sessionId) return;
    const { selfMute, selfDeaf, selfVideo, selfStream, selfRecord, serverRecord } = getCalls();
    const selves = { sessionId: this.sessionId, selfMute, selfDeaf, selfVideo, selfStream, selfRecord };
    const t = this.target;
    try {
      if (t.kind === "voice") {
        const kept = await this.api.calls.keepVoice({ serverId: t.serverId, channelId: t.channelId, serverRecord, ...selves });
        this.videoSuppressed = !!kept.state?.videoSuppress;
        this.moderated(!!kept.state?.serverVideoOff);
        // The channel took VIDEO away meanwhile.
        if (this.videoSuppressed && getCalls().selfVideo) {
          toast("You can't have your camera on in this channel any more.");
          void setCamera(false);
        }
        if (this.videoSuppressed && getCalls().selfStream) {
          toast("You can't share your screen in this channel any more.");
          void setScreen(false);
        }
        this.recordSuppressed = !!kept.state?.recordSuppress;
        if (this.recordSuppressed && getCalls().selfRecord) {
          toast("You can't record in this channel any more.");
          setRecording(false);
        }
        // RECORD went away, or the instance stopped recording on the server.
        if (serverRecord && getCalls().serverRecord && !kept.state?.serverRecord) {
          if (this.recordSuppressed) toast("You can't record in this channel any more.");
          else if (kept.recordingsFull) toast(FULL);
          else toast("This instance stopped recording voice channels on the server.");
          setCalls(() => ({ serverRecord: false }));
        }
      } else {
        await this.api.calls.keepDmCall({ conversationId: t.conversationId, ...selves });
      }
    } catch (err) {
      const e = toFuwaError(err);
      if (gone(e)) void hangUp(t.kind === "voice" ? "You were disconnected from the voice channel." : "The call ended.");
      // Anything else (the instance restarting, a blip): the next keep tries again.
    }
  }

  private teardown() {
    for (const id of [...this.heard]) this.speakers.remove(id);
    this.heard.clear();
    for (const feed of [...this.screenFeeds]) this.dropScreenSound(feed);
    clearRemoteVideos();
    this.video = null;
    this.screenVideo = null;
    this.screenSound = null;
    this.channel?.close();
    this.pc?.close();
    this.channel = null;
    this.pc = null;
  }

  /** Leaves the call. `tell` asks the instance to let go of the place at once. */
  async stop(tell: boolean) {
    if (this.stopped) return;
    this.stopped = true;
    if (this.keeper) clearInterval(this.keeper);
    if (this.secretPoll) clearInterval(this.secretPoll);
    this.unwatch?.();
    this.unwant?.();
    this.unprefs?.();
    this.unmoderated?.();
    if (this.layersTimer) clearTimeout(this.layersTimer);
    if (this.broken) clearTimeout(this.broken);
    this.teardown();
    // Hanging up ends a recording and saves it.
    this.setRecording(false);
    this.mic?.close();
    this.camera?.stop();
    this.camera = null;
    setLocalVideo(null);
    this.screen?.stop();
    this.screen = null;
    this.screenAudio?.stop();
    this.screenAudio = null;
    setLocalScreen(null);
    this.speakers.close();
    this.frames?.close();
    if (!tell || !this.sessionId) return;
    const t = this.target;
    try {
      if (t.kind === "voice") await this.api.calls.leaveVoice({ serverId: t.serverId, sessionId: this.sessionId });
      else await this.api.calls.leaveDmCall({ conversationId: t.conversationId, sessionId: this.sessionId });
    } catch {
      // The place runs out by itself in a few seconds.
    }
  }

  /** When the page goes away: let go of the place without waiting. */
  unload() {
    if (!this.sessionId) return;
    const e = engine(this.target.instance);
    leaveOnUnload(e.url, e.token, this.target, this.sessionId);
  }
}

/** The instance says the place is gone for good (not a blip worth retrying). */
const gone = (e: FuwaError) =>
  e.code === Code.FailedPrecondition || e.code === Code.NotFound || e.code === Code.PermissionDenied || e.code === Code.Unauthenticated;

/** Waits for this browser's own addresses, briefly: the media server finds the rest as packets arrive. */
function gathered(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(timer);
      pc.removeEventListener("icegatheringstatechange", check);
      resolve();
    };
    const check = () => pc.iceGatheringState === "complete" && done();
    const timer = setTimeout(done, 1_200);
    pc.addEventListener("icegatheringstatechange", check);
  });
}

/** Opus first, with its forward error correction on, which the media server passes along. */
function preferOpus(transceiver: RTCRtpTransceiver) {
  const codecs = typeof RTCRtpReceiver !== "undefined" ? RTCRtpReceiver.getCapabilities?.("audio")?.codecs : undefined;
  if (!codecs || !transceiver.setCodecPreferences) return;
  const opus = codecs.filter((c) => c.mimeType.toLowerCase() === "audio/opus");
  if (!opus.length) return;
  try {
    transceiver.setCodecPreferences([...opus, ...codecs.filter((c) => !opus.includes(c))]);
  } catch {
    // The browser's own order is fine.
  }
}

/** VP8 first: the media server takes only Opus and VP8, so every app can show every camera. */
function preferVp8(transceiver: RTCRtpTransceiver) {
  const codecs = typeof RTCRtpSender !== "undefined" ? RTCRtpSender.getCapabilities?.("video")?.codecs : undefined;
  if (!codecs || !transceiver.setCodecPreferences) return;
  const vp8 = codecs.filter((c) => c.mimeType.toLowerCase() === "video/vp8");
  // Resending lost packets and forward error correction ride along.
  const helpers = codecs.filter((c) => ["video/rtx", "video/red", "video/ulpfec"].includes(c.mimeType.toLowerCase()));
  if (!vp8.length) return;
  try {
    transceiver.setCodecPreferences([...vp8, ...helpers]);
  } catch {
    // The browser's own order is fine.
  }
}

/** A track of silence, for joining to listen when there's no microphone. */
function silentTrack(): MediaStreamTrack {
  const ctx = new AudioContext();
  const dest = ctx.createMediaStreamDestination();
  return dest.stream.getAudioTracks()[0]!;
}

// ───────────────────────── The one call ─────────────────────────

let session: Session | null = null;

/** Joins a voice channel or a conversation's call, leaving any other call first. */
export async function joinCall(target: CallTarget) {
  if (session && sameTarget(session.target, target)) return;
  if (session) await hangUp(null);
  const s = new Session(target);
  session = s;
  reportUsage(target.kind === "voice" ? "call.join_voice" : "call.join_dm");
  setCalls(() => ({ call: { target, status: "connecting", since: null, problem: null }, speaking: {}, ended: null, selfVideo: false, selfStream: false, selfRecord: false, serverRecord: false, screenSound: null, screenSounds: {}, quietScreens: {} }));
  try {
    await s.start();
  } catch (err) {
    if (session !== s) return;
    const e = err instanceof Error && !("code" in err) ? err.message : toFuwaError(err).message;
    await hangUp(null);
    toast(e);
  }
}

/** Leaves the call, with `why` shown when it wasn't you. */
export async function hangUp(why: string | null, tell = true) {
  const s = session;
  if (!s) return;
  session = null;
  setCalls(() => ({ call: null, speaking: {}, ended: why, selfVideo: false, selfStream: false, selfRecord: false, serverRecord: false, screenSound: null, screenSounds: {}, quietScreens: {} }));
  cue("disconnect");
  if (why) toast(why);
  await s.stop(tell);
}

export function toggleMute() {
  const { selfMute, selfDeaf } = getCalls();
  // Unmuting while deafened undeafens too, as Discord does.
  if (selfDeaf && selfMute) setCalls(() => ({ selfMute: false, selfDeaf: false }));
  else setCalls(() => ({ selfMute: !selfMute }));
  cue(getCalls().selfMute ? "mute" : "unmute");
  session?.applyMute();
  void session?.keep();
}

export function toggleDeafen() {
  const { selfDeaf } = getCalls();
  // Deafening mutes you too, and undeafening unmutes, as Discord does.
  setCalls(() => (selfDeaf ? { selfDeaf: false, selfMute: false } : { selfDeaf: true, selfMute: true }));
  cue(selfDeaf ? "undeafen" : "deafen");
  session?.applyMute();
  void session?.keep();
}

/** Turns your camera on or off in the call you're in. */
export async function setCamera(on: boolean) {
  const s = session;
  if (!s || getCalls().selfVideo === on) return;
  if (on && s.videoSuppressed) return void toast("You can't turn your camera on in this channel.");
  if (on && s.videoOff) return void toast("A moderator turned your camera off here.");
  if (on) reportUsage("call.camera");
  setCalls(() => ({ selfVideo: on }));
  try {
    await s.setCamera(on);
    cue(on ? "unmute" : "mute");
  } catch (err) {
    setCalls(() => ({ selfVideo: false }));
    toast(cameraProblem(err));
  }
  void s.keep();
}

export const toggleCamera = () => setCamera(!getCalls().selfVideo);

/** Shares your screen in the call you're in, or stops. */
export async function setScreen(on: boolean) {
  const s = session;
  if (!s || getCalls().selfStream === on) return;
  if (on && s.videoSuppressed) return void toast("You can't share your screen in this channel.");
  if (on && s.videoOff) return void toast("A moderator turned screen sharing off for you here.");
  if (on) reportUsage("call.screen_share");
  setCalls(() => ({ selfStream: on }));
  try {
    await s.setScreen(on, getPrefs().shareSound);
    cue(on ? "unmute" : "mute");
  } catch (err) {
    setCalls(() => ({ selfStream: false }));
    const problem = screenProblem(err);
    if (problem) toast(problem);
  }
  void s.keep();
}

export const toggleScreen = () => setScreen(!getCalls().selfStream);

/** Starts sharing your screen, with its sound or without (and remembers which you like). */
export async function shareScreen(sound: boolean) {
  setPrefs({ shareSound: sound });
  await setScreen(true);
}

/** Turns your shared screen's sound off, or back on, while you share. */
export function setScreenSound(on: boolean) {
  const s = session;
  if (!s || getCalls().screenSound === null || getCalls().screenSound === on) return;
  s.setScreenSound(on);
  cue(on ? "unmute" : "mute");
}

/** Turns someone's shared screen's sound off for you, or back on. */
export function toggleScreenQuiet(userId: string) {
  setCalls((s) => {
    const { [userId]: was, ...rest } = s.quietScreens;
    return { quietScreens: was ? rest : { ...rest, [userId]: true } };
  });
  session?.applyVolumes();
}

/** Starts or stops recording the call you're in, on this device. Everyone in it sees that you are. */
export function setRecording(on: boolean) {
  const s = session;
  if (!s || getCalls().selfRecord === on) return;
  if (on && s.recordSuppressed) return void toast("You can't record in this channel.");
  if (on && typeof MediaRecorder === "undefined") return void toast("This browser can't record.");
  setCalls(() => ({ selfRecord: on }));
  s.setRecording(on);
  cue(on ? "recording" : "mute");
  void s.keep();
}

export const toggleRecording = () => setRecording(!getCalls().selfRecord);

/**
 * Starts or stops recording the voice channel you're in on the server: it
 * keeps everyone's sound, a track per person, for the people with Record
 * there. Everyone in the channel sees it, and hears a beep when it starts.
 */
export function setServerRecording(on: boolean) {
  const s = session;
  if (!s || s.target.kind !== "voice" || getCalls().serverRecord === on) return;
  if (on && s.recordSuppressed) return void toast("You can't record in this channel.");
  if (on && !getCalls().serverRecordings) return void toast("This instance doesn't record voice channels on the server.");
  setCalls(() => ({ serverRecord: on }));
  cue(on ? "recording" : "mute");
  if (!on) toast("Stopped. The recording is in this channel's recordings.");
  void s.keep();
}

export const toggleServerRecording = () => setServerRecording(!getCalls().serverRecord);

/** What recordings are saved as, best first: Opus wherever the browser can. */
const RECORDING_TYPES = ["audio/webm;codecs=opus", "audio/ogg;codecs=opus", "audio/mp4"];

/** Hands a finished recording to the browser to save, named for when it started. */
function saveRecording(blob: Blob, started: Date) {
  if (!blob.size) return;
  const pad = (n: number) => String(n).padStart(2, "0");
  const when = `${started.getFullYear()}-${pad(started.getMonth() + 1)}-${pad(started.getDate())} ${pad(started.getHours())}.${pad(started.getMinutes())}`;
  const ext = blob.type.includes("ogg") ? "ogg" : blob.type.includes("mp4") ? "m4a" : "webm";
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = `fuwa call ${when}.${ext}`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 60_000);
}

/** Push to talk's key went down or up. */
export function setPushing(down: boolean) {
  if (getCalls().talking === down) return;
  setCalls(() => ({ talking: down }));
  session?.applyMute();
}

/** Someone's volume, or the output device, changed. */
export const applyVolumes = () => session?.applyVolumes();

if (typeof window !== "undefined") {
  window.addEventListener("pagehide", () => session?.unload());
}
