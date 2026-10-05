import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { LoaderCircleIcon, MegaphoneIcon, MegaphoneOffIcon, SirenIcon, TriangleAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useState, type FormEvent } from "react";
import { AnnouncementSchema, AnnouncementTone, type Announcement as AnnouncementMessage } from "@/gen/fuwa/v1/types_pb";
import { setAnnouncement } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { BannerBody } from "@/components/AnnouncementBanner";
import { endsLabel, isLive, toneOf } from "@/lib/announcement";
import { SPRING } from "@/lib/motion";
import { Chips } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { type Key, T, useI18n } from "@/i18n/react";
import { formatStamp, toDate } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { Choice, Setting } from "../controls";

const TEXT_MAX = 300;

/** How long it stays up. `keep` leaves an existing end where it is. */
type Ends = "keep" | "never" | number;
const HOUR = 3_600_000;
/** Labels are catalog keys. */
const LENGTHS: { value: Ends; label: Key }[] = [
  { value: "never", label: "instancesettings.announcement.untilDown" },
  { value: HOUR, label: "instancesettings.announcement.hour" },
  { value: 4 * HOUR, label: "instancesettings.announcement.fourHours" },
  { value: 24 * HOUR, label: "instancesettings.announcement.day" },
  { value: 7 * 24 * HOUR, label: "instancesettings.announcement.week" },
];

/**
 * The banner across the top of every client of this instance, for news and
 * maintenance notices. Changing only the tone or the end keeps it closed for
 * people who closed it; new words bring it back for everyone.
 */
export function Announcement({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const now = useNow(15_000);
  const current = inst?.node?.announcement;
  const live = isLive(current, now) ? current : undefined;
  const [text, setText] = useState(live?.text ?? "");
  const [tone, setTone] = useState<AnnouncementTone>(live ? toneOf(live) : AnnouncementTone.INFO);
  const [ends, setEnds] = useState<Ends>(live?.endsAt ? "keep" : "never");
  const save = useAction(setAnnouncement);
  const shout = useAnimationControls();

  const trimmed = text.trim();
  const endsAt = endTime(ends, live, now);
  const changed = !live || trimmed !== live.text || tone !== toneOf(live) || endChanged(ends, live);
  const draft = create(AnnouncementSchema, {
    id: trimmed === live?.text ? live.id : "draft",
    text: trimmed || t("instancesettings.announcement.previewText"),
    tone,
    endsAt: endsAt ? timestampFromDate(endsAt) : undefined,
  });

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!trimmed || !changed) return;
    void shout.start({ rotate: [0, -18, 12, -6, 0], scale: [1, 1.25, 1.1, 1], transition: { duration: 0.5 } });
    const next = await save.go(instanceKey, { text: trimmed, tone, endsAt });
    if (next === undefined) return;
    setEnds(next?.endsAt ? "keep" : "never");
    toast(t(live ? "instancesettings.announcement.updated" : "instancesettings.announcement.up"));
  }

  async function takeDown() {
    if ((await save.go(instanceKey, { text: "", tone })) === undefined) return;
    setText("");
    setEnds("never");
    toast(t("instancesettings.announcement.down"));
  }

  return (
    <form onSubmit={submit} className="flex flex-col">
      <Setting id="announcement-preview" title={t("settings.controls.preview")} hint={t("instancesettings.announcement.previewHint")} badge={false}>
        <div className="relative overflow-hidden rounded-2xl border bg-background/40">
          <BannerBody announcement={draft} preview onClose={tone === AnnouncementTone.CRITICAL ? undefined : () => {}} />
          <div className="flex h-20 gap-2 p-3 opacity-50" aria-hidden>
            <span className="w-10 rounded-xl bg-muted" />
            <span className="w-28 rounded-xl bg-muted/70" />
            <span className="flex flex-1 flex-col gap-2">
              <span className="h-3 w-2/3 rounded-full bg-muted" />
              <span className="h-3 w-1/2 rounded-full bg-muted/70" />
            </span>
          </div>
        </div>
        <LiveStatus live={live} />
      </Setting>

      <Setting id="announcement-text" title={t("instancesettings.announcement.message")} hint={t("instancesettings.announcement.messageHint")} badge={false} delay={0.04}>
        <div className="relative">
          <Textarea
            id="announcement-text"
            rows={2}
            maxLength={TEXT_MAX}
            value={text}
            placeholder={t("instancesettings.announcement.placeholder")}
            onChange={(e) => {
              setText(e.target.value);
              save.setError(null);
            }}
            className="min-h-20 rounded-xl pr-12"
          />
          <LengthRing length={text.length} max={TEXT_MAX} />
        </div>
      </Setting>

      <Setting id="announcement-tone" title={t("instancesettings.announcement.tone")} badge={false} delay={0.08}>
        <Choice
          value={tone}
          onChange={setTone}
          options={[
            { value: AnnouncementTone.INFO, label: t("instancesettings.announcement.info"), hint: t("instancesettings.announcement.infoHint"), icon: <MegaphoneIcon className="size-4" /> },
            { value: AnnouncementTone.WARNING, label: t("instancesettings.announcement.warning"), hint: t("instancesettings.announcement.warningHint"), icon: <TriangleAlertIcon className="size-4" /> },
            { value: AnnouncementTone.CRITICAL, label: t("instancesettings.announcement.critical"), hint: t("instancesettings.announcement.criticalHint"), icon: <SirenIcon className="size-4" /> },
          ]}
        />
      </Setting>

      <EndsSetting live={live} ends={ends} onChange={setEnds} endsAt={endsAt} />

      {save.error && <p className="mb-3 text-sm text-destructive first-letter:uppercase">{save.error}</p>}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <AnnouncementActions live={!!live} pending={save.pending} canSave={!!trimmed && changed} shout={shout} onTakeDown={takeDown} />
      </div>
    </form>
  );
}

