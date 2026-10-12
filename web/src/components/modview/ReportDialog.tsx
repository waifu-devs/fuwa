import { FlagIcon, LoaderCircleIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useId, useState, type FormEvent } from "react";
import type { ModReportReason, User } from "@/gen/fuwa/v1/types_pb";
import { reportMember, reportMessage } from "@/fuwa/moderation";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";
import { REASONS, REPORT_NOTE_MAX, snippet } from "@/lib/mod-view";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** What's being reported: someone's message, or the person. */
export type ReportTarget =
  | { kind: "message"; instanceKey: string; serverId: string; channelId: string; messageId: string; author: User | undefined; content: string }
  | { kind: "member"; instanceKey: string; serverId: string; user: User };

/**
 * Reporting a message or a person to the server's moderators: a reason and,
 * if they like, a few words. The person reported is never told
 * (docs/mod-view.md).
 */
export function ReportDialog({ target, onClose }: { target: ReportTarget | null; onClose: () => void }) {
  // Keep what was shown while the dialog closes.
  const [shown, setShown] = useState<ReportTarget | null>(null);
  if (target && target !== shown) setShown(target);
  return (
    <Dialog open={!!target} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>{shown && <Body key={shown.kind === "message" ? shown.messageId : shown.user.id} target={shown} onDone={onClose} />}</DialogContent>
    </Dialog>
  );
}

function Body({ target, onDone }: { target: ReportTarget; onDone: () => void }) {
  const { t } = useI18n();
  const id = useId();
  const [reason, setReason] = useState<number>(0);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const who = target.kind === "message" ? target.author : target.user;
  const name = displayName(who);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!reason || busy) return;
    setBusy(true);
    setError(null);
    try {
      const why = reason as ModReportReason;
      if (target.kind === "message") await reportMessage(target.instanceKey, target.serverId, target.channelId, target.messageId, why, note.trim());
      else await reportMember(target.instanceKey, target.serverId, target.user.id, why, note.trim());
      toast(t("chattools.report.sent"));
      onDone();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-4">
      <DialogHeader
        title={
          <span className="flex items-center gap-2.5">
            <motion.span
              initial={{ scale: 0.4, rotate: -25 }}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: "spring", stiffness: 520, damping: 14 }}
              className="grid size-9 shrink-0 place-items-center rounded-xl bg-destructive/15 text-destructive"
            >
              <FlagIcon className="size-5" />
            </motion.span>
            {target.kind === "message" ? t("chattools.report.messageTitle") : t("chattools.report.memberTitle", { name })}
          </span>
        }
        description={t("chattools.report.about")}
      />
      <div className="flex items-center gap-3 rounded-2xl bg-muted/60 p-3">
        <UserAvatar user={who} className="size-9" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-bold">{name}</span>
          {target.kind === "message" ? (
            <span className="line-clamp-2 text-xs break-words text-muted-foreground">{snippet(target.content, 200) || t("chattools.modview.noText")}</span>
          ) : (
            <span className="block truncate text-xs text-muted-foreground">@{target.user.username}</span>
          )}
        </span>
      </div>

      <fieldset className="flex flex-col gap-2">
        <legend className="mb-2 text-sm font-bold">{t("chattools.report.why")}</legend>
        <div role="radiogroup" className="grid gap-1.5 sm:grid-cols-2">
          {REASONS.map((r, n) => {
            const active = r.value === reason;
            return (
              <motion.button
                key={r.value}
                type="button"
                role="radio"
                aria-checked={active}
                onClick={() => setReason(r.value)}
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: n * 0.02 } }}
                whileTap={{ scale: 0.97 }}
                className={cn(
                  "relative flex flex-col rounded-xl border px-3 py-2 text-left transition-colors",
                  active ? "border-primary/60" : "hover:border-primary/30 hover:bg-muted/40",
                  r.value === 9 && "sm:col-span-2",
                )}
              >
                {active && <motion.span layoutId={`${id}-reason`} transition={SPRING} className="absolute inset-0 rounded-xl bg-primary/12" />}
                <span className={cn("relative text-sm font-bold", active && "text-primary")}>{t(r.label)}</span>
                <span className="relative text-xs text-muted-foreground">{t(r.about)}</span>
              </motion.button>
            );
          })}
        </div>
      </fieldset>

      <div className="flex flex-col gap-2">
        <Label htmlFor={`${id}-note`} className="flex items-center justify-between font-bold">
          {t("chattools.report.note")}
          <span className="text-xs font-normal text-muted-foreground tabular-nums">
            <Count value={REPORT_NOTE_MAX - note.length} />
          </span>
        </Label>
        <Textarea
          id={`${id}-note`}
          rows={3}
          maxLength={REPORT_NOTE_MAX}
          value={note}
          placeholder={t("chattools.report.notePlaceholder")}
          onChange={(e) => setNote(e.target.value)}
          className="rounded-xl"
        />
        <p className="text-xs text-muted-foreground">{t("chattools.report.private")}</p>
      </div>

      <AnimatePresence mode="popLayout" initial={false}>
        {error && (
          <motion.p key="error" {...SLIDE_IN} transition={SPRING} className="text-sm text-destructive first-letter:uppercase">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <motion.div layout="position" transition={SPRING} className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onDone} className="rounded-xl">
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="destructive" disabled={!reason || busy} className="group rounded-xl font-bold">
          {busy ? <LoaderCircleIcon className="animate-spin" /> : <FlagIcon className="transition-transform group-hover:-rotate-12" />}
          {t("chattools.report.send")}
        </Button>
      </motion.div>
    </form>
  );
}
