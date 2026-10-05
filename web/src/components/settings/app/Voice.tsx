import { AudioLinesIcon, CheckIcon, ChevronDownIcon, HeadphonesIcon, KeyboardIcon, MicIcon, PlayIcon, SquareIcon, VideoIcon, Volume2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import { Mic, micProblem, canPickOutput, audioContext } from "@/calls/audio";
import { cameraProblem, openCamera } from "@/calls/video";
import { VideoView } from "@/components/calls/Video";
import { SPRING } from "@/lib/motion";
import { Choice, Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Slider } from "@/components/ui/slider";
import { type I18n, type Key, useI18n } from "@/i18n/react";
import { actionById, bindingOf } from "@/lib/keybinds";
import { setPrefs, usePrefs, type InputMode } from "@/lib/prefs";
import { cue, play } from "@/lib/sounds";
import { openSettings } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { Keycaps, PrefSetting } from "./common";

export const voiceSettings = (t: I18n["t"]) => [
  { id: "devices", label: t("appsettings.voice.devices"), keywords: "input output device headset" },
  { id: "mic-test", label: t("appsettings.voice.micTest"), keywords: "check level meter" },
  { id: "input-mode", label: t("appsettings.voice.inputMode"), keywords: "voice activity push to talk ptt" },
  { id: "sensitivity", label: t("appsettings.voice.sensitivity"), keywords: "threshold gate noise" },
  { id: "processing", label: t("appsettings.voice.processing"), keywords: "echo noise suppression gain" },
  { id: "camera", label: t("appsettings.voice.camera"), keywords: "video webcam mirror preview" },
  { id: "call-sounds", label: t("appsettings.voice.callSounds"), keywords: "ring ringtone join leave" },
];

/** A device; `label` is empty until the browser has been allowed to name it, so it goes by its place in the list. */
type Device = { id: string; label: string; n: number };

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
            .map((d, n) => ({ id: d.deviceId, label: d.label, n: n + 1 }));
        setDevices({ inputs: pick("audioinput"), outputs: pick("audiooutput"), cameras: pick("videoinput") });
      });
    read();
    navigator.mediaDevices.addEventListener("devicechange", read);
    return () => navigator.mediaDevices.removeEventListener("devicechange", read);
  }, []);
  return devices;
}

