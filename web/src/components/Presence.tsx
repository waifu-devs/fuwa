import { timestampDate } from "@bufbuild/protobuf/wkt";
import { ExternalLinkIcon, Gamepad2Icon, HeadphonesIcon, RadioIcon, TrophyIcon, TvIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { ActivityKind, PresenceStatus, type Activity, type Presence } from "@/gen/fuwa/v1/presence_pb";
import { usePresence } from "@/fuwa/presence";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { i18n, type Key } from "@/i18n/i18n";
import { T, useI18n } from "@/i18n/react";
import { shownPicture } from "@/lib/shown";
import { reportUsage } from "@/lib/reports";
import { cn } from "@/lib/utils";

/**
 * Presence on screen (docs/presence.md): the dot on avatars, the one line
 * under a name in the member list, and activity cards on profile cards.
 */

type Shown = "online" | "idle" | "dnd" | "offline";

const SHOWN: Record<number, Shown> = {
  [PresenceStatus.ONLINE]: "online",
  [PresenceStatus.IDLE]: "idle",
  [PresenceStatus.DO_NOT_DISTURB]: "dnd",
};

export const STATUS_LABEL: Record<Shown | "invisible", Key> = {
  online: "workspace.presence.status.online",
  idle: "workspace.presence.status.idle",
  dnd: "workspace.presence.status.dnd",
  offline: "workspace.presence.status.offline",
  invisible: "workspace.presence.status.invisible",
};

export const shownOf = (presence: Presence | undefined): Shown => (presence ? (SHOWN[presence.status] ?? "offline") : "offline");

/** The dot itself, for a status already known. */
export function StatusDot({ status, className }: { status: Shown; className?: string }) {
  const { t } = useI18n();
  return (
    <motion.span
      key={status}
      initial={{ scale: 0.3, opacity: 0.4 }}
      animate={{ scale: 1, opacity: 1 }}
      transition={{ type: "spring", stiffness: 600, damping: 16 }}
      role="img"
      aria-label={t(STATUS_LABEL[status])}
      title={t(STATUS_LABEL[status])}
      data-status={status}
      className={cn("presence-dot", className)}
    />
  );
}

/**
 * Someone's dot, for placing on their avatar. Subscribes to that one person,
 * so a change redraws only the dot. Draws nothing on instances without
 * presence, or while `hideOffline` and they're offline.
 */
export function PresenceDot({
  instanceKey,
  userId,
  className,
  hideOffline = false,
}: {
  instanceKey: string;
  userId: string | undefined;
  className?: string;
  hideOffline?: boolean;
}) {
  const presence = usePresence(instanceKey, userId);
  const status = shownOf(presence);
  if (hideOffline && status === "offline") return null;
  return <StatusDot status={status} className={cn("ring-[3px] ring-card", className)} />;
}

/** "Playing {name}", "Listening to {name}"… by kind. */
const LINE: Record<number, Key> = {
  [ActivityKind.PLAYING]: "workspace.presence.line.playing",
  [ActivityKind.STREAMING]: "workspace.presence.line.streaming",
  [ActivityKind.LISTENING]: "workspace.presence.line.listening",
  [ActivityKind.WATCHING]: "workspace.presence.line.watching",
  [ActivityKind.COMPETING]: "workspace.presence.line.competing",
};

const HEADING: Record<number, Key> = {
  [ActivityKind.PLAYING]: "workspace.presence.heading.playing",
  [ActivityKind.STREAMING]: "workspace.presence.heading.streaming",
  [ActivityKind.LISTENING]: "workspace.presence.heading.listening",
  [ActivityKind.WATCHING]: "workspace.presence.heading.watching",
  [ActivityKind.COMPETING]: "workspace.presence.heading.competing",
};

const ICON: Record<number, LucideIcon> = {
  [ActivityKind.PLAYING]: Gamepad2Icon,
  [ActivityKind.STREAMING]: RadioIcon,
  [ActivityKind.LISTENING]: HeadphonesIcon,
  [ActivityKind.WATCHING]: TvIcon,
  [ActivityKind.COMPETING]: TrophyIcon,
};

// ─────────────── Timers ───────────────

/**
 * One clock for every running timer on screen. Each tick writes the new
 * text straight into its element, so timers count without React drawing
 * anything, and the clock stops when no timer is shown.
 */
const timers = new Map<HTMLElement, () => string>();
let clock: ReturnType<typeof setInterval> | null = null;

function tick() {
  for (const [el, text] of timers) {
    const next = text();
    if (el.textContent !== next) el.textContent = next;
  }
}

function watchTimer(el: HTMLElement, text: () => string) {
  timers.set(el, text);
  el.textContent = text();
  clock ??= setInterval(tick, 1000);
  return () => {
    timers.delete(el);
    if (!timers.size && clock) {
      clearInterval(clock);
      clock = null;
    }
  };
}

const clockText = (ms: number) => {
  const total = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const two = (n: number) => String(n).padStart(2, "0");
  return h ? `${h}:${two(m)}:${two(s)}` : `${m}:${two(s)}`;
};

/** "12:04 elapsed" counting up, or "3:10 left" counting down. */
export function ActivityTimer({ activity, className }: { activity: Activity; className?: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  const start = activity.startedAt ? timestampDate(activity.startedAt).getTime() : 0;
  const end = activity.endsAt ? timestampDate(activity.endsAt).getTime() : 0;
  useEffect(() => {
    const el = ref.current;
    if (!el || (!start && !end)) return;
    return watchTimer(el, () =>
      end ? i18n().t("workspace.presence.left", { time: clockText(end - Date.now()) }) : i18n().t("workspace.presence.elapsed", { time: clockText(Date.now() - start) }),
    );
  }, [start, end]);
  if (!start && !end) return null;
  return <span ref={ref} className={cn("tabular-nums", className)} />;
}

// ─────────────── Lines and cards ───────────────

/** "Playing **Celeste**", for under a name. */
export function ActivityLine({ activity, className }: { activity: Activity; className?: string }) {
  return (
    <span className={cn("truncate", className)}>
      <T k={(Object.hasOwn(LINE, activity.kind) ? LINE[activity.kind] : undefined) ?? LINE[ActivityKind.PLAYING]!} values={{ name: <b className="font-bold text-foreground/85">{activity.name}</b> }} />
    </span>
  );
}

/** What someone's doing, on their profile card: one card per activity. */
export function ActivityCards({ instanceKey, userId }: { instanceKey: string; userId: string }) {
  const presence = usePresence(instanceKey, userId);
  const activities = presence?.activities ?? [];
  return (
    <AnimatePresence initial={false}>
      {activities.map((activity, n) => (
        <motion.div
          key={`${activity.applicationId}:${activity.name}`}
          initial={{ opacity: 0, y: 8, scale: 0.97 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, scale: 0.97 }}
          transition={{ type: "spring", stiffness: 480, damping: 32, delay: 0.05 + n * 0.04 }}
        >
          <ActivityCard activity={activity} />
        </motion.div>
      ))}
    </AnimatePresence>
  );
}

export function ActivityCard({ activity: a }: { activity: Activity }) {
  const { t } = useI18n();
  const Icon = ICON[a.kind] ?? Gamepad2Icon;
  const large = shownPicture(a.largeImageUrl);
  const small = shownPicture(a.smallImageUrl);
  const [leaving, setLeaving] = useState<{ label: string; url: string } | null>(null);
  return (
    <div className="mt-3 rounded-2xl bg-muted/60 p-3">
      <p className="mb-2 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t((Object.hasOwn(HEADING, a.kind) ? HEADING[a.kind] : undefined) ?? HEADING[ActivityKind.PLAYING]!)}</p>
      <div className="flex items-center gap-3">
        <span className="relative shrink-0">
          {large ? (
            <img src={large} alt={a.largeText || a.name} title={a.largeText || undefined} className="size-16 rounded-xl object-cover" draggable={false} />
          ) : (
            // Pictures by key point at Discord, which apps never load: the app's kind stands in.
            <span className="activity-tile grid size-16 place-items-center rounded-xl text-primary-foreground">
              <Icon className="size-8 drop-shadow" />
            </span>
          )}
          {small && (
            <img
              src={small}
              alt={a.smallText || ""}
              title={a.smallText || undefined}
              className="absolute -right-1.5 -bottom-1.5 size-6 rounded-full object-cover ring-[3px] ring-muted"
              draggable={false}
            />
          )}
        </span>
        <span className="min-w-0 text-sm leading-snug">
          <span className="block truncate font-extrabold">{a.name}</span>
          {a.details && <span className="block truncate">{a.details}</span>}
          {(a.state || a.partyMax > 0) && (
            <span className="block truncate">
              {a.state}
              {a.partyMax > 0 && ` ${t("workspace.presence.party", { size: a.partySize, max: a.partyMax })}`}
            </span>
          )}
          <ActivityTimer activity={a} className="block text-xs text-muted-foreground" />
        </span>
      </div>
      {a.buttons.length > 0 && (
        <div className="mt-3 flex flex-col gap-1.5">
          {a.buttons.map((b) => (
            <button
              key={b.url}
              type="button"
              onClick={() => setLeaving(b)}
              className="flex items-center justify-center gap-1.5 rounded-lg bg-background/70 px-3 py-1.5 text-sm font-bold transition hover:bg-background active:scale-[0.98]"
            >
              <span className="truncate">{b.label}</span>
              <ExternalLinkIcon className="size-3.5 shrink-0 opacity-60" />
            </button>
          ))}
        </div>
      )}
      <LeaveDialog link={leaving} onClose={() => setLeaving(null)} />
    </div>
  );
}

const hostOf = (url: string) => {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
};

/** Before following a link someone's activity carries: says where it goes. */
function LeaveDialog({ link, onClose }: { link: { label: string; url: string } | null; onClose: () => void }) {
  // Kept while the dialog closes, so its text doesn't vanish mid-animation.
  const { t } = useI18n();
  const [shown, setShown] = useState(link);
  if (link && link !== shown) setShown(link);
  const host = shown ? hostOf(shown.url) : "";
  const safe = !!shown && shown.url.startsWith("https://") && !!host;
  return (
    <Dialog open={!!link} onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader title={t("workspace.presence.leave.title", { host: host || t("workspace.presence.leave.thisLink") })} description={t("workspace.presence.leave.about")} />
        <p className="rounded-xl bg-muted px-3 py-2 font-mono text-xs break-all">{shown?.url}</p>
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" onClick={onClose} className="rounded-xl px-4 py-2 text-sm font-bold transition hover:bg-muted">
            {t("workspace.presence.leave.stay")}
          </button>
          <button
            type="button"
            disabled={!safe}
            onClick={() => {
              if (!shown || !safe) return;
              reportUsage("presence.button_open");
              window.open(shown.url, "_blank", "noopener,noreferrer");
              onClose();
            }}
            className="rounded-xl bg-primary px-4 py-2 text-sm font-bold text-primary-foreground transition hover:brightness-110 disabled:opacity-50"
          >
            {t("workspace.presence.leave.open", { host })}
          </button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