/** When it would come down: where it was, never, or that long from now. */
function endTime(ends: Ends, live: AnnouncementMessage | undefined, now: number): Date | undefined {
  if (ends === "keep") return live?.endsAt ? toDate(live.endsAt) : undefined;
  return ends === "never" ? undefined : new Date(now + ends);
}

/** Whether the end picked differs from the one it has. */
const endChanged = (ends: Ends, live: AnnouncementMessage) => ends !== "keep" && (ends === "never" ? !!live.endsAt : true);

/** Whether one is up, since when and until when. */
function LiveStatus({ live }: { live: AnnouncementMessage | undefined }) {
  const { t } = useI18n();
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.p
        key={live ? `live-${live.id}` : "none"}
        initial={{ opacity: 0, y: 6 }}
        animate={{ opacity: 1, y: 0 }}
        exit={{ opacity: 0, y: -6 }}
        transition={SPRING}
        className="flex items-center gap-2 text-xs text-muted-foreground"
      >
        <span className={cn("relative size-2 rounded-full", live ? "bg-emerald-500" : "bg-muted-foreground/40")}>
          {live && <span className="absolute inset-0 animate-ping rounded-full bg-emerald-500/60" />}
        </span>
        {live
          ? live.endsAt
            ? t("instancesettings.announcement.upSinceUntil", { time: formatStamp(toDate(live.createdAt)), end: endsLabel(toDate(live.endsAt)) })
            : t("instancesettings.announcement.upSince", { time: formatStamp(toDate(live.createdAt)) })
          : t("instancesettings.announcement.nothingUp")}
      </motion.p>
    </AnimatePresence>
  );
}

