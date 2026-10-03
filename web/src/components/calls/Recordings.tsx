import { AudioLinesIcon, DownloadIcon, FileArchiveIcon, LoaderCircleIcon, ServerIcon, Trash2Icon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Permission, type Channel, type VoiceState } from "@/gen/fuwa/v1/types_pb";
import type { Recording, RecordingTrack } from "@/gen/fuwa/v1/call_pb";
import { useAccess } from "@/fuwa/hooks";
import { toFuwaError } from "@/fuwa/errors";
import { useFuwa, type FuwaState } from "@/fuwa/store";
import { engine } from "@/fuwa/sync";
import { SPRING } from "@/components/motion";
import { UserAvatar } from "@/components/Icons";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { displayName, formatBytes, memberName, toDate } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { hasIn } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { saveFile, zip } from "@/lib/zip";

/**
 * A voice channel's recordings on the server: one track per person, each
 * an Ogg Opus file, for the people with Record there. Downloads come as
 * they are (a track each, or all of them in a .zip), lined up from the
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
  const load = useCallback(async () => {
    try {
      const res = await engine(instanceKey).api.calls.listRecordings({ serverId, channelId: channel.id });
      setRecordings(res.recordings);
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
      <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-2 rounded-3xl border border-dashed px-6 py-10 text-center">
        <motion.span animate={{ y: [0, -4, 0] }} transition={{ duration: 2.4, repeat: Infinity, ease: "easeInOut" }} className="grid size-12 place-items-center rounded-2xl bg-primary/10 text-primary">
          <AudioLinesIcon className="size-6" />
        </motion.span>
        <p className="font-bold">No recordings yet</p>
        <p className="max-w-xs text-sm text-muted-foreground">In the call, press Record and pick "On the server". Everyone in the channel sees it, and hears a beep when it starts.</p>
      </motion.div>
    );
  return (
    <ul className="flex flex-col gap-3">
      <AnimatePresence initial={false} mode="popLayout">
        {recordings.map((rec, n) => (
          <RecordingCard key={rec.id} instanceKey={instanceKey} serverId={serverId} channel={channel} rec={rec} index={n} onDeleted={() => setRecordings((list) => list?.filter((r) => r.id !== rec.id) ?? null)} />
        ))}
      </AnimatePresence>
    </ul>
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

function RecordingCard({ instanceKey, serverId, channel, rec, index, onDeleted }: { instanceKey: string; serverId: string; channel: Channel; rec: Recording; index: number; onDeleted: () => void }) {
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

  /** One track's file, as plain Ogg Opus. */
  const fetchTrack = async (track: RecordingTrack) => {
    const parts: Uint8Array<ArrayBuffer>[] = [];
    let got = 0;
    for await (const res of engine(instanceKey).api.calls.downloadRecording({ serverId, recordingId: rec.id, userId: track.userId })) {
      parts.push(new Uint8Array(res.data));
      got += res.data.length;
      setProgress((p) => ({ ...p, [track.userId]: Math.min(0.98, got / Math.max(1, Number(track.sizeBytes))) }));
    }
    const data = new Uint8Array(got);
    let at = 0;
    for (const part of parts) {
      data.set(part, at);
      at += part.length;
    }
    return data;
  };
  const fileName = (track: RecordingTrack) => `${baseName(channel, rec)} - ${safe(names[track.userId] ?? track.userId)}.opus`;
  const done = (ids: string[]) => setProgress((p) => Object.fromEntries(Object.entries(p).filter(([id]) => !ids.includes(id))));

  const downloadOne = async (track: RecordingTrack) => {
    setProgress((p) => ({ ...p, [track.userId]: 0 }));
    try {
      saveFile(new Blob([await fetchTrack(track)], { type: "audio/ogg" }), fileName(track));
    } catch (err) {
      toast(`Couldn't download it: ${toFuwaError(err).message}`);
    } finally {
      done([track.userId]);
    }
  };
  const downloadAll = async () => {
    setProgress(Object.fromEntries(rec.tracks.map((t) => [t.userId, 0])));
    try {
      const files = [];
      for (const track of rec.tracks) files.push({ name: fileName(track), data: await fetchTrack(track) });
      saveFile(zip(files, started), `${safe(baseName(channel, rec))}.zip`);
    } catch (err) {
      toast(`Couldn't download it: ${toFuwaError(err).message}`);
    } finally {
      done(rec.tracks.map((t) => t.userId));
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
      <div className="flex items-center gap-3 px-4 pt-3.5 pb-2">
        <Wave live={live} />
        <div className="min-w-0 flex-1">
          <p className="truncate font-extrabold">
            {started.toLocaleString(undefined, { day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" })}
          </p>
          <p className="truncate text-xs text-muted-foreground">
            {live ? (
              <span className="font-bold text-[#ed4245]">Recording now · {clock(length)}</span>
            ) : (
              <>
                {clock(length)} · {formatBytes(Number(rec.sizeBytes))}
              </>
            )}{" "}
            · started by {starter}
          </p>
        </div>
        {!live && (
          <div className="flex shrink-0 items-center gap-1">
            <Button size="sm" variant="secondary" className="group h-8 rounded-xl font-bold" disabled={busy || !rec.tracks.length} onClick={() => void downloadAll()}>
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
              {progress[track.userId] !== undefined && (
                <motion.span
                  aria-hidden
                  className="absolute inset-y-0 left-0 w-full origin-left bg-primary/12"
                  initial={{ scaleX: 0, opacity: 1 }}
                  animate={{ scaleX: progress[track.userId] }}
                  exit={{ scaleX: 1, opacity: 0, transition: { duration: 0.35 } }}
                  transition={{ type: "spring", stiffness: 200, damping: 30 }}
                />
              )}
            </AnimatePresence>
            <TrackAvatar instanceKey={instanceKey} userId={track.userId} />
            <span className="relative min-w-0 flex-1 truncate text-sm font-bold">{names[track.userId]}</span>
            <span className="relative shrink-0 text-xs text-muted-foreground tabular-nums">{formatBytes(Number(track.sizeBytes))}</span>
            {!live && (
              <motion.button
                type="button"
                whileTap={{ scale: 0.85 }}
                disabled={progress[track.userId] !== undefined}
                onClick={() => void downloadOne(track)}
                aria-label={`Download ${names[track.userId]}'s track`}
                title="Download this track (Ogg Opus)"
                className="group relative grid size-8 shrink-0 place-items-center rounded-xl text-muted-foreground transition hover:bg-primary/10 hover:text-primary disabled:opacity-60"
              >
                {progress[track.userId] !== undefined ? <LoaderCircleIcon className="size-4 animate-spin" /> : <DownloadIcon className="size-4 transition-transform group-hover:translate-y-0.5" />}
              </motion.button>
            )}
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
