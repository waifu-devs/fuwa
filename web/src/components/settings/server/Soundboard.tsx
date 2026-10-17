import { Code } from "@connectrpc/connect";
import { AudioLinesIcon, LoaderCircleIcon, PlayIcon, SmilePlusIcon, SquareIcon, Trash2Icon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useRef, useState, type DragEvent } from "react";
import type { Sound } from "@/gen/fuwa/v1/types_pb";
import { playLoose } from "@/calls/audio";
import { run, uploadSound } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { createSound, deleteSound, previewSound, updateSound, useSounds } from "@/fuwa/soundboard";
import { SoundEmoji } from "@/components/calls/Soundboard";
import { EmojiPicker } from "@/components/EmojiPicker";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { displayName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { clampWindow, freeName, MAX_SOUND_MS, MAX_SOUND_NAME, soundEmoji, soundNameFromFile, validName } from "@/lib/soundboard";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { T, useI18n } from "@/i18n/react";
import { canMakeSounds, decodeSoundFile, lengthMs, makeSound } from "@/voice/sound-file";

/** How many bars the picked file's waveform has. */
const BARS = 96;

/** A picked file on its way to being a sound. */
type Draft = {
  file: File;
  buffer: AudioBuffer | null;
  startMs: number;
  name: string;
  emoji: string;
  volume: number;
  stage: "reading" | "ready" | "making" | "uploading";
  sent: number;
};

/** Seconds as people read them: "7.2 s", or "1:05.3" past a minute. */
function useSeconds() {
  const { number } = useI18n();
  return (ms: number) => {
    const s = Math.max(0, ms) / 1000;
    if (s < 60) return number(s, { style: "unit", unit: "second", maximumFractionDigits: 1, minimumFractionDigits: s < 10 ? 1 : 0 });
    const m = Math.floor(s / 60);
    return `${m}:${(s - m * 60).toFixed(1).padStart(4, "0")}`;
  };
}

/**
 * The server's soundboard (docs/soundboard.md): its sounds, each previewed,
 * renamed, given an emoji and a volume in place, or deleted; and adding
 * one, from any audio file the browser reads, cut to at most 10 seconds
 * here and made into Ogg Opus on this device before it's uploaded.
 */
export function Soundboard({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const list = useSounds(instanceKey, serverId);
  const members = inst?.members[serverId];
  const emojis = inst?.emojis[serverId];
  const sounds = list?.sounds ?? [];
  const [draft, setDraft] = useState<Draft | null>(null);
  const [dragging, setDragging] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const shake = useAnimationControls();
  const busy = !!draft && draft.stage !== "ready";

  async function pick(file: File | undefined) {
    if (!file || busy) return;
    if (!canMakeSounds()) return void toast(t("serversettings.soundboard.cantEncode"));
    const name = freeName(soundNameFromFile(file.name) || t("serversettings.soundboard.newSound"), sounds.map((s) => s.name));
    setDraft({ file, buffer: null, startMs: 0, name, emoji: "", volume: 1, stage: "reading", sent: 0 });
    try {
      const buffer = await decodeSoundFile(file);
      setDraft((d) => (d?.file === file ? { ...d, buffer, stage: "ready" } : d));
    } catch (err) {
      setDraft((d) => (d?.file === file ? null : d));
      toast((err as Error).message);
      void shake.start({ x: [0, -8, 7, -5, 3, 0], transition: { duration: 0.4 } });
    }
  }

  async function add() {
    const d = draft;
    if (!d?.buffer || d.stage !== "ready") return;
    if (!validName(d.name)) return void toast(t("serversettings.soundboard.badName"));
    const total = lengthMs(d.buffer);
    const { startMs, endMs } = clampWindow(total, d.startMs);
    const file = d.file;
    const stage = (patch: Partial<Draft>) => setDraft((x) => (x?.file === file ? { ...x, ...patch } : x));
    stage({ stage: "making", sent: 0 });
    try {
      const { ogg, durationMs } = await makeSound(d.buffer, startMs, endMs);
      if (durationMs > MAX_SOUND_MS) throw new Error(t("serversettings.soundboard.tooLong"));
      stage({ stage: "uploading" });
      const url = await run(uploadSound(instanceKey, serverId, new Blob([ogg], { type: "audio/ogg" }), (sent) => stage({ sent: sent * 0.9 })));
      await createSound(instanceKey, { serverId, name: d.name.trim(), url, emoji: soundEmoji(d.emoji), volume: d.volume, durationMs });
      stage({ sent: 1 });
      setDraft((x) => (x?.file === file ? null : x));
    } catch (err) {
      const e = err as FuwaError;
      toast(e.code === Code.ResourceExhausted ? t("serversettings.soundboard.full") : e.message || t("serversettings.soundboard.uploadFailed"));
      stage({ stage: "ready", sent: 0 });
    }
  }

  const drop = {
    onDragOver: (e: DragEvent) => {
      if (!e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      setDragging(true);
    },
    onDragLeave: (e: DragEvent) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      setDragging(false);
      void pick(e.dataTransfer.files[0]);
    },
  };

  return (
    <div className="flex flex-col gap-5">
      <AnimatePresence mode="popLayout" initial={false}>
        {draft ? (
          <DraftCard
            key="draft"
            draft={draft}
            emojis={emojis}
            serverName={inst?.servers.find((s) => s.id === serverId)?.name ?? ""}
            serverIcon={inst?.servers.find((s) => s.id === serverId)?.iconUrl ?? ""}
            serverId={serverId}
            onChange={(patch) => setDraft((d) => (d ? { ...d, ...patch } : d))}
            onCancel={() => setDraft(null)}
            onAdd={() => void add()}
          />
        ) : (
          <motion.button
            key="drop"
            type="button"
            animate={shake}
            initial={{ opacity: 0, y: -8 }}
            exit={{ opacity: 0, y: -8 }}
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.98 }}
            onClick={() => input.current?.click()}
            {...drop}
            className={cn(
              "group relative flex flex-col items-center gap-2 overflow-hidden rounded-3xl border-2 border-dashed p-6 text-center transition-colors",
              dragging ? "border-primary bg-primary/10" : "border-border hover:border-primary/50 hover:bg-primary/5",
            )}
          >
            <motion.span
              animate={dragging ? { scale: 1.2, rotate: -12, y: -4 } : { scale: 1, rotate: 0, y: 0, opacity: 1 }}
              transition={{ type: "spring", stiffness: 500, damping: 14 }}
              className="grid size-12 place-items-center rounded-2xl bg-primary/15 text-primary"
            >
              <AudioLinesIcon className="size-6 transition group-hover:scale-110" />
            </motion.span>
            <span className="font-extrabold">{dragging ? t("serversettings.soundboard.letGo") : t("serversettings.soundboard.drop")}</span>
            <span className="max-w-sm text-xs text-muted-foreground">{t("serversettings.soundboard.dropHint")}</span>
            <span className="mt-1 flex items-center gap-1 rounded-full bg-muted px-2.5 py-0.5 text-xs font-bold tabular-nums">
              <T k="serversettings.soundboard.count" values={{ count: <Count value={sounds.length} /> }} count={sounds.length} />
            </span>
          </motion.button>
        )}
      </AnimatePresence>
      <input
        ref={input}
        type="file"
        accept="audio/*,.ogg,.opus,.mp3,.wav,.flac,.m4a,.aac,.webm"
        className="hidden"
        onChange={(e) => {
          void pick(e.target.files?.[0]);
          e.target.value = "";
        }}
      />

      {!list || (list.status === "loading" && !sounds.length) ? (
        <div className="grid gap-2">
          {[0, 1, 2].map((n) => (
            <div key={n} className="shimmer h-16 rounded-2xl" />
          ))}
        </div>
      ) : (
        <div>
          {sounds.length === 0 && (
            <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="flex flex-col items-center gap-2 py-8 text-center">
              <motion.span animate={{ rotate: [0, -10, 10, -6, 0], y: [0, -4, 0] }} transition={{ duration: 2.4, repeat: Infinity, repeatDelay: 1.2 }} className="text-4xl">
                📯
              </motion.span>
              <p className="font-bold">{t("serversettings.soundboard.none")}</p>
              <p className="text-sm text-muted-foreground">{t("serversettings.soundboard.noneHint")}</p>
            </motion.div>
          )}
          <ul className="flex flex-col gap-2">
            <AnimatePresence initial={false}>
              {sounds.map((sound) => (
                <SoundRow
                  key={sound.id}
                  instanceKey={instanceKey}
                  serverId={serverId}
                  sound={sound}
                  taken={sounds.filter((s) => s.id !== sound.id).map((s) => s.name)}
                  creator={members?.find((m) => m.user?.id === sound.creatorId)?.user ?? inst?.users[sound.creatorId]}
                />
              ))}
            </AnimatePresence>
          </ul>
        </div>
      )}
    </div>
  );
}

