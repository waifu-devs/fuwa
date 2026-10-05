import { AudioLinesIcon, DownloadIcon, FileArchiveIcon, HourglassIcon, LoaderCircleIcon, MonitorIcon, ServerIcon, Trash2Icon, VideoIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Permission, type Channel, type VoiceState } from "@/gen/fuwa/v1/types_pb";
import { RecordingPart, type Recording, type RecordingTrack } from "@/gen/fuwa/v1/call_pb";
import { useAccess } from "@/fuwa/hooks";
import { toFuwaError } from "@/fuwa/errors";
import { useFuwa, type FuwaState } from "@/fuwa/store";
import { engine } from "@/fuwa/sync";
import { SPRING } from "@/components/motion";
import { UserAvatar } from "@/components/Icons";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { displayName, formatBytes, memberName, toDate } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { useNow } from "@/lib/notifications";
import { hasIn } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { saveFile, zip } from "@/lib/zip";

/**
 * A voice channel's recordings on the server: one track per person, each
 * an Ogg Opus file (and, when the server records video, their camera and
 * shared screen as WebM), for the people with Record there. Downloads come
 * as they are (a file each, or all of them in a .zip), lined up from the
 * recording's start so they drop straight into an editor.
 */

/** Opens the channel's recordings. Only for people with Record there. */
export function RecordingsButton({ instanceKey, serverId, channel, states }: { instanceKey: string; serverId: string; channel: Channel; states: VoiceState[] }) {
  const access = useAccess(instanceKey, serverId);
  const [open, setOpen] = useState(false);
  const live = states.some((s) => s.serverRecord);
  if (!hasIn(access, channel.id, Permission.RECORD)) return null;
  return (
    <>
      <motion.button
        type="button"
        whileTap={{ scale: 0.88 }}
        onClick={() => setOpen(true)}
        aria-label="Recordings"
        title="Recordings on the server"
        className="group relative grid size-9 shrink-0 place-items-center rounded-full text-muted-foreground transition hover:bg-muted hover:text-foreground"
      >
        <AudioLinesIcon className="size-[18px] transition-transform group-hover:scale-110" />
        <AnimatePresence>
          {live && (
            <motion.span initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={SPRING} className="absolute top-1.5 right-1.5 grid size-2 place-items-center">
              <span className="absolute inset-0 animate-ping rounded-full bg-[#ed4245]/60" />
              <span className="size-2 rounded-full bg-[#ed4245] ring-2 ring-background" />
            </motion.span>
          )}
        </AnimatePresence>
      </motion.button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent wide>
          <DialogHeader
            title={
              <span className="flex items-center gap-2">
                <ServerIcon className="size-5 text-primary" /> Recordings
              </span>
            }
            description={`Kept on the server for people who can record in ${channel.name}. One track per person, lined up from the start.`}
          />
          {open && <RecordingList instanceKey={instanceKey} serverId={serverId} channel={channel} live={live} />}
        </DialogContent>
      </Dialog>
    </>
  );
}

