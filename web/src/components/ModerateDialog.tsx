import { DoorOpenIcon, GavelIcon, HourglassIcon, LoaderCircleIcon, PencilIcon, TimerOffIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import { Permission, type Member, type Role } from "@/gen/fuwa/v1/types_pb";
import { banMember, kickMember, setNickname, timeOutMember } from "@/fuwa/actions";
import { useAccess, useAction, useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Chips } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { formatDuration, formatLeft, formatStamp, memberName, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { has, outranks, standing, type Access } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

export type ModAction = "timeout" | "kick" | "ban" | "nickname";

/** Discord's time-out lengths. */
const TIME_OUT = [60, 5 * 60, 10 * 60, 60 * 60, 24 * 60 * 60, 7 * 24 * 60 * 60].map((s) => ({ value: s, label: formatDuration(s) }));

/** How much of a banned person's history goes with them. */
const DELETE = [
  { value: 0, label: "Keep it" },
  { value: 60 * 60, label: "Last hour" },
  { value: 6 * 60 * 60, label: "6 hours" },
  { value: 24 * 60 * 60, label: "24 hours" },
  { value: 3 * 24 * 60 * 60, label: "3 days" },
  { value: 7 * 24 * 60 * 60, label: "7 days" },
];

const REASON_MAX = 512;

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
  useEffect(() => {
    if (member && action) setShown({ member, action });
  }, [member, action]);
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

function Body({ instanceKey, serverId, member, action, onDone }: { instanceKey: string; serverId: string; member: Member; action: ModAction; onDone: () => void }) {
  const name = memberName(member);
  const userId = member.user?.id ?? "";
  const now = useNow(1000);
  const until = timedOutUntil(member, now);
  const [reason, setReason] = useState("");
  const [seconds, setSeconds] = useState(TIME_OUT[3]!.value);
  const [purge, setPurge] = useState(0);
  const [nickname, setNick] = useState(member.nickname);
  const timeOut = useAction(timeOutMember);
  const kick = useAction(kickMember);
  const ban = useAction(banMember);
  const rename = useAction(setNickname);
  const busy = timeOut.pending || kick.pending || ban.pending || rename.pending;
  const error = timeOut.error ?? kick.error ?? ban.error ?? rename.error;
  const gavel = useAnimationControls();

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (action === "timeout") {
      if ((await timeOut.go(instanceKey, serverId, userId, seconds, reason.trim())) === undefined) return;
      toast(`${name} is timed out for ${formatDuration(seconds)}`);
    } else if (action === "kick") {
      if ((await kick.go(instanceKey, serverId, userId, reason.trim())) === undefined) return;
      toast(`Kicked ${name}`);
    } else if (action === "ban") {
      await gavel.start({ rotate: [0, -50, 20, 0], transition: { duration: 0.45, times: [0, 0.4, 0.7, 1] } });
      const deleted = await ban.go(instanceKey, serverId, userId, reason.trim(), purge);
      if (deleted === undefined) return;
      toast(deleted ? `Banned ${name} and deleted ${deleted} ${deleted === 1 ? "message" : "messages"}` : `Banned ${name}`);
    } else {
      if ((await rename.go(instanceKey, serverId, nickname.trim(), userId)) === undefined) return;
    }
    onDone();
  }

  async function endTimeOut() {
    if ((await timeOut.go(instanceKey, serverId, userId, 0, "")) === undefined) return;
    toast(`${name} can talk again`);
    onDone();
  }

  const title = { timeout: `Time out ${name}`, kick: `Kick ${name}`, ban: `Ban ${name}`, nickname: `Change ${name}'s nickname` }[action];
  const description = {
    timeout: "They can still read, but can't send or edit messages until it ends.",
    kick: "They leave the server. They can join again while it shows in Browse.",
    ban: "They leave the server and can't join again until someone unbans them.",
    nickname: "Only this server sees it. Leave it empty to show their display name.",
  }[action];

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <DialogHeader title={title} description={description} />
      <div className="flex items-center gap-3 rounded-2xl bg-muted/60 p-3">
        <UserAvatar user={member.user} className="size-10" />
        <span className="min-w-0 flex-1">
          <span className="block truncate font-bold">{name}</span>
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
              title={`Until ${formatStamp(until)}`}
            >
              <HourglassIcon className="size-3.5 animate-[spin_3s_ease-in-out_infinite]" /> {formatLeft(until.getTime() - now)}
            </motion.span>
          )}
        </AnimatePresence>
      </div>

      {action === "timeout" && (
        <div className="flex flex-col gap-2">
          <Label className="font-bold">For how long</Label>
          <Chips label="Time-out length" value={seconds} onChange={setSeconds} options={TIME_OUT} />
          <p className="text-xs text-muted-foreground">
            Ends <b>{formatStamp(new Date(now + seconds * 1000))}</b>
            {until && ", in place of the one running now"}.
          </p>
        </div>
      )}
      {action === "ban" && (
        <div className="flex flex-col gap-2">
          <Label className="font-bold">Delete their recent messages</Label>
          <Chips label="Delete messages from" value={purge} onChange={setPurge} options={DELETE} />
        </div>
      )}
      {action === "nickname" ? (
        <div className="flex flex-col gap-2">
          <Label htmlFor="mod-nickname" className="font-bold">
            Nickname
          </Label>
          <Input id="mod-nickname" autoFocus maxLength={32} value={nickname} placeholder={memberName({ ...member, nickname: "" })} onChange={(e) => setNick(e.target.value)} className="h-11 rounded-xl" />
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          <Label htmlFor="mod-reason" className="flex items-center justify-between font-bold">
            Reason <span className="text-xs font-normal text-muted-foreground">Optional, for the audit log</span>
          </Label>
          <Textarea id="mod-reason" rows={2} maxLength={REASON_MAX} value={reason} onChange={(e) => setReason(e.target.value)} className="rounded-xl" />
        </div>
      )}

      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
      <div className="flex flex-wrap items-center justify-end gap-2">
        {action === "timeout" && until && (
          <Button type="button" variant="ghost" disabled={busy} onClick={endTimeOut} className="mr-auto rounded-xl">
            <TimerOffIcon /> End time-out
          </Button>
        )}
        <Button type="button" variant="ghost" disabled={busy} onClick={onDone} className="rounded-xl">
          Cancel
        </Button>
        <Button
          type="submit"
          variant={action === "nickname" ? "default" : "destructive"}
          disabled={busy}
          className={cn("group rounded-xl font-bold", action === "nickname" && "btn")}
        >
          {busy ? (
            <LoaderCircleIcon className="animate-spin" />
          ) : action === "ban" ? (
            <motion.span animate={gavel} style={{ originX: 0.2, originY: 0.8 }} className="inline-flex transition-transform group-hover:-rotate-12">
              <GavelIcon />
            </motion.span>
          ) : action === "kick" ? (
            <DoorOpenIcon className="transition-transform group-hover:translate-x-0.5" />
          ) : action === "timeout" ? (
            <HourglassIcon className="transition-transform duration-500 group-hover:rotate-180" />
          ) : (
            <PencilIcon />
          )}
          {{ timeout: until ? "Change time-out" : "Time out", kick: "Kick", ban: "Ban", nickname: "Save" }[action]}
        </Button>
      </div>
    </form>
  );
}

/** What you may do to someone: what your permissions allow, and only to people ranked below you. */
export function useModeration(instanceKey: string, serverId: string, target: Member | undefined) {
  const access = useAccess(instanceKey, serverId);
  const roles = useRoles(instanceKey, serverId);
  const ownerId = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.ownerId ?? "");
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id);
  return moderationFor(access, ownerId, roles, meId, target);
}

/** What you may do to someone, from access already worked out: only to people ranked below you, never yourself. */
export function moderationFor(access: Access, ownerId: string, roles: readonly Role[], meId: string | undefined, target: Member | undefined) {
  const below = !!target?.user && target.user.id !== meId && outranks(access, standing(ownerId, roles, target));
  const can = (p: Permission) => below && has(access, p);
  const allowed: Record<ModAction, boolean> = {
    timeout: can(Permission.TIME_OUT_MEMBERS),
    kick: can(Permission.KICK_MEMBERS),
    ban: can(Permission.BAN_MEMBERS),
    nickname: can(Permission.MANAGE_NICKNAMES),
  };
  return { ...allowed, any: Object.values(allowed).some(Boolean) };
}