/** The loudest moment in each of `bars` stretches of a file, 0 to 1. */
function peaksOf(buffer: AudioBuffer, bars: number): number[] {
  const data = buffer.getChannelData(0);
  const size = Math.max(1, Math.floor(data.length / bars));
  const peaks: number[] = [];
  for (let b = 0; b < bars; b++) {
    let peak = 0;
    const end = Math.min(data.length, (b + 1) * size);
    // Every fourth sample is plenty to see the shape.
    for (let i = b * size; i < end; i += 4) peak = Math.max(peak, Math.abs(data[i]!));
    peaks.push(peak);
  }
  const top = Math.max(0.01, ...peaks);
  return peaks.map((p) => p / top);
}

/** The file picked: its waveform with the part kept, and the sound's name, emoji and volume. */
function DraftCard({
  draft,
  emojis,
  serverId,
  serverName,
  serverIcon,
  onChange,
  onCancel,
  onAdd,
}: {
  draft: Draft;
  emojis: Parameters<typeof SoundEmoji>[0]["emojis"];
  serverId: string;
  serverName: string;
  serverIcon: string;
  onChange: (patch: Partial<Draft>) => void;
  onCancel: () => void;
  onAdd: () => void;
}) {
  const { t, number } = useI18n();
  const seconds = useSeconds();
  const [hearing, setHearing] = useState(false);
  const stop = useRef<(() => void) | null>(null);
  useEffect(() => () => stop.current?.(), []);
  const buffer = draft.buffer;
  const total = buffer ? lengthMs(buffer) : 0;
  const { startMs, endMs } = clampWindow(total, draft.startMs);
  const peaks = useMemo(() => (buffer ? peaksOf(buffer, BARS) : []), [buffer]);
  const working = draft.stage === "making" || draft.stage === "uploading";
  const server = useMemo(() => ({ id: serverId, name: serverName, iconUrl: serverIcon }), [serverId, serverName, serverIcon]);

  const hear = () => {
    stop.current?.();
    stop.current = null;
    if (hearing || !buffer) return setHearing(false);
    setHearing(true);
    stop.current = playLoose(buffer, draft.volume, () => setHearing(false), startMs / 1000, (endMs - startMs) / 1000);
  };

  // Moving the window or the volume stops what's playing, so what's heard is what's kept.
  useEffect(() => {
    stop.current?.();
    stop.current = null;
    setHearing(false);
  }, [startMs, draft.volume]);

  return (
    <motion.div
      layout
      initial={{ opacity: 0, scale: 0.96, y: -8 }}
      animate={{ opacity: 1, scale: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.96, y: -8 }}
      transition={SPRING}
      className="relative flex flex-col gap-4 overflow-hidden rounded-3xl border bg-card/60 p-4"
    >
      {draft.stage === "uploading" && (
        <motion.span className="absolute inset-x-0 top-0 h-1 origin-left bg-primary" initial={{ scaleX: 0 }} animate={{ scaleX: draft.sent }} transition={{ ease: "easeOut" }} />
      )}
      <div className="flex items-center gap-2">
        <AudioLinesIcon className="size-4 shrink-0 text-primary" />
        <span className="min-w-0 flex-1 truncate text-sm font-bold">{draft.file.name}</span>
        {buffer && <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{t("serversettings.soundboard.length", { length: seconds(total) })}</span>}
      </div>

      {!buffer ? (
        <div className="flex h-20 items-center justify-center gap-2 rounded-2xl bg-muted/50 text-sm text-muted-foreground">
          <LoaderCircleIcon className="size-4 animate-spin" /> {t("serversettings.soundboard.reading", { file: draft.file.name })}
        </div>
      ) : (
        <div className="flex flex-col gap-1">
          <div className="flex items-baseline justify-between gap-2">
            <span className="text-sm font-bold">{t("serversettings.soundboard.window")}</span>
            <span className="text-xs text-muted-foreground tabular-nums">{t("serversettings.soundboard.windowRange", { start: seconds(startMs), end: seconds(endMs) })}</span>
          </div>
          <div className="flex items-center gap-2">
            <motion.button
              type="button"
              onClick={hear}
              whileTap={{ scale: 0.9 }}
              aria-pressed={hearing}
              aria-label={hearing ? t("serversettings.soundboard.stop") : t("serversettings.soundboard.previewCut")}
              title={hearing ? t("serversettings.soundboard.stop") : t("serversettings.soundboard.previewCut")}
              className="grid size-10 shrink-0 place-items-center rounded-full bg-primary text-primary-foreground shadow-sm transition hover:brightness-110"
            >
              <motion.span key={String(hearing)} initial={{ scale: 0.4 }} animate={{ scale: 1 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="grid place-items-center">
                {hearing ? <SquareIcon className="size-4 fill-current" /> : <PlayIcon className="size-4 fill-current" />}
              </motion.span>
            </motion.button>
            <div className="relative flex h-14 min-w-0 flex-1 items-center gap-px overflow-hidden rounded-xl bg-muted/50 px-1" aria-hidden>
              {peaks.map((p, i) => {
                const at = ((i + 0.5) / peaks.length) * total;
                const kept = at >= startMs && at <= endMs;
                return (
                  <motion.span
                    key={i}
                    initial={{ scaleY: 0 }}
                    animate={{ scaleY: 1 }}
                    transition={{ ...SPRING, delay: Math.min(i, 60) * 0.004 }}
                    className={cn("min-w-0 flex-1 rounded-full transition-colors duration-200", kept ? "bg-primary" : "bg-muted-foreground/30")}
                    style={{ height: `${Math.max(6, p * 100)}%` }}
                  />
                );
              })}
              <motion.span
                className="pointer-events-none absolute inset-y-1 rounded-lg border-2 border-primary/70"
                animate={{ left: `${total ? (startMs / total) * 100 : 0}%`, width: `${total ? ((endMs - startMs) / total) * 100 : 100}%` }}
                transition={SPRING}
              />
            </div>
          </div>
          {total > MAX_SOUND_MS && (
            <Slider
              label={t("serversettings.soundboard.start")}
              value={startMs}
              min={0}
              max={Math.max(0, total - MAX_SOUND_MS)}
              step={50}
              format={seconds}
              onChange={(v) => onChange({ startMs: v })}
            />
          )}
          <p className="text-xs text-muted-foreground">{t("serversettings.soundboard.windowHint")}</p>
        </div>
      )}

      <div className="flex flex-col gap-3 sm:flex-row sm:items-end">
        <label className="flex min-w-0 flex-1 flex-col gap-1.5">
          <span className="text-sm font-bold">{t("serversettings.soundboard.name")}</span>
          <div className="flex items-center gap-2">
            <EmojiPicker emojis={emojis} server={server} placement="bottom-start" onPick={(e) => onChange({ emoji: soundEmoji(e.text) })}>
              {(open) => (
                <motion.button
                  type="button"
                  whileHover={{ scale: 1.08, rotate: -6 }}
                  whileTap={{ scale: 0.9 }}
                  aria-label={t("serversettings.soundboard.pickEmoji")}
                  title={t("serversettings.soundboard.pickEmoji")}
                  className={cn("grid size-10 shrink-0 place-items-center rounded-xl border text-xl transition-colors", open ? "border-primary/60 bg-primary/10" : "hover:border-primary/40")}
                >
                  {draft.emoji ? <SoundEmoji emoji={draft.emoji} emojis={emojis} className="size-6" /> : <SmilePlusIcon className="size-4 text-muted-foreground" />}
                </motion.button>
              )}
            </EmojiPicker>
            <AnimatePresence>
              {draft.emoji && (
                <motion.button
                  type="button"
                  initial={{ opacity: 0, scale: 0.5 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.5 }}
                  transition={SPRING}
                  onClick={() => onChange({ emoji: "" })}
                  aria-label={t("serversettings.soundboard.clearEmoji")}
                  title={t("serversettings.soundboard.clearEmoji")}
                  className="-ml-1 grid size-6 shrink-0 place-items-center rounded-full text-muted-foreground transition hover:bg-muted hover:text-foreground"
                >
                  <XIcon className="size-3.5" />
                </motion.button>
              )}
            </AnimatePresence>
            <Input
              value={draft.name}
              maxLength={MAX_SOUND_NAME}
              placeholder={t("serversettings.soundboard.namePlaceholder")}
              aria-label={t("serversettings.soundboard.name")}
              onChange={(e) => onChange({ name: e.target.value })}
              onKeyDown={(e) => e.key === "Enter" && onAdd()}
              className={cn("h-10 min-w-0 flex-1 rounded-xl font-bold", !validName(draft.name) && "border-destructive/60")}
            />
          </div>
        </label>
        <div className="sm:w-44">
          <div className="-mb-5 flex items-baseline justify-between text-sm">
            <span className="font-bold">{t("serversettings.soundboard.volume")}</span>
            <span className="text-xs font-bold text-muted-foreground tabular-nums">{number(draft.volume, { style: "percent" })}</span>
          </div>
          <Slider
            label={t("serversettings.soundboard.volume")}
            value={Math.round(draft.volume * 100)}
            min={0}
            max={100}
            step={5}
            format={(n) => number(n / 100, { style: "percent" })}
            onChange={(n) => onChange({ volume: n / 100 })}
          />
        </div>
      </div>

      <div className="flex items-center justify-end gap-2">
        <AnimatePresence mode="popLayout">
          {working && (
            <motion.span key={draft.stage} initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING} className="mr-auto flex items-center gap-1.5 text-xs text-muted-foreground">
              <LoaderCircleIcon className="size-3.5 animate-spin" />
              {draft.stage === "making" ? t("serversettings.soundboard.encoding") : t("serversettings.soundboard.uploading")}
            </motion.span>
          )}
        </AnimatePresence>
        <Button type="button" variant="ghost" className="rounded-full" disabled={working} onClick={onCancel}>
          {t("serversettings.soundboard.cancel")}
        </Button>
        <Button type="button" className="btn rounded-full font-bold" disabled={!buffer || working || !validName(draft.name)} onClick={onAdd}>
          {working ? <LoaderCircleIcon className="animate-spin" /> : <AudioLinesIcon />} {t("serversettings.soundboard.upload")}
        </Button>
      </div>
    </motion.div>
  );
}

/** One sound: preview, its emoji, name and volume changed in place, who added it and how long it is, and delete. */
function SoundRow({
  instanceKey,
  serverId,
  sound,
  taken,
  creator,
}: {
  instanceKey: string;
  serverId: string;
  sound: Sound;
  taken: string[];
  creator: Parameters<typeof UserAvatar>[0]["user"];
}) {
  const { t, number } = useI18n();
  const seconds = useSeconds();
  const inst = useInstance(instanceKey);
  const emojis = inst?.emojis[serverId];
  const serverRef = useMemo(() => {
    const s = inst?.servers.find((x) => x.id === serverId);
    return { id: serverId, name: s?.name ?? "", iconUrl: s?.iconUrl ?? "" };
  }, [inst?.servers, serverId]);
  const [name, setName] = useState(sound.name);
  const [volume, setVolume] = useState(Math.round(sound.volume * 100));
  const [saving, setSaving] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [playing, setPlaying] = useState(false);
  const stop = useRef<(() => void) | null>(null);
  const shake = useAnimationControls();
  useEffect(() => setName(sound.name), [sound.name]);
  useEffect(() => setVolume(Math.round(sound.volume * 100)), [sound.volume]);
  useEffect(() => () => stop.current?.(), []);
  const clash = taken.some((n) => n.toLowerCase() === name.trim().toLowerCase());

  async function save(change: { name?: string; emoji?: string; volume?: number }) {
    setSaving(true);
    try {
      await updateSound(instanceKey, serverId, sound.id, change);
    } catch (err) {
      toast((err as FuwaError).message);
      setName(sound.name);
      setVolume(Math.round(sound.volume * 100));
    } finally {
      setSaving(false);
    }
  }

  function rename() {
    const next = name.trim();
    if (next === sound.name) return setName(sound.name);
    if (!validName(next) || clash) {
      void shake.start({ x: [0, -6, 5, -3, 0], transition: { duration: 0.35 } });
      toast(t("serversettings.soundboard.badName"));
      return setName(sound.name);
    }
    void save({ name: next });
  }

  async function preview() {
    if (playing) {
      stop.current?.();
      stop.current = null;
      return setPlaying(false);
    }
    setPlaying(true);
    try {
      stop.current = await previewSound({ ...sound, volume: volume / 100 }, () => setPlaying(false));
    } catch (err) {
      setPlaying(false);
      toast((err as Error).message);
    }
  }

  async function remove() {
    await deleteSound(instanceKey, serverId, sound.id).catch((err: FuwaError) => toast(err.message));
  }

  return (
    <motion.li
      layout
      initial={{ opacity: 0, scale: 0.94 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.85, filter: "blur(4px)" }}
      transition={SPRING}
      className="group flex flex-wrap items-center gap-3 rounded-2xl border bg-background/50 p-3 transition-colors hover:border-primary/30"
    >
      <motion.button
        type="button"
        onClick={() => void preview()}
        whileTap={{ scale: 0.88 }}
        aria-pressed={playing}
        aria-label={t("serversettings.soundboard.preview", { name: sound.name })}
        title={t("serversettings.soundboard.preview", { name: sound.name })}
        className={cn("grid size-9 shrink-0 place-items-center rounded-full transition", playing ? "bg-primary text-primary-foreground" : "bg-muted text-foreground hover:bg-primary/15 hover:text-primary")}
      >
        <motion.span key={String(playing)} initial={{ scale: 0.4 }} animate={{ scale: 1 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="grid place-items-center">
          {playing ? <SquareIcon className="size-3.5 fill-current" /> : <PlayIcon className="size-3.5 fill-current" />}
        </motion.span>
      </motion.button>
      <EmojiPicker emojis={emojis} server={serverRef} placement="bottom-start" onPick={(e) => void save({ emoji: soundEmoji(e.text) })}>
        {(open) => (
          <motion.button
            type="button"
            whileHover={{ scale: 1.12, rotate: -8 }}
            whileTap={{ scale: 0.9 }}
            aria-label={t("serversettings.soundboard.pickEmoji")}
            title={t("serversettings.soundboard.pickEmoji")}
            className={cn("grid size-9 shrink-0 place-items-center rounded-xl text-xl transition-colors", open ? "bg-primary/10" : "hover:bg-muted")}
          >
            <motion.span key={sound.emoji} initial={{ scale: 0.4, rotate: -30 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="grid place-items-center">
              <SoundEmoji emoji={sound.emoji} emojis={emojis} className="size-6" />
            </motion.span>
          </motion.button>
        )}
      </EmojiPicker>
      <span className="min-w-0 flex-1 basis-40">
        <motion.span animate={shake} className="flex items-center font-bold">
          <Input
            value={name}
            aria-label={t("serversettings.soundboard.nameOf", { name: sound.name })}
            maxLength={MAX_SOUND_NAME}
            onChange={(e) => setName(e.target.value)}
            onBlur={rename}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") setName(sound.name);
            }}
            className={cn(
              "h-7 w-auto max-w-full min-w-[3ch] rounded-md border-transparent bg-transparent px-0.5 font-bold shadow-none [field-sizing:content] focus-visible:border-input",
              (!validName(name) || clash) && "text-destructive",
            )}
          />
          {saving && <LoaderCircleIcon className="ml-1 size-3.5 shrink-0 animate-spin text-muted-foreground" />}
          {sound.emoji && (
            <button
              type="button"
              onClick={() => void save({ emoji: "" })}
              aria-label={t("serversettings.soundboard.clearEmoji")}
              title={t("serversettings.soundboard.clearEmoji")}
              className="ml-1 grid size-5 shrink-0 place-items-center rounded-full text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:bg-muted hover:text-foreground focus-visible:opacity-100"
            >
              <XIcon className="size-3" />
            </button>
          )}
        </motion.span>
        <span className="mt-0.5 flex items-center gap-1.5 truncate text-xs text-muted-foreground">
          <UserAvatar user={creator} className="size-4" />
          <span className="truncate">
            {creator ? displayName(creator) : t("common.someone")} · {seconds(sound.durationMs)}
          </span>
        </span>
      </span>
      <div className="w-32 shrink-0">
        <Slider
          label={t("serversettings.soundboard.volumeOf", { name: sound.name })}
          value={volume}
          min={0}
          max={100}
          step={5}
          format={(n) => number(n / 100, { style: "percent" })}
          onChange={setVolume}
          onCommit={(n) => n !== Math.round(sound.volume * 100) && void save({ volume: n / 100 })}
        />
      </div>
      <AnimatePresence mode="popLayout" initial={false}>
        {confirm ? (
          <motion.span key="sure" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={SPRING} className="flex items-center gap-1">
            <span className="hidden text-xs font-bold text-destructive sm:inline">{t("serversettings.soundboard.deleteQuestion")}</span>
            <Button type="button" size="sm" variant="destructive" className="h-8 rounded-full px-3 text-xs font-bold" onClick={() => void remove()}>
              {t("serversettings.shared.delete")}
            </Button>
            <Button type="button" size="icon" variant="ghost" aria-label={t("serversettings.shared.keepIt")} className="size-8 rounded-full" onClick={() => setConfirm(false)}>
              <XIcon />
            </Button>
          </motion.span>
        ) : (
          <motion.span key="ask" initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING}>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              aria-label={t("serversettings.soundboard.deleteNamed", { name: sound.name })}
              className="size-8 rounded-full text-muted-foreground opacity-60 transition hover:text-destructive group-hover:opacity-100"
              onClick={() => setConfirm(true)}
            >
              <Trash2Icon />
            </Button>
          </motion.span>
        )}
      </AnimatePresence>
    </motion.li>
  );
}