function RecordingList({ instanceKey, serverId, channel, live }: { instanceKey: string; serverId: string; channel: Channel; live: boolean }) {
  const [recordings, setRecordings] = useState<Recording[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [usage, setUsage] = useState<Usage | null>(null);
  const load = useCallback(async () => {
    try {
      const res = await engine(instanceKey).api.calls.listRecordings({ serverId, channelId: channel.id });
      setRecordings(res.recordings);
      setUsage({ used: Number(res.usedBytes), cap: res.capBytes === undefined ? null : Number(res.capBytes), keepDays: res.keepDays === undefined ? null : Number(res.keepDays) });
      setProblem(null);
    } catch (err) {
      setProblem(toFuwaError(err).message);
    }
  }, [instanceKey, serverId, channel.id]);
  const going = live || recordings?.some((r) => !r.endedAt);
  useEffect(() => {
    void load();
    // While one is going on, its tracks grow.
    if (!going) return;
    const id = setInterval(() => void load(), 3_000);
    return () => clearInterval(id);
  }, [load, going]);

  if (problem && !recordings) return <p className="text-sm font-bold text-destructive first-letter:uppercase">{problem}</p>;
  if (!recordings)
    return (
      <div className="grid place-items-center py-10 text-muted-foreground">
        <LoaderCircleIcon className="size-6 animate-spin" />
      </div>
    );
  if (!recordings.length)
    return (
      <div className="flex flex-col gap-3">
        {usage && <UsageStrip usage={usage} />}
        <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-2 rounded-3xl border border-dashed px-6 py-10 text-center">
          <motion.span animate={{ y: [0, -4, 0] }} transition={{ duration: 2.4, repeat: Infinity, ease: "easeInOut" }} className="grid size-12 place-items-center rounded-2xl bg-primary/10 text-primary">
            <AudioLinesIcon className="size-6" />
          </motion.span>
          <p className="font-bold">No recordings yet</p>
          <p className="max-w-xs text-sm text-muted-foreground">In the call, press Record and pick "On the server". Everyone in the channel sees it, and hears a beep when it starts.</p>
        </motion.div>
      </div>
    );
  return (
    <div className="flex flex-col gap-3">
      {usage && <UsageStrip usage={usage} />}
      <ul className="flex flex-col gap-3">
        <AnimatePresence initial={false} mode="popLayout">
          {recordings.map((rec, n) => (
            <RecordingCard
              key={rec.id}
              instanceKey={instanceKey}
              serverId={serverId}
              channel={channel}
              rec={rec}
              index={n}
              onDeleted={() => {
                setRecordings((list) => list?.filter((r) => r.id !== rec.id) ?? null);
                void load();
              }}
            />
          ))}
        </AnimatePresence>
      </ul>
    </div>
  );
}

type Usage = { used: number; cap: number | null; keepDays: number | null };

/**
 * How much the server's recordings (every channel's) take, against its cap
 * when it has one, and how long they're kept. The bar fills as it grows,
 * and turns red once it's full, when no recording can go on.
 */
function UsageStrip({ usage: { used, cap, keepDays } }: { usage: Usage }) {
  const lang = useI18n();
  if (cap === null && keepDays === null) return null;
  const share = cap ? Math.min(1, used / cap) : 0;
  const full = cap !== null && used >= cap;
  return (
    <motion.div initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col gap-2 rounded-2xl border bg-muted/40 px-3.5 py-3">
      {cap !== null && (
        <>
          <div className="flex items-baseline justify-between gap-3 text-sm">
            <span className={cn("font-bold", full && "text-destructive")}>{full ? "Full: delete some to record again" : "This server's recordings"}</span>
            <span className="shrink-0 text-xs text-muted-foreground tabular-nums">
              {formatBytes(lang, used)} of {formatBytes(lang, cap)}
            </span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-muted">
            <motion.div
              className={cn("h-full origin-left rounded-full", full ? "bg-destructive" : share > 0.8 ? "bg-amber-500" : "bg-primary")}
              initial={{ scaleX: 0 }}
              animate={{ scaleX: share }}
              transition={{ type: "spring", stiffness: 120, damping: 20 }}
            />
          </div>
        </>
      )}
      {keepDays !== null && (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <HourglassIcon className="size-3.5 shrink-0" />
          Each recording deletes itself {keepDays === 1 ? "a day" : `${keepDays} days`} after it ends.
        </p>
      )}
    </motion.div>
  );
}

const pad = (n: number) => String(n).padStart(2, "0");

function clock(ms: number) {
  const s = Math.max(0, Math.round(ms / 1000));
  const h = Math.floor(s / 3600);
  return h ? `${h}:${pad(Math.floor(s / 60) % 60)}:${pad(s % 60)}` : `${Math.floor(s / 60)}:${pad(s % 60)}`;
}

/** "Booth 2026-10-03 09.41": what files from a recording are named after. */
function baseName(channel: Channel, rec: Recording) {
  const d = toDate(rec.startedAt);
  return `${channel.name} ${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}.${pad(d.getMinutes())}`;
}

/** Characters no file system takes in a name. */
const safe = (name: string) => name.replace(/[\\/:*?"<>|]+/g, "-").trim() || "someone";

/** One of a person's files in a recording. */
type FilePart = { part: RecordingPart; bytes: (t: RecordingTrack) => bigint; suffix: string; type: string; what: string; icon: typeof VideoIcon };

const PARTS: FilePart[] = [
  { part: RecordingPart.UNSPECIFIED, bytes: (t) => t.sizeBytes, suffix: ".opus", type: "audio/ogg", what: "sound (Ogg Opus)", icon: DownloadIcon },
  { part: RecordingPart.CAMERA, bytes: (t) => t.cameraBytes, suffix: " camera.webm", type: "video/webm", what: "camera (WebM)", icon: VideoIcon },
  { part: RecordingPart.SCREEN, bytes: (t) => t.screenBytes, suffix: " screen.webm", type: "video/webm", what: "shared screen (WebM)", icon: MonitorIcon },
];

/** The files a person has in a recording. */
const filesOf = (track: RecordingTrack) => PARTS.filter((p) => p.bytes(track) > 0n);
const fileKey = (track: RecordingTrack, part: FilePart) => `${track.userId}:${part.part}`;

function RecordingCard({ instanceKey, serverId, channel, rec, index, onDeleted }: { instanceKey: string; serverId: string; channel: Channel; rec: Recording; index: number; onDeleted: () => void }) {
  const lang = useI18n();
  const live = !rec.endedAt;
  const now = useNow(live ? 1_000 : 60_000);
  const started = toDate(rec.startedAt);
  const length = live ? now - started.getTime() : Math.max(...rec.tracks.map((t) => Number(t.durationMs)), toDate(rec.endedAt).getTime() - started.getTime());
  const starter = useName(instanceKey, serverId, rec.startedBy);
  const access = useAccess(instanceKey, serverId);
  const mine = useFuwa((s) => s.instances[instanceKey]?.me?.id === rec.startedBy);
  // Whoever started it, or someone who runs the channel.
  const mayDelete = mine || hasIn(access, channel.id, Permission.MANAGE_CHANNELS);
  // Joined into one string, so the store only wakes this card when a name changes.
  const joinedNames = useFuwa((s) => rec.tracks.map((t) => nameIn(s, instanceKey, serverId, t.userId)).join("\u0000"));
  const names = useMemo(() => {
    const list = joinedNames.split("\u0000");
    return Object.fromEntries(rec.tracks.map((t, i) => [t.userId, list[i] ?? t.userId]));
  }, [joinedNames, rec.tracks]);
  const [progress, setProgress] = useState<Record<string, number>>({});
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const busy = Object.keys(progress).length > 0;

  /** One of a person's files, plain: Ogg Opus, or WebM. */
  const fetchTrack = async (track: RecordingTrack, part: FilePart) => {
    const parts: Uint8Array<ArrayBuffer>[] = [];
    let got = 0;
    const key = fileKey(track, part);
    for await (const res of engine(instanceKey).api.calls.downloadRecording({ serverId, recordingId: rec.id, userId: track.userId, part: part.part })) {
      parts.push(new Uint8Array(res.data));
      got += res.data.length;
      setProgress((p) => ({ ...p, [key]: Math.min(0.98, got / Math.max(1, Number(part.bytes(track)))) }));
    }
    const data = new Uint8Array(got);
    let at = 0;
    for (const part of parts) {
      data.set(part, at);
      at += part.length;
    }
    return data;
  };
  const fileName = (track: RecordingTrack, part: FilePart) => `${baseName(channel, rec)} - ${safe(names[track.userId] ?? track.userId)}${part.suffix}`;
  const done = (keys: string[]) => setProgress((p) => Object.fromEntries(Object.entries(p).filter(([key]) => !keys.includes(key))));
  const everything = rec.tracks.flatMap((track) => filesOf(track).map((part) => ({ track, part })));

  const downloadOne = async (track: RecordingTrack, part: FilePart) => {
    const key = fileKey(track, part);
    setProgress((p) => ({ ...p, [key]: 0 }));
    try {
      saveFile(new Blob([await fetchTrack(track, part)], { type: part.type }), fileName(track, part));
    } catch (err) {
      toast(`Couldn't download it: ${toFuwaError(err).message}`);
    } finally {
      done([key]);
    }
  };
  const downloadAll = async () => {
    setProgress(Object.fromEntries(everything.map(({ track, part }) => [fileKey(track, part), 0])));
    try {
      const files = [];
      for (const { track, part } of everything) files.push({ name: fileName(track, part), data: await fetchTrack(track, part) });
      saveFile(zip(files, started), `${safe(baseName(channel, rec))}.zip`);
    } catch (err) {
      toast(`Couldn't download it: ${toFuwaError(err).message}`);
    } finally {
      done(everything.map(({ track, part }) => fileKey(track, part)));
    }
  };
  const remove = async () => {
    setDeleting(true);
    try {
      await engine(instanceKey).api.calls.deleteRecording({ serverId, recordingId: rec.id });
      onDeleted();
      toast("Recording deleted.");
    } catch (err) {
      toast(`Couldn't delete it: ${toFuwaError(err).message}`);
      setDeleting(false);
    }
  };

  return (
    <motion.li
      layout
      initial={{ opacity: 0, y: 14, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { ...SPRING, delay: Math.min(index, 6) * 0.05 } }}
      exit={{ opacity: 0, x: -40, scale: 0.95, transition: { duration: 0.2 } }}
      className={cn("overflow-hidden rounded-3xl border bg-background/60", live && "border-[#ed4245]/40 shadow-[0_0_0_1px_rgb(237_66_69/0.15),0_12px_40px_-16px_rgb(237_66_69/0.5)]")}
    >
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 px-4 pt-3.5 pb-2">
        <Wave live={live} />
        <div className="min-w-40 flex-1">
          <p className="flex items-center gap-1.5 font-extrabold">
            <span className="truncate">{started.toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}</span>
            {rec.video && (
              <motion.span
                initial={{ opacity: 0, scale: 0.7 }}
                animate={{ opacity: 1, scale: 1 }}
                transition={SPRING}
                title="Cameras and shared screens are recorded too"
                className="flex shrink-0 items-center gap-1 rounded-full bg-primary/10 px-1.5 py-0.5 text-[0.7rem] font-bold text-primary"
              >
                <VideoIcon className="size-3.5" /> <span className="hidden sm:inline">With video</span>
              </motion.span>
            )}
          </p>
          <p className="truncate text-xs text-muted-foreground">
            {live ? (
              <span className="font-bold text-[#ed4245]">Recording now · {clock(length)}</span>
            ) : (
              <>
                {clock(length)} · {formatBytes(lang, Number(rec.sizeBytes))}
              </>
            )}{" "}
            · started by {starter}
          </p>
        </div>
        {!live && (
          <div className="ml-auto flex shrink-0 items-center gap-1">
            <Button size="sm" variant="secondary" className="group h-8 rounded-xl font-bold" disabled={busy || !everything.length} onClick={() => void downloadAll()}>
              <FileArchiveIcon className="transition-transform group-hover:-translate-y-0.5" /> <span className="hidden sm:inline">All</span> .zip
            </Button>
            <AnimatePresence mode="wait" initial={false}>
              {!mayDelete ? null : confirming ? (
                <motion.div key="sure" initial={{ opacity: 0, x: 10 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 10 }} transition={SPRING} className="flex items-center gap-1">
                  <Button size="sm" variant="destructive" className="h-8 rounded-xl font-bold" disabled={deleting} onClick={() => void remove()}>
                    {deleting ? "Deleting…" : "Delete"}
                  </Button>
                  <Button size="sm" variant="ghost" className="h-8 rounded-xl" disabled={deleting} onClick={() => setConfirming(false)}>
                    Keep
                  </Button>
                </motion.div>
              ) : (
                <motion.button
                  key="bin"
                  type="button"
                  initial={{ opacity: 0, scale: 0.6 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0, scale: 0.6 }}
                  whileTap={{ scale: 0.85 }}
                  onClick={() => setConfirming(true)}
                  aria-label="Delete this recording"
                  title="Delete this recording for everyone"
                  className="group grid size-8 place-items-center rounded-xl text-muted-foreground transition hover:bg-destructive/10 hover:text-destructive"
                >
                  <Trash2Icon className="size-4 transition-transform group-hover:-rotate-12" />
                </motion.button>
              )}
            </AnimatePresence>
          </div>
        )}
      </div>
      <ul className="flex flex-col px-2 pb-2">
        {rec.tracks.map((track, n) => (
          <motion.li
            key={track.userId}
            initial={{ opacity: 0, x: -8 }}
            animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: 0.05 + n * 0.04 } }}
            className="relative flex items-center gap-2.5 overflow-hidden rounded-2xl px-2 py-1.5 transition-colors hover:bg-muted/60"
          >
            <AnimatePresence>
              {filesOf(track).some((part) => progress[fileKey(track, part)] !== undefined) && (
                <motion.span
                  aria-hidden
                  className="absolute inset-y-0 left-0 w-full origin-left bg-primary/12"
                  initial={{ scaleX: 0, opacity: 1 }}
                  animate={{ scaleX: Math.max(...filesOf(track).map((part) => progress[fileKey(track, part)] ?? 0)) }}
                  exit={{ scaleX: 1, opacity: 0, transition: { duration: 0.35 } }}
                  transition={{ type: "spring", stiffness: 200, damping: 30 }}
                />
              )}
            </AnimatePresence>
            <TrackAvatar instanceKey={instanceKey} userId={track.userId} />
            <span className="relative min-w-0 flex-1 truncate text-sm font-bold">{names[track.userId]}</span>
            <span className="relative shrink-0 text-xs text-muted-foreground tabular-nums">{formatBytes(lang, Number(track.sizeBytes + track.cameraBytes + track.screenBytes))}</span>
            {live
              ? filesOf(track)
                  .filter((part) => part.part !== RecordingPart.UNSPECIFIED)
                  .map((part) => (
                    <motion.span key={part.part} initial={{ scale: 0 }} animate={{ scale: 1 }} transition={SPRING} title={`Recording their ${part.what.split(" (")[0]}`} className="relative text-muted-foreground">
                      <part.icon className="size-3.5" />
                    </motion.span>
                  ))
              : filesOf(track).map((part) => {
                  const getting = progress[fileKey(track, part)] !== undefined;
                  return (
                    <motion.button
                      key={part.part}
                      type="button"
                      initial={{ opacity: 0, scale: 0.6 }}
                      animate={{ opacity: 1, scale: 1 }}
                      transition={SPRING}
                      whileTap={{ scale: 0.85 }}
                      disabled={getting}
                      onClick={() => void downloadOne(track, part)}
                      aria-label={`Download ${names[track.userId]}'s ${part.what}`}
                      title={`Download their ${part.what}`}
                      className="group relative grid size-8 shrink-0 place-items-center rounded-xl text-muted-foreground transition hover:bg-primary/10 hover:text-primary disabled:opacity-60"
                    >
                      {getting ? <LoaderCircleIcon className="size-4 animate-spin" /> : <part.icon className="size-4 transition-transform group-hover:translate-y-0.5" />}
                    </motion.button>
                  );
                })}
          </motion.li>
        ))}
        {!rec.tracks.length && <li className="px-2 py-1.5 text-sm text-muted-foreground">{live ? "Nobody has said anything yet." : "Nobody said anything."}</li>}
      </ul>
    </motion.li>
  );
}

