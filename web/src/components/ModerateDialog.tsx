import { DoorOpenIcon, GavelIcon, HourglassIcon, LoaderCircleIcon, PencilIcon, TimerOffIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useState, type FormEvent } from "react";
import type { Member } from "@/gen/fuwa/v1/types_pb";
import { banMember, kickMember, setNickname, timeOutMember } from "@/fuwa/actions";
import { useAccess, useAction, useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/lib/motion";
import { Chips } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { formatDuration, formatLeft, formatStamp, memberName, timedOutUntil } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { useNow } from "@/lib/notifications";
import { moderationFor, type ModAction } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

export type { ModAction };

/** Discord's time-out lengths. */
const TIME_OUT = [60, 5 * 60, 10 * 60, 60 * 60, 24 * 60 * 60, 7 * 24 * 60 * 60];

/** How much of a banned person's history goes with them. */
const DELETE = [
  { value: 0, label: "workspace.moderate.purge.keep" },
  { value: 60 * 60, label: "workspace.moderate.purge.hour" },
  { value: 6 * 60 * 60, label: "workspace.moderate.purge.hours6" },
  { value: 24 * 60 * 60, label: "workspace.moderate.purge.hours24" },
  { value: 3 * 24 * 60 * 60, label: "workspace.moderate.purge.days3" },
  { value: 7 * 24 * 60 * 60, label: "workspace.moderate.purge.days7" },
] as const;

const REASON_MAX = 512;

const SUBMIT = { kick: "workspace.moderate.submit.kick", ban: "workspace.moderate.submit.ban", nickname: "workspace.moderate.submit.save" } as const;

/**
 * Time out, kick, ban or rename someone, from the member list, their profile
 * card or the Members page. Each asks for a reason, kept in the audit log.
 */
export function ModerateDialog({
  instanceKey,
  serverId,
  member,
  action,
  onClose,
}: {
  instanceKey: string;
  serverId: string;
  member: Member | null;
  action: ModAction | null;
  onClose: () => void;
}) {
  const open = !!member && !!action;
  // Keep what was shown while the dialog closes.
  const [shown, setShown] = useState<{ member: Member; action: ModAction } | null>(null);
  if (member && action && (shown?.member !== member || shown.action !== action)) setShown({ member, action });
  return (
    <Dialog open={open} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        {shown && (
          <Body key={`${shown.action}-${shown.member.user?.id}`} instanceKey={instanceKey} serverId={serverId} member={shown.member} action={shown.action} onDone={onClose} />
        )}
      </DialogContent>
    </Dialog>
  );
}

/** The four calls behind the dialog, each saying how it went once it's done. */
function useModerate(instanceKey: string, serverId: string, member: Member, onDone: () => void) {
  const lang = useI18n();
  const { t } = lang;
  const name = memberName(member);
  const userId = member.user?.id ?? "";
  const timeOut = useAction(timeOutMember);
  const kick = useAction(kickMember);
  const ban = useAction(banMember);
  const rename = useAction(setNickname);
  const gavel = useAnimationControls();

  async function act(action: ModAction, { seconds, reason, purge, nickname }: { seconds: number; reason: string; purge: number; nickname: string }) {
    if (action === "timeout") {
      if ((await timeOut.go(instanceKey, serverId, userId, seconds, reason.trim())) === undefined) return;
      toast(t("workspace.moderate.timedOut", { name, duration: formatDuration(lang, seconds) }));
    } else if (action === "kick") {
      if ((await kick.go(instanceKey, serverId, userId, reason.trim())) === undefined) return;
      toast(t("workspace.moderate.kicked", { name }));
    } else if (action === "ban") {
      await gavel.start({ rotate: [0, -50, 20, 0], transition: { duration: 0.45, times: [0, 0.4, 0.7, 1] } });
      const deleted = await ban.go(instanceKey, serverId, userId, reason.trim(), purge);
      if (deleted === undefined) return;
      toast(deleted ? t("workspace.moderate.bannedDeleted", { name, count: deleted }) : t("workspace.moderate.banned", { name }));
    } else {
      if ((await rename.go(instanceKey, serverId, nickname.trim(), userId)) === undefined) return;
    }
    onDone();
  }

  async function endTimeOut() {
    if ((await timeOut.go(instanceKey, serverId, userId, 0, "")) === undefined) return;
    toast(t("workspace.moderate.canTalk", { name }));
    onDone();
  }

  return {
    act,
    endTimeOut,
    gavel,
    busy: timeOut.pending || kick.pending || ban.pending || rename.pending,
    error: timeOut.error ?? kick.error ?? ban.error ?? rename.error,
  };
}

function Body({ instanceKey, serverId, member, action, onDone }: { instanceKey: string; serverId: string; member: Member; action: ModAction; onDone: () => void }) {
  const { t } = useI18n();
  const name = memberName(member);
  const now = useNow(1000);
  const until = timedOutUntil(member, now);
  const [reason, setReason] = useState("");
  const [seconds, setSeconds] = useState(TIME_OUT[3]!);
  const [purge, setPurge] = useState(0);
  // Their nickname until you change it.
  const [draft, setNick] = useState<string | null>(null);
  const nickname = draft ?? member.nickname;
  const { act, endTimeOut, gavel, busy, error } = useModerate(instanceKey, serverId, member, onDone);

  function submit(e: FormEvent) {
    e.preventDefault();
    void act(action, { seconds, reason, purge, nickname });
  }

  const title = t(`workspace.moderate.title.${action}`, { name });
  const description = t(`workspace.moderate.about.${action}`);

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <DialogHeader title={title} description={description} />
      <Who member={member} until={until} now={now} />

      {action === "timeout" && <TimeOutLength seconds={seconds} onChange={setSeconds} replacing={!!until} now={now} />}
      {action === "ban" && <PurgeChoice purge={purge} onChange={setPurge} />}
      {action === "nickname" ? (
        <div className="flex flex-col gap-2">
          <Label htmlFor="mod-nickname" className="font-bold">
            {t("workspace.moderate.nickname")}
          </Label>
          <Input id="mod-nickname" autoFocus maxLength={32} value={nickname} placeholder={memberName({ ...member, nickname: "" })} onChange={(e) => setNick(e.target.value)} className="h-11 rounded-xl" />
        </div>
      ) : (
        <Reason reason={reason} onChange={setReason} />
      )}

      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {action === "timeout" && until && (
          <Button type="button" variant="ghost" disabled={busy} onClick={endTimeOut} className="mr-auto rounded-xl">
            <TimerOffIcon /> {t("workspace.moderate.endTimeout")}
          </Button>
        )}
        <Button type="button" variant="ghost" disabled={busy} onClick={onDone} className="rounded-xl">
          {t("common.cancel")}
        </Button>
        <Button
          type="submit"
          variant={action === "nickname" ? "default" : "destructive"}
          disabled={busy}
          className={cn("group rounded-xl font-bold", action === "nickname" && "btn")}
        >
          {busy ? <LoaderCircleIcon className="animate-spin" /> : <ActionIcon action={action} gavel={gavel} />}
          {t(action === "timeout" ? (until ? "workspace.moderate.submit.changeTimeout" : "workspace.moderate.submit.timeout") : SUBMIT[action])}
        </Button>
      </div>
    </form>
  );
}