/** How long it stays up, with the time it would come down. */
function EndsSetting({ live, ends, onChange, endsAt }: { live: AnnouncementMessage | undefined; ends: Ends; onChange: (ends: Ends) => void; endsAt: Date | undefined }) {
  const { t } = useI18n();
  const fixed = LENGTHS.map((l) => ({ value: l.value, label: t(l.label) }));
  const lengths: { value: Ends; label: string }[] =
    live?.endsAt ? [{ value: "keep", label: t("instancesettings.announcement.until", { time: endsLabel(toDate(live.endsAt)) }) }, ...fixed] : fixed;
  return (
    <Setting id="announcement-ends" title={t("instancesettings.announcement.comesDown")} hint={t("instancesettings.announcement.comesDownHint")} badge={false} delay={0.12}>
      <Chips label={t("instancesettings.announcement.comesDown")} value={ends} onChange={onChange} options={lengths} />
      <AnimatePresence initial={false}>
        {endsAt && ends !== "keep" && (
          <motion.p
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={SPRING}
            className="overflow-hidden text-xs text-muted-foreground"
          >
            <T k="instancesettings.announcement.comesDownAt" values={{ time: <b>{formatStamp(endsAt)}</b> }} />
          </motion.p>
        )}
      </AnimatePresence>
    </Setting>
  );
}

/** Taking it down, and putting it up (or updating it). */
function AnnouncementActions({
  live,
  pending,
  canSave,
  shout,
  onTakeDown,
}: {
  live: boolean;
  pending: boolean;
  canSave: boolean;
  shout: ReturnType<typeof useAnimationControls>;
  onTakeDown: () => void;
}) {
  const { t } = useI18n();
  return (
    <>
    <AnimatePresence initial={false}>
      {live && (
        <motion.span initial={{ opacity: 0, x: 10 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 10 }} transition={SPRING} className="mr-auto">
          <Button type="button" variant="ghost" disabled={pending} onClick={onTakeDown} className="group rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive">
            <MegaphoneOffIcon className="transition-transform group-hover:-rotate-12" /> {t("instancesettings.announcement.takeDown")}
          </Button>
        </motion.span>
      )}
    </AnimatePresence>
    <Button type="submit" disabled={!canSave || pending} className="btn group rounded-xl px-4 font-bold">
      {pending ? (
        <LoaderCircleIcon className="animate-spin" />
      ) : (
        <motion.span animate={shout} className="inline-flex transition-transform group-hover:-rotate-12">
          <MegaphoneIcon />
        </motion.span>
      )}
      {t(live ? "instancesettings.announcement.update" : "instancesettings.announcement.putUp")}
    </Button>
    </>
  );
}

/** How much of the limit is used, as a ring that warms up near the end. */
function LengthRing({ length, max }: { length: number; max: number }) {
  const share = Math.min(1, length / max);
  const left = max - length;
  const r = 9;
  const c = 2 * Math.PI * r;
  const color = left <= 0 ? "text-destructive" : left <= 30 ? "text-amber-500" : "text-primary";
  return (
    <span className="pointer-events-none absolute right-2.5 bottom-2.5 grid size-7 place-items-center" aria-hidden>
      <svg viewBox="0 0 24 24" className={cn("absolute inset-0 -rotate-90 transition-colors", color)}>
        <circle cx="12" cy="12" r={r} fill="none" stroke="currentColor" strokeOpacity={0.15} strokeWidth={2.5} />
        <motion.circle
          cx="12"
          cy="12"
          r={r}
          fill="none"
          stroke="currentColor"
          strokeWidth={2.5}
          strokeLinecap="round"
          strokeDasharray={c}
          initial={false}
          animate={{ strokeDashoffset: c * (1 - share) }}
          transition={SPRING}
        />
      </svg>
      <AnimatePresence>
        {left <= 30 && (
          <motion.span
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            exit={{ scale: 0 }}
            transition={SPRING}
            className={cn("text-[0.6rem] font-extrabold tabular-nums", color)}
          >
            {left}
          </motion.span>
        )}
      </AnimatePresence>
    </span>
  );
}
