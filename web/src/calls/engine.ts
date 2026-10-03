import { Code } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { createClient } from "@connectrpc/connect";
import { CallService, type GetCallSettingsResponse } from "@/gen/fuwa/v1/call_pb";
import { dmEngine } from "@/e2ee/engine";
import { toFuwaError, type FuwaError } from "@/fuwa/errors";
import { engine } from "@/fuwa/sync";
import { store } from "@/fuwa/store";
import { getPrefs } from "@/lib/prefs";
import { cue } from "@/lib/sounds";
import { toast } from "@/lib/ui";
import { Mic, micProblem, Speakers } from "./audio";
import { canEncryptCalls, encryptedConfig, FrameCrypto } from "./frames";
import { getCalls, sameTarget, setCalls, type CallTarget } from "./state";

/**
 * This browser's call: one RTCPeerConnection to the instance's media
 * server, sending your microphone and getting one track per other person
 * (each track's stream id is their user id, so each voice is its own).
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

type Signal = { type: "offer" | "answer"; sdp: string } | { type: "replaced" | "restarting" | "closed" };

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
  private speakers: Speakers;
  private frames: FrameCrypto | null = null;
  private sessionId = "";
  private settings: GetCallSettingsResponse | null = null;
  private keeper: ReturnType<typeof setInterval> | null = null;
  private secretPoll: ReturnType<typeof setInterval> | null = null;
  private broken: ReturnType<typeof setTimeout> | null = null;
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
    const channel = pc.createDataChannel(CHANNEL, { ordered: true });
    this.channel = channel;
    channel.onmessage = (e) => void this.onSignal(pc, e.data);
    pc.ontrack = (e) => {
      const userId = e.streams[0]?.id;
      if (!userId) return;
      this.frames?.receive(e.receiver, userId);
      const stream = e.streams[0]!;
      this.speakers.add(userId, stream);
      if (!this.heard.has(userId) && getCalls().call?.status === "connected") cue("someoneJoined");
      this.heard.add(userId);
      stream.onremovetrack = () => {
        if (stream.getTracks().length) return;
        this.speakers.remove(userId);
        this.heard.delete(userId);
        this.setSpeaking(userId, false);
        if (getCalls().call?.status === "connected") cue("someoneLeft");
      };
    };
    pc.onconnectionstatechange = () => {
      if (pc !== this.pc || this.stopped) return;
      const state = pc.connectionState;
      if (state === "connected") {
        if (this.broken) clearTimeout(this.broken);
        this.broken = null;
        const before = getCalls().call;
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
    const { selfMute, selfDeaf } = getCalls();
    const sdp = pc.localDescription?.sdp ?? offer.sdp ?? "";
    const t = this.target;
    const joined =
      t.kind === "voice"
        ? await this.api.calls.joinVoice({ serverId: t.serverId, channelId: t.channelId, offer: sdp, selfMute, selfDeaf, sessionId: this.sessionId })
        : await this.api.calls.joinDmCall({ conversationId: t.conversationId, offer: sdp, selfMute, selfDeaf, sessionId: this.sessionId });
    if (attempt !== this.attempt || this.stopped) return;
    this.sessionId = joined.sessionId;
    await pc.setRemoteDescription({ type: "answer", sdp: joined.answer });
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
    const tryAgain = async (delay: number) => {
      if (this.stopped) return;
      try {
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

  /** Keeps the place in the call, and tells the instance how you sound. */
  async keep() {
    if (this.stopped || !this.sessionId) return;
    const { selfMute, selfDeaf } = getCalls();
    const t = this.target;
    try {
      if (t.kind === "voice") {
        await this.api.calls.keepVoice({ serverId: t.serverId, channelId: t.channelId, sessionId: this.sessionId, selfMute, selfDeaf });
      } else {
        await this.api.calls.keepDmCall({ conversationId: t.conversationId, sessionId: this.sessionId, selfMute, selfDeaf });
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
    if (this.broken) clearTimeout(this.broken);
    this.teardown();
    this.mic?.close();
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
  setCalls(() => ({ call: { target, status: "connecting", since: null, problem: null }, speaking: {}, ended: null }));
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
  setCalls(() => ({ call: null, speaking: {}, ended: why }));
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