/** How long a time-out lasts, and when it would end. */
function TimeOutLength({ seconds, onChange, replacing, now }: { seconds: number; onChange: (seconds: number) => void; replacing: boolean; now: number }) {
  const lang = useI18n();
  const { t } = lang;
  return (
    <div className="flex flex-col gap-2">
      <Label className="font-bold">{t("workspace.moderate.howLong")}</Label>
      <Chips label={t("workspace.moderate.lengthLabel")} value={seconds} onChange={onChange} options={TIME_OUT.map((s) => ({ value: s, label: formatDuration(lang, s) }))} />
      <p className="text-xs text-muted-foreground">
        <T k={replacing ? "workspace.moderate.endsReplacing" : "workspace.moderate.ends"} values={{ time: <b>{formatStamp(new Date(now + seconds * 1000))}</b> }} />
      </p>
    </div>
  );
}

/** How much of their history a ban takes with it. */
function PurgeChoice({ purge, onChange }: { purge: number; onChange: (purge: number) => void }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-2">
      <Label className="font-bold">{t("workspace.moderate.purgeLabel")}</Label>
      <Chips label={t("workspace.moderate.purgeChips")} value={purge} onChange={onChange} options={DELETE.map((d) => ({ value: d.value, label: t(d.label) }))} />
    </div>
  );
}

function Reason({ reason, onChange }: { reason: string; onChange: (reason: string) => void }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-2">
      <Label htmlFor="mod-reason" className="flex items-center justify-between font-bold">
        {t("workspace.moderate.reason")} <span className="text-xs font-normal text-muted-foreground">{t("workspace.moderate.reasonHint")}</span>
      </Label>
      <Textarea id="mod-reason" rows={2} maxLength={REASON_MAX} value={reason} onChange={(e) => onChange(e.target.value)} className="rounded-xl" />
    </div>
  );
}

/** Who it's about, and how long their time-out has left. */
function Who({ member, until, now }: { member: Member; until: Date | null; now: number }) {
  const lang = useI18n();
  const { t } = lang;
  return (
    <div className="flex items-center gap-3 rounded-2xl bg-muted/60 p-3">
      <UserAvatar user={member.user} className="size-10" />
      <span className="min-w-0 flex-1">
        <span className="block truncate font-bold">{memberName(member)}</span>
        <span className="block truncate text-xs text-muted-foreground">@{member.user?.username}</span>
      </span>
      <AnimatePresence>
        {until && (
          <motion.span
            initial={{ scale: 0.6, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={{ scale: 0.6, opacity: 0 }}
            transition={SPRING}
            className="flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-1 text-xs font-bold text-amber-600 tabular-nums dark:text-amber-400"
            title={t("workspace.moderate.until", { time: formatStamp(until) })}
          >
            <HourglassIcon className="size-3.5 animate-[spin_3s_ease-in-out_infinite]" /> {formatLeft(lang, until.getTime() - now)}
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}

/** The submit button's icon; the gavel swings before a ban. */
function ActionIcon({ action, gavel }: { action: ModAction; gavel: ReturnType<typeof useAnimationControls> }) {
  if (action === "ban") {
    return (
      <motion.span animate={gavel} style={{ originX: 0.2, originY: 0.8 }} className="inline-flex transition-transform group-hover:-rotate-12">
        <GavelIcon />
      </motion.span>
    );
  }
  if (action === "kick") return <DoorOpenIcon className="transition-transform group-hover:translate-x-0.5" />;
  if (action === "timeout") return <HourglassIcon className="transition-transform duration-500 group-hover:rotate-180" />;
  return <PencilIcon />;
}

/** What you may do to someone: what your permissions allow, and only to people ranked below you. */
export function useModeration(instanceKey: string, serverId: string, target: Member | undefined) {
  const access = useAccess(instanceKey, serverId);
  const roles = useRoles(instanceKey, serverId);
  const ownerId = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.ownerId ?? "");
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id);
  return moderationFor(access, ownerId, roles, meId, target);
}