function DevicePicker({
  icon: Icon,
  label,
  unnamed,
  value,
  devices,
  onChange,
  disabled,
}: {
  icon: typeof MicIcon;
  label: string;
  /** What a device the browser hasn't named yet is called, with its {number}. */
  unnamed: Key;
  value: string;
  devices: Device[];
  onChange: (id: string) => void;
  disabled?: string;
}) {
  const { t } = useI18n();
  const name = (d: Device) => d.label || t(unnamed, { number: d.n });
  const picked = devices.find((d) => d.id === value);
  const current = picked ? name(picked) : t("appsettings.voice.systemDefault");
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
          {[{ id: "", label: t("appsettings.voice.systemDefault"), n: 0 }, ...devices].map((d) => (
            <DropdownMenuItem key={d.id || "default"} onSelect={() => onChange(d.id)}>
              <span className="min-w-0 flex-1 truncate">{name(d)}</span>
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
  const { t } = useI18n();
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
          {testing ? t("appsettings.voice.stop") : t("appsettings.voice.micTestStart")}
        </Button>
        <AnimatePresence>
          {testing && (
            <motion.span initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING}>
              <Toggle checked={loopback} onChange={setLoopback} label={t("appsettings.voice.hearMyself")} hint={t("appsettings.voice.hearMyselfHint")} />
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
  const { t } = useI18n();
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
          {testing ? t("appsettings.voice.stop") : t("appsettings.voice.cameraTestStart")}
        </Button>
        <Toggle checked={mirror} onChange={(mirrorVideo) => setPrefs({ mirrorVideo })} label={t("appsettings.voice.mirror")} hint={t("appsettings.voice.mirrorHint")} />
      </div>
      {problem && <p className="text-sm text-destructive">{problem}</p>}
    </div>
  );
}

/** A level meter: the bar moves with your voice and turns green while what you say goes out. */
function Meter({ level, threshold, open }: { level: number; threshold?: number; open: boolean }) {
  const { t } = useI18n();
  return (
    <div className="relative h-3 w-full overflow-hidden rounded-full bg-muted" role="meter" aria-valuemin={-100} aria-valuemax={0} aria-valuenow={Math.round(level)} aria-label={t("appsettings.voice.micLevel")}>
      <motion.div
        className={cn("absolute inset-0 rounded-full transition-colors duration-150", open ? "bg-[#3ba55d]" : "bg-primary/60")}
        animate={{ x: `${meterAt(level) * 100 - 100}%` }}
        transition={{ type: "spring", stiffness: 900, damping: 40 }}
      />
      {threshold !== undefined && (
        <motion.div className="absolute inset-0" animate={{ x: `${meterAt(threshold) * 100}%` }} transition={{ type: "spring", stiffness: 300, damping: 30 }}>
          <div className="absolute inset-y-0 left-0 w-0.5 bg-foreground/70" />
        </motion.div>
      )}
    </div>
  );
}

export function Voice() {
  const { t, number } = useI18n();
  const p = usePrefs((x) => x);
  const devices = useDevices();
  const ptt = usePrefs((x) => bindingOf(actionById("pushToTalk")!, x));
  return (
    <div className="flex flex-col">
      <PrefSetting id="devices" title={t("appsettings.voice.devices")} keys={["inputDevice", "outputDevice", "inputVolume", "outputVolume"]}>
        <div className="flex flex-col gap-4 sm:flex-row">
          <div className="flex min-w-0 flex-1 flex-col gap-3">
            <DevicePicker icon={MicIcon} label={t("appsettings.voice.mic")} unnamed="appsettings.voice.unnamedMic" value={p.inputDevice} devices={devices.inputs} onChange={(inputDevice) => setPrefs({ inputDevice })} />
            <Volume label={t("appsettings.voice.micVolume")} value={p.inputVolume} onChange={(inputVolume) => setPrefs({ inputVolume })} />
          </div>
          <div className="flex min-w-0 flex-1 flex-col gap-3">
            <DevicePicker
              icon={HeadphonesIcon}
              label={t("appsettings.voice.speakers")}
              unnamed="appsettings.voice.unnamedSpeakers"
              value={p.outputDevice}
              devices={devices.outputs}
              onChange={(outputDevice) => setPrefs({ outputDevice })}
              disabled={canPickOutput() ? undefined : t("appsettings.voice.systemSpeakers")}
            />
            <Volume label={t("appsettings.voice.callVolume")} value={p.outputVolume} onChange={(outputVolume) => setPrefs({ outputVolume })} onCommit={() => cue("someoneJoined")} />
          </div>
        </div>
      </PrefSetting>
      <PrefSetting id="mic-test" title={t("appsettings.voice.micTest")} hint={t("appsettings.voice.micTestHint")} keys={[]} delay={0.04}>
        <MicTest />
      </PrefSetting>
      <PrefSetting id="input-mode" title={t("appsettings.voice.inputMode")} keys={["inputMode", "pttRelease"]} delay={0.08}>
        <Choice<InputMode>
          value={p.inputMode}
          onChange={(inputMode) => setPrefs({ inputMode })}
          options={[
            { value: "voice", label: t("appsettings.voice.voiceActivity"), hint: t("appsettings.voice.voiceActivityHint"), icon: <AudioLinesIcon className="size-4" /> },
            { value: "ptt", label: t("appsettings.voice.pushToTalk"), hint: t("appsettings.voice.pushToTalkHint"), icon: <KeyboardIcon className="size-4" /> },
          ]}
        />
        <AnimatePresence initial={false}>
          {p.inputMode === "ptt" && (
            <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="flex flex-col gap-3 overflow-hidden">
              <div className="flex flex-wrap items-center gap-2 text-sm">
                <span className="text-muted-foreground">{t("appsettings.voice.yourKey")}</span>
                {ptt ? <Keycaps combo={ptt} /> : <span className="font-bold text-destructive">{t("appsettings.voice.noKey")}</span>}
                <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={() => openSettings("keybinds")}>
                  <KeyboardIcon /> {ptt ? t("appsettings.voice.changeKey") : t("appsettings.voice.pickKey")}
                </Button>
              </div>
              <Slider label={t("appsettings.voice.releaseDelay")} value={p.pttRelease} min={0} max={2000} step={20} format={(n) => number(n, { style: "unit", unit: "millisecond" })} onChange={(pttRelease) => setPrefs({ pttRelease })} />
            </motion.div>
          )}
        </AnimatePresence>
      </PrefSetting>
      <AnimatePresence initial={false}>
        {p.inputMode === "voice" && (
          <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
            <PrefSetting id="sensitivity" title={t("appsettings.voice.sensitivity")} keys={["autoSensitivity", "sensitivity"]} delay={0.12}>
              <Toggle checked={p.autoSensitivity} onChange={(autoSensitivity) => setPrefs({ autoSensitivity })} label={t("appsettings.voice.autoSensitivity")} hint={t("appsettings.voice.autoSensitivityHint")} />
              <AnimatePresence initial={false}>
                {!p.autoSensitivity && (
                  <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
                    <Slider label={t("appsettings.voice.opensAt")} value={p.sensitivity} min={-100} max={0} step={1} format={(n) => t("appsettings.voice.decibels", { value: n })} onChange={(sensitivity) => setPrefs({ sensitivity })} />
                  </motion.div>
                )}
              </AnimatePresence>
            </PrefSetting>
          </motion.div>
        )}
      </AnimatePresence>
      <PrefSetting id="processing" title={t("appsettings.voice.processing")} keys={["echoCancellation", "noiseSuppression", "autoGainControl"]} delay={0.16}>
        <Toggle checked={p.echoCancellation} onChange={(echoCancellation) => setPrefs({ echoCancellation })} label={t("appsettings.voice.echo")} hint={t("appsettings.voice.echoHint")} />
        <Toggle checked={p.noiseSuppression} onChange={(noiseSuppression) => setPrefs({ noiseSuppression })} label={t("appsettings.voice.noise")} hint={t("appsettings.voice.noiseHint")} />
        <Toggle checked={p.autoGainControl} onChange={(autoGainControl) => setPrefs({ autoGainControl })} label={t("appsettings.voice.gain")} hint={t("appsettings.voice.gainHint")} />
      </PrefSetting>
      <PrefSetting id="camera" title={t("appsettings.voice.camera")} keys={["videoDevice", "mirrorVideo"]} delay={0.2}>
        <div className="flex flex-col gap-4">
          <DevicePicker icon={VideoIcon} label={t("appsettings.voice.camera")} unnamed="appsettings.voice.unnamedCamera" value={p.videoDevice} devices={devices.cameras} onChange={(videoDevice) => setPrefs({ videoDevice })} />
          <CameraTest />
        </div>
      </PrefSetting>
      <PrefSetting id="call-sounds" title={t("appsettings.voice.callSounds")} keys={["sounds"]} delay={0.2}>
        <SoundRow label={t("appsettings.voice.cues")} hint={t("appsettings.voice.cuesHint")} playLabel={t("appsettings.voice.playCues")} on={p.sounds.call} onChange={(call) => setPrefs((x) => ({ sounds: { ...x.sounds, call } }))} preview={() => cue("connect")} />
        <SoundRow label={t("appsettings.voice.ringtone")} hint={t("appsettings.voice.ringtoneHint")} playLabel={t("appsettings.voice.playRingtone")} on={p.sounds.ring} onChange={(ring) => setPrefs((x) => ({ sounds: { ...x.sounds, ring } }))} preview={() => play("ring", true)} />
      </PrefSetting>
    </div>
  );
}

/** A volume from 0 to 200%, with its name and value above it. */
function Volume({ label, value, onChange, onCommit }: { label: string; value: number; onChange: (v: number) => void; onCommit?: () => void }) {
  const { number } = useI18n();
  const percent = (n: number) => number(n / 100, { style: "percent" });
  return (
    <div>
      <div className="-mb-5 flex items-baseline justify-between text-sm">
        <span className="font-bold">{label}</span>
        <span className="text-xs font-bold text-muted-foreground tabular-nums">{percent(value)}</span>
      </div>
      <Slider label={label} value={value} min={0} max={200} step={5} format={percent} marks={[{ value: 100, label: percent(100) }]} onChange={onChange} onCommit={onCommit} />
    </div>
  );
}

function SoundRow({ label, hint, playLabel, on, onChange, preview }: { label: string; hint: string; playLabel: string; on: boolean; onChange: (on: boolean) => void; preview: () => void }) {
  return (
    <div className="flex items-center gap-3">
      <button
        type="button"
        onClick={preview}
        aria-label={playLabel}
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