/** Bars that dance while the recording is going on, and rest once it's done. */
function Wave({ live }: { live: boolean }) {
  return (
    <span className={cn("grid size-10 shrink-0 place-items-center rounded-2xl", live ? "bg-[#ed4245]/12 text-[#ed4245]" : "bg-primary/10 text-primary")}>
      <span className="flex h-4 items-center gap-[3px]">
        {[0.55, 1, 0.7, 0.4].map((h, i) => (
          <motion.span
            key={i}
            className="w-[3px] rounded-full bg-current"
            style={{ height: "100%", originY: 0.5 }}
            initial={false}
            animate={live ? { scaleY: [h, 0.25, 1, h] } : { scaleY: h }}
            transition={live ? { duration: 1.1, repeat: Infinity, ease: "easeInOut", delay: i * 0.13 } : SPRING}
          />
        ))}
      </span>
    </span>
  );
}

function TrackAvatar({ instanceKey, userId }: { instanceKey: string; userId: string }) {
  const user = useFuwa((s) => s.instances[instanceKey]?.users[userId]);
  return <UserAvatar user={user} className="relative size-7 shrink-0" />;
}


function nameIn(s: FuwaState, instanceKey: string, serverId: string, userId: string) {
  const member = s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === userId);
  return member ? memberName(member) : displayName(s.instances[instanceKey]?.users[userId]);
}

function useName(instanceKey: string, serverId: string, userId: string) {
  return useFuwa((s) => nameIn(s, instanceKey, serverId, userId));
}
