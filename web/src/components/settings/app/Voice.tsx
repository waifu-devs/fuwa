import { AudioLinesIcon, CheckIcon, ChevronDownIcon, HeadphonesIcon, KeyboardIcon, MicIcon, PlayIcon, SquareIcon, VideoIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { Mic, micProblem, canPickOutput, audioContext } from "@/calls/audio";
import { cameraProblem, openCamera } from "@/calls/video";
import { VideoView } from "@/components/calls/Video";
import { SPRING } from "@/components/motion";
import { Choice, Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Slider } from "@/components/ui/slider";
import { actionById, bindingOf } from "@/lib/keybinds";
import { setPrefs, usePrefs, type InputMode } from "@/lib/prefs";
import { cue, play } from "@/lib/sounds";
import { openSettings } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { Keycaps, PrefSetting } from "./common";

export const VOICE_SETTINGS = [
  { id: "devices", label: "Microphone and speakers", keywords: "input output device headset" },
  { id: "mic-test", label: "Mic test", keywords: "check level meter" },
  { id: "input-mode", label: "Input mode", keywords: "voice activity push to talk ptt" },
  { id: "sensitivity", label: "Sensitivity", keywords: "threshold gate noise" },
  { id: "processing", label: "Processing", keywords: "echo noise suppression gain" },
  { id: "camera", label: "Camera", keywords: "video webcam mirror preview" },
  { id: "call-sounds", label: "Call sounds", keywords: "ring ringtone join leave" },
];

type Device = { id: string; label: string };

/** The microphones, speakers and cameras this browser can use. Names show once it's been allowed them. */
function useDevices() {
  const [devices, setDevices] = useState<{ inputs: Device[]; outputs: Device[]; cameras: Device[] }>({ inputs: [], outputs: [], cameras: [] });
  useEffect(() => {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    const read = () =>
      void navigator.mediaDevices.enumerateDevices().then((list) => {
        const pick = (kind: MediaDeviceKind) =>
          list
            .filter((d) => d.kind === kind && d.deviceId && d.deviceId !== "default" && d.deviceId !== "communications")
            .map((d, n) => ({ id: d.deviceId, label: d.label || `${kind === "audioinput" ? "Microphone" : kind === "videoinput" ? "Camera" : "Speakers"} ${n + 1}` }));
        setDevices({ inputs: pick("audioinput"), outputs: pick("audiooutput"), cameras: pick("videoinput") });
      });
    read();
    navigator.mediaDevices.addEventListener("devicechange", read);
    return () => navigator.mediaDevices.removeEventListener("devicechange", read);
  }, []);
  return devices;
}

function DevicePicker({ icon: Icon, label, value, devices, onChange, disabled }: { icon: typeof MicIcon; label: string; value: string; devices: Device[]; onChange: (id: string) => void; disabled?: string }) {
  const current = devices.find((d) => d.id === value)?.label ?? "System default";
  return (
    <div className="flex min-w-0 flex-1 flex-col gap-1.5">
      <span className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <Icon className="size-3.5" /> {label}
      </span>
      <DropdownMenu>
        <DropdownMenuTrigger asChild disabled={!!disabled}>
          <button
            type="button"
            title={disabled}
            className="group flex h-10 w-full min-w-0 items-center gap-2 rounded-xl border bg-background px-3 text-left text-sm transition hover:border-primary/50 disabled:opacity-60 data-[state=open]:border-primary"
          >
            <span className="min-w-0 flex-1 truncate">{current}</span>
            <ChevronDownIcon className="size-4 shrink-0 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-[var(--radix-dropdown-menu-trigger-width)] min-w-56">
          {[{ id: "", label: "System default" }, ...devices].map((d) => (
            <DropdownMenuItem key={d.id || "default"} onSelect={() => onChange(d.id)}>
              <span className="min-w-0 flex-1 truncate">{d.label}</span>
              {d.id === value && <CheckIcon className="text-primary" />}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      {disabled && <span className="text-xs text-muted-foreground">{disabled}</span>}
    </div>
  );
}

/** -100..0 dB as a fraction of a meter. */
const meterAt = (db: number) => Math.min(1, Math.max(0, (db + 100) / 100));

/** Your microphone's level live, with where voice activity opens; optionally played back to you. */
function MicTest() {
  const [testing, setTesting] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [state, setState] = useState({ level: -100, threshold: -50, open: false });
  const [loopback, setLoopback] = useState(false);
  const [mic, setMic] = useState<Mic | null>(null);

  useEffect(() => {
    if (!testing) return;
    let cancelled = false;
    let opened: Mic | null = null;
    Mic.open((s) => !cancelled && setState(s))
      .then((m) => {
        if (cancelled) return m.close();
        opened = m;
        setMic(m);
        setProblem(null);
      })
      .catch((err) => {
        if (cancelled) return;
        setProblem(micProblem(err));
        setTesting(false);
      });
    return () => {
      cancelled = true;
      opened?.close();
      setMic(null);
    };
  }, [testing]);

  // Hearing yourself: what would be sent, gate and all, to your speakers.
  useEffect(() => {
    if (!mic || !loopback) return;
    const ctx = audioContext();
    const source = ctx.createMediaStreamSource(new MediaStream([mic.track]));
    source.connect(ctx.destination);
    return () => source.disconnect();
  }, [mic, loopback]);

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" variant={testing ? "secondary" : "default"} className="btn group rounded-xl font-bold" onClick={() => setTesting((t) => !t)}>
          {testing ? <SquareIcon className="size-3.5" /> : <PlayIcon className="transition-transform group-hover:scale-110" />}
          {testing ? "Stop" : "Let's check"}
        </Button>
        <AnimatePresence>
          {testing && (
            <motion.span initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING}>
              <Toggle checked={loopback} onChange={setLoopback} label="Hear myself" hint="Use headphones, or you'll hear an echo." />
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      <Meter level={testing ? state.level : -100} threshold={state.threshold} open={testing && state.open} />
      {problem && <p className="text-sm text-destructive">{problem}</p>}
    </div>
  );
}

/** Your camera, as others will see it (mirrored for you, if you like), until you stop it. */
function CameraTest() {
  const [testing, setTesting] = useState(false);
  const [track, setTrack] = useState<MediaStreamTrack | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const device = usePrefs((p) => p.videoDevice);
  const mirror = usePrefs((p) => p.mirrorVideo);

  useEffect(() => {
    if (!testing) return;
    let cancelled = false;
    let opened: MediaStreamTrack | null = null;
    openCamera()
      .then((t) => {
        if (cancelled) return t.stop();
        opened = t;
        setTrack(t);
        setProblem(null);
      })
      .catch((err) => {
        if (cancelled) return;
        setProblem(cameraProblem(err));
        setTesting(false);
      });
    return () => {
      cancelled = true;
      opened?.stop();
      setTrack(null);
    };
  }, [testing, device]);

  return (
    <div className="flex flex-col gap-3">
      <div className="relative aspect-video w-full max-w-md overflow-hidden rounded-2xl border bg-muted">
        <AnimatePresence>
          {track ? (
            <motion.div key="on" initial={{ opacity: 0, scale: 1.04 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0 }} transition={{ duration: 0.3 }} className="absolute inset-0 bg-black">
              <VideoView track={track} mirror={mirror} />
            </motion.div>
          ) : (
            <motion.div key="off" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="absolute inset-0 grid place-items-center text-muted-foreground">
              <VideoIcon className="size-8" />
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <Button type="button" variant={testing ? "secondary" : "default"} className="btn group rounded-xl font-bold" onClick={() => setTesting((t) => !t)}>
          {testing ? <SquareIcon className="size-3.5" /> : <PlayIcon className="transition-transform group-hover:scale-110" />}
          {testing ? "Stop" : "Check my camera"}
        </Button>
        <Toggle checked={mirror} onChange={(mirrorVideo) => setPrefs({ mirrorVideo })} label="Mirror my camera" hint="Only for you: everyone else sees it the right way round." />
      </div>
      {problem && <p className="text-sm text-destructive">{problem}</p>}
    </div>
  );
}

/** A level meter: the bar moves with your voice and turns green while what you say goes out. */
function Meter({ level, threshold, open }: { level: number; threshold?: number; open: boolean }) {
  return (
    <div className="relative h-3 w-full overflow-hidden rounded-full bg-muted" role="meter" aria-valuemin={-100} aria-valuemax={0} aria-valuenow={Math.round(level)} aria-label="Microphone level">
      <motion.div
        className={cn("absolute inset-y-0 left-0 rounded-full transition-colors duration-150", open ? "bg-[#3ba55d]" : "bg-primary/60")}
        animate={{ width: `${meterAt(level) * 100}%` }}
        transition={{ type: "spring", stiffness: 900, damping: 40 }}
      />
      {threshold !== undefined && (
        <motion.div
          className="absolute inset-y-0 w-0.5 bg-foreground/70"
          animate={{ left: `${meterAt(threshold) * 100}%` }}
          transition={{ type: "spring", stiffness: 300, damping: 30 }}
        />
      )}
    </div>
  );
}

export function Voice() {
  const p = usePrefs((x) => x);
  const devices = useDevices();
  const ptt = usePrefs((x) => bindingOf(actionById("pushToTalk")!, x));
  return (
    <div className="flex flex-col">
      <PrefSetting id="devices" title="Microphone and speakers" keys={["inputDevice", "outputDevice", "inputVolume", "outputVolume"]}>
        <div className="flex flex-col gap-4 sm:flex-row">
          <div className="flex min-w-0 flex-1 flex-col gap-3">
            <DevicePicker icon={MicIcon} label="Microphone" value={p.inputDevice} devices={devices.inputs} onChange={(inputDevice) => setPrefs({ inputDevice })} />
            <Volume label="Microphone volume" value={p.inputVolume} onChange={(inputVolume) => setPrefs({ inputVolume })} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-3">
            <DevicePicker
              icon={HeadphonesIcon}
              label="Speakers"
              value={p.outputDevice}
              devices={devices.outputs}
              onChange={(outputDevice) => setPrefs({ outputDevice })}
              disabled={canPickOutput() ? undefined : "This browser plays calls through the system's speakers."}
            />
            <Volume label="Call volume" value={p.outputVolume} onChange={(outputVolume) => setPrefs({ outputVolume })} onCommit={() => cue("someoneJoined")} />
          </div>
        </div>
      </PrefSetting>
      <PrefSetting id="mic-test" title="Mic test" hint="Say something and watch the bar. It turns green when what you say would go out." keys={[]} delay={0.04}>
        <MicTest />
      </PrefSetting>
      <PrefSetting id="input-mode" title="Input mode" keys={["inputMode", "pttRelease"]} delay={0.08}>
        <Choice<InputMode>
          value={p.inputMode}
          onChange={(inputMode) => setPrefs({ inputMode })}
          options={[
            { value: "voice", label: "Voice activity", hint: "Goes out when you talk.", icon: <AudioLinesIcon className="size-4" /> },
            { value: "ptt", label: "Push to talk", hint: "Goes out while you hold a key.", icon: <KeyboardIcon className="size-4" /> },
          ]}
        />
        <AnimatePresence initial={false}>
          {p.inputMode === "ptt" && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="flex flex-col gap-3 overflow-hidden">
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span className="text-muted-foreground">Your key:</span>
                {ptt ? <Keycaps combo={ptt} /> : <span className="font-bold text-destructive">none yet</span>}
                <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={() => openSettings("keybinds")}>
                  <KeyboardIcon /> {ptt ? "Change" : "Pick one"}
                </Button>
              </div>
              <Slider label="Release delay" value={p.pttRelease} min={0} max={2000} step={20} format={(n) => `${n} ms`} onChange={(pttRelease) => setPrefs({ pttRelease })} />
            </motion.div>
          )}
        </AnimatePresence>
      </PrefSetting>
      <AnimatePresence initial={false}>
        {p.inputMode === "voice" && (
          <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
            <PrefSetting id="sensitivity" title="Sensitivity" keys={["autoSensitivity", "sensitivity"]} delay={0.12}>
              <Toggle checked={p.autoSensitivity} onChange={(autoSensitivity) => setPrefs({ autoSensitivity })} label="Pick it for me" hint="Follows the noise in your room." />
              <AnimatePresence initial={false}>
                {!p.autoSensitivity && (
                  <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
                    <Slider label="Opens at" value={p.sensitivity} min={-100} max={0} step={1} format={(n) => `${n} dB`} onChange={(sensitivity) => setPrefs({ sensitivity })} />
                  </motion.div>
                )}
              </AnimatePresence>
            </PrefSetting>
          </motion.div>
        )}
      </AnimatePresence>
      <PrefSetting id="processing" title="Processing" keys={["echoCancellation", "noiseSuppression", "autoGainControl"]} delay={0.16}>
        <Toggle checked={p.echoCancellation} onChange={(echoCancellation) => setPrefs({ echoCancellation })} label="Echo cancellation" hint="Keeps your speakers out of your microphone." />
        <Toggle checked={p.noiseSuppression} onChange={(noiseSuppression) => setPrefs({ noiseSuppression })} label="Noise suppression" hint="Softens fans, keyboards and the street." />
        <Toggle checked={p.autoGainControl} onChange={(autoGainControl) => setPrefs({ autoGainControl })} label="Automatic gain" hint="Evens out how loud you are. Off for a studio microphone." />
      </PrefSetting>
      <PrefSetting id="camera" title="Camera" keys={["videoDevice", "mirrorVideo"]} delay={0.2}>
        <div className="flex flex-col gap-4">
          <DevicePicker icon={VideoIcon} label="Camera" value={p.videoDevice} devices={devices.cameras} onChange={(videoDevice) => setPrefs({ videoDevice })} />
          <CameraTest />
        </div>
      </PrefSetting>
      <PrefSetting id="call-sounds" title="Call sounds" keys={["sounds"]} delay={0.2}>
        <SoundRow label="Joining, leaving, mute and deafen" hint="Little cues while you're in a call." on={p.sounds.call} onChange={(call) => setPrefs((x) => ({ sounds: { ...x.sounds, call } }))} preview={() => cue("connect")} />
        <SoundRow label="Ringtone" hint="When someone calls you." on={p.sounds.ring} onChange={(ring) => setPrefs((x) => ({ sounds: { ...x.sounds, ring } }))} preview={() => play("ring", true)} />
      </PrefSetting>
    </div>
  );
}

/** A volume from 0 to 200%, with its name and value above it. */
function Volume({ label, value, onChange, onCommit }: { label: string; value: number; onChange: (v: number) => void; onCommit?: () => void }) {
  return (
    <div>
      <div className="-mb-5 flex items-baseline justify-between text-sm">
        <span className="font-bold">{label}</span>
        <span className="text-xs font-bold text-muted-foreground tabular-nums">{value}%</span>
      </div>
      <Slider label={label} value={value} min={0} max={200} step={5} format={(n) => `${n}%`} marks={[{ value: 100, label: "100%" }]} onChange={onChange} onCommit={onCommit} />
    </div>
  );
}

function SoundRow({ label, hint, on, onChange, preview }: { label: string; hint: string; on: boolean; onChange: (on: boolean) => void; preview: () => void }) {
  return (
    <div className="flex items-center gap-3">
      <button
        type="button"
        onClick={preview}
        aria-label={`Play: ${label.toLowerCase()}`}
        className="group grid size-9 shrink-0 place-items-center rounded-full bg-muted text-muted-foreground transition hover:bg-primary hover:text-primary-foreground active:scale-90"
      >
        <Volume2Icon className="size-4 transition-transform group-hover:scale-110" />
      </button>
      <div className="min-w-0 flex-1">
        <Toggle checked={on} onChange={onChange} label={label} hint={hint} />
      </div>
    </div>
  );
}
