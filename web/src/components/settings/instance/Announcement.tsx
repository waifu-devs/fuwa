import { create } from "@bufbuild/protobuf";
import { timestampFromDate } from "@bufbuild/protobuf/wkt";
import { LoaderCircleIcon, MegaphoneIcon, MegaphoneOffIcon, SirenIcon, TriangleAlertIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useState, type FormEvent } from "react";
import { AnnouncementSchema, AnnouncementTone } from "@/gen/fuwa/v1/types_pb";
import { setAnnouncement } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { BannerBody, endsLabel, isLive, toneOf } from "@/components/AnnouncementBanner";
import { SPRING } from "@/components/motion";
import { Chips } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { formatStamp, toDate } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { Choice, Setting } from "../controls";

const TEXT_MAX = 300;

/** How long it stays up. `keep` leaves an existing end where it is. */
type Ends = "keep" | "never" | number;
const HOUR = 3_600_000;
const LENGTHS: { value: Ends; label: string }[] = [
  { value: "never", label: "Until taken down" },
  { value: HOUR, label: "1 hour" },
  { value: 4 * HOUR, label: "4 hours" },
  { value: 24 * HOUR, label: "1 day" },
  { value: 7 * 24 * HOUR, label: "1 week" },
];

/**
 * The banner across the top of every client of this instance, for news and
 * maintenance notices. Changing only the tone or the end keeps it closed for
 * people who closed it; new words bring it back for everyone.
 */
export function Announcement({ instanceKey }: { instanceKey: string }) {
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
  const endsAt =
    ends === "keep" ? (live?.endsAt ? toDate(live.endsAt) : undefined) : ends === "never" ? undefined : new Date(now + ends);
  const changed =
    !live ||
    trimmed !== live.text ||
    tone !== toneOf(live) ||
    (ends !== "keep" && (ends === "never" ? !!live.endsAt : true));
  const draft = create(AnnouncementSchema, {
    id: trimmed === live?.text ? live.id : "draft",
    text: trimmed || "Your announcement shows here.",
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
    toast(live ? "Announcement updated" : "Announcement is up");
  }

  async function takeDown() {
    if ((await save.go(instanceKey, { text: "", tone })) === undefined) return;
    setText("");
    setEnds("never");
    toast("Announcement taken down");
  }

  const lengths: { value: Ends; label: string }[] =
    live?.endsAt ? [{ value: "keep", label: `Until ${endsLabel(toDate(live.endsAt))}` }, ...LENGTHS] : LENGTHS;

  return (
    <form onSubmit={submit} className="flex flex-col">
      <Setting id="announcement-preview" title="Preview" hint="What everyone on this instance sees at the top of the app." badge={false}>
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
              ? `Up since ${formatStamp(toDate(live.createdAt))}${live.endsAt ? `, until ${endsLabel(toDate(live.endsAt))}` : ""}`
              : "Nothing is up right now."}
          </motion.p>
        </AnimatePresence>
      </Setting>

      <Setting id="announcement-text" title="Message" hint="Bold, italics, code and links work. Keep it to a line or two." badge={false} delay={0.04}>
        <div className="relative">
          <Textarea
            id="announcement-text"
            rows={2}
            maxLength={TEXT_MAX}
            value={text}
            placeholder="We're moving to a bigger server tonight at 10 PM. Expect a few minutes offline."
            onChange={(e) => {
              setText(e.target.value);
              save.setError(null);
            }}
            className="min-h-20 rounded-xl pr-12"
          />
          <LengthRing length={text.length} max={TEXT_MAX} />
        </div>
      </Setting>

      <Setting id="announcement-tone" title="Tone" badge={false} delay={0.08}>
        <Choice
          value={tone}
          onChange={setTone}
          options={[
            { value: AnnouncementTone.INFO, label: "Info", hint: "News and small notes. People can close it.", icon: <MegaphoneIcon className="size-4" /> },
            { value: AnnouncementTone.WARNING, label: "Heads-up", hint: "Maintenance or a change coming soon.", icon: <TriangleAlertIcon className="size-4" /> },
            { value: AnnouncementTone.CRITICAL, label: "Urgent", hint: "Something is wrong now. Nobody can close it.", icon: <SirenIcon className="size-4" /> },
          ]}
        />
      </Setting>

      <Setting id="announcement-ends" title="Comes down" hint="It disappears from every client by itself at the end." badge={false} delay={0.12}>
        <Chips label="Comes down" value={ends} onChange={setEnds} options={lengths} />
        <AnimatePresence initial={false}>
          {endsAt && ends !== "keep" && (
            <motion.p
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={SPRING}
              className="overflow-hidden text-xs text-muted-foreground"
            >
              Comes down <b>{formatStamp(endsAt)}</b>, counted from when you put it up.
            </motion.p>
          )}
        </AnimatePresence>
      </Setting>

      {save.error && <p className="mb-3 text-sm text-destructive first-letter:uppercase">{save.error}</p>}
      <div className="flex flex-wrap items-center justify-end gap-2">
        <AnimatePresence initial={false}>
          {live && (
            <motion.span initial={{ opacity: 0, x: 10 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 10 }} transition={SPRING} className="mr-auto">
              <Button type="button" variant="ghost" disabled={save.pending} onClick={takeDown} className="group rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive">
                <MegaphoneOffIcon className="transition-transform group-hover:-rotate-12" /> Take it down
              </Button>
            </motion.span>
          )}
        </AnimatePresence>
        <Button type="submit" disabled={!trimmed || !changed || save.pending} className="btn group rounded-xl px-4 font-bold">
          {save.pending ? (
            <LoaderCircleIcon className="animate-spin" />
          ) : (
            <motion.span animate={shout} className="inline-flex transition-transform group-hover:-rotate-12">
              <MegaphoneIcon />
            </motion.span>
          )}
          {live ? "Update" : "Put it up"}
        </Button>
      </div>
    </form>
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
