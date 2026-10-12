import {
  CircleCheckIcon,
  CircleSlashIcon,
  CornerDownRightIcon,
  DoorOpenIcon,
  FlagIcon,
  GavelIcon,
  HourglassIcon,
  LoaderCircleIcon,
  RotateCcwIcon,
  Trash2Icon,
  UserRoundIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useEffect, useId, useState } from "react";
import type { GetModCaseResponse } from "@/gen/fuwa/v1/moderation_pb";
import { ModCaseStatus, Permission } from "@/gen/fuwa/v1/types_pb";
import { deleteMessage, run } from "@/fuwa/actions";
import { useAccess, useMember } from "@/fuwa/hooks";
import { closeModView, getCase, updateCase, watchMod } from "@/fuwa/moderation";
import { requestJump } from "@/fuwa/search";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { ModerateDialog, useModeration, type ModAction } from "@/components/ModerateDialog";
import { goTo } from "@/components/menus/common";
import { Count } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Handle, Heading, kindLook, MessageText, ReasonChips, StatusPill, useScope } from "@/components/modview/shared";
import { useI18n } from "@/i18n/react";
import { ago, displayName, formatDuration, formatStamp, timedOutUntil, toDate } from "@/lib/format";
import { AUTO_MOD, kindLabel, MEMBER_REPORT, OPEN, reasonLabel, RESOLUTION_MAX } from "@/lib/mod-view";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { useNow } from "@/lib/notifications";
import { hasIn } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * One case: what was said (kept as it was when it was caught or first
 * reported), the message as it is now, who reported it and why, and what a
 * moderator can do about it.
 */
export function CaseDetail({ caseId, onClose }: { caseId: string; onClose: () => void }) {
  const lang = useI18n();
  const { t } = lang;
  const { instanceKey, server, user, learn, openPerson, meId } = useScope();
  const [data, setData] = useState<GetModCaseResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    getCase(instanceKey, server.id, caseId)
      .then((res) => {
        learn(res.users);
        setData(res);
        setError(null);
      })
      .catch((err: Error) => setError(err.message));
  }, [instanceKey, server.id, caseId, learn]);

  useEffect(() => load(), [load]);
  // Reported again or closed by someone else: read again, for the reports.
  useEffect(() => watchMod(instanceKey, server.id, (e) => e.case === "modCaseUpdated" && e.value.modCase?.id === caseId && load()), [instanceKey, server.id, caseId, load]);

  if (error)
    return (
      <div className="flex items-center gap-2 rounded-2xl border p-4 text-sm text-destructive">
        <span className="flex-1 first-letter:uppercase">{error}</span>
        <Button type="button" variant="ghost" size="icon" onClick={onClose} aria-label={t("common.close")} className="rounded-full">
          <XIcon />
        </Button>
      </div>
    );
  if (!data?.modCase) return <div className="shimmer h-80 rounded-3xl" />;
  const mc = data.modCase;
  const look = kindLook(mc.kind);
  const subject = user(mc.subjectId);
  return (
    <article className="flex flex-col gap-4 rounded-3xl border bg-card/60 p-4">
      <header className="flex items-start gap-3">
        <motion.span
          initial={{ scale: 0.5, rotate: -20 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 520, damping: 16 }}
          className={cn("grid size-10 shrink-0 place-items-center rounded-xl", look.tint)}
        >
          <look.icon className="size-5" />
        </motion.span>
        <div className="min-w-0 flex-1">
          <p className="flex flex-wrap items-center gap-2 font-extrabold">
            {t(kindLabel(mc.kind))} <StatusPill status={mc.status} />
          </p>
          <p className="text-xs text-muted-foreground">
            {t("chattools.modview.opened", { time: formatStamp(toDate(mc.createdAt)) })}
          </p>
        </div>
      </header>

      <div className="flex items-center gap-3 rounded-2xl bg-muted/50 p-3">
        <UserAvatar user={subject} className="size-10" />
        <span className="min-w-0 flex-1">
          <span className="block truncate font-bold">{displayName(subject)}</span>
          <span className="flex text-xs text-muted-foreground">
            <Handle user={subject} me={mc.subjectId === meId} />
          </span>
        </span>
        <Button type="button" variant="outline" size="sm" onClick={() => openPerson(mc.subjectId)} className="group rounded-xl">
          <UserRoundIcon className="transition-transform group-hover:scale-110" /> {t("chattools.modview.viewMember")}
        </Button>
      </div>

      {mc.kind !== MEMBER_REPORT && <WhatItSaid data={data} />}
      {mc.kind === AUTO_MOD && mc.autoMod && (
        <section className="rounded-2xl border border-violet-500/30 bg-violet-500/5 p-3 text-sm">
          <p className="font-bold">{mc.autoMod.ruleName || t("chattools.modview.autoModRule")}</p>
          <p className="text-xs text-muted-foreground">
            {[
              mc.autoMod.blocked ? t("chattools.modview.wasBlocked") : t("chattools.modview.wasFlagged"),
              mc.autoMod.timedOutSeconds > 0 ? t("chattools.modview.timedOutFor", { duration: formatDuration(lang, mc.autoMod.timedOutSeconds) }) : "",
              mc.autoMod.matched.length ? t("chattools.modview.matched", { words: mc.autoMod.matched.join(", ") }) : "",
            ]
              .filter(Boolean)
              .join(" · ")}
          </p>
        </section>
      )}

      {data.reports.length > 0 && <Reports data={data} />}
      <Resolution data={data} onChanged={(next) => setData((d) => (d ? ({ ...d, modCase: next } as GetModCaseResponse) : d))} />
      <Shortcuts data={data} onDeleted={load} />
    </article>
  );
}

/** The case's copy of the message, the message now when it's changed, and a way to it. */
function WhatItSaid({ data }: { data: GetModCaseResponse }) {
  const { t } = useI18n();
  const { instanceKey, server } = useScope();
  const mc = data.modCase!;
  const channel = useFuwa((s) => s.instances[instanceKey]?.channels[server.id]?.find((c) => c.id === mc.channelId));
  const live = data.message;
  const edited = !!live && live.content !== mc.content;
  const gone = !!mc.messageId && !live;
  function jump() {
    if (!live) return;
    // A reply only in its thread is found under the message that started it.
    const target = live.threadId && !live.alsoInChannel ? live.threadId : live.id;
    requestJump(instanceKey, live.channelId, target);
    goTo({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: server.id, channel: live.channelId } });
    closeModView();
  }
  return (
    <section className="flex flex-col gap-2">
      <Heading
        icon={FlagIcon}
        aside={
          gone ? (
            <span className="text-[0.65rem] normal-case">{t("chattools.modview.deletedSince")}</span>
          ) : !mc.messageId ? (
            <span className="text-[0.65rem] normal-case">{t("chattools.modview.neverSent")}</span>
          ) : null
        }
      >
        {channel ? t("chattools.modview.saidIn", { channel: channel.name }) : t("chattools.modview.said")}
      </Heading>
      <blockquote className="rounded-2xl border-l-4 border-destructive/50 bg-muted/40 px-3 py-2 text-sm break-words">
        {mc.content ? <MessageText content={mc.content} emojis={live?.emojis} /> : <span className="text-muted-foreground italic">{t("chattools.modview.noText")}</span>}
      </blockquote>
      <AnimatePresence mode="popLayout" initial={false}>
        {edited && (
          <motion.div key="now" {...SLIDE_IN} transition={SPRING} className="flex flex-col gap-1">
            <p className="text-xs font-bold text-muted-foreground">{t("chattools.modview.editedSince")}</p>
            <blockquote className="rounded-2xl border-l-4 border-border bg-muted/30 px-3 py-2 text-sm break-words">
              {live.content ? <MessageText content={live.content} emojis={live.emojis} /> : <span className="text-muted-foreground italic">{t("chattools.modview.noText")}</span>}
            </blockquote>
          </motion.div>
        )}
      </AnimatePresence>
      {live && (
        <Button type="button" variant="outline" size="sm" onClick={jump} className="group self-start rounded-xl">
          <CornerDownRightIcon className="transition-transform group-hover:translate-x-0.5" /> {t("chattools.modview.jump")}
        </Button>
      )}
    </section>
  );
}

/** Who reported it, why and when, oldest first. */
function Reports({ data }: { data: GetModCaseResponse }) {
  const lang = useI18n();
  const { t } = lang;
  const { user } = useScope();
  const now = useNow();
  return (
    <section>
      <Heading icon={FlagIcon} aside={<Count value={data.reports.length} />}>
        {t("chattools.modview.reports")}
      </Heading>
      <ol className="flex flex-col gap-1.5">
        {data.reports.map((r, n) => (
          <motion.li
            key={r.reporterId}
            layout="position"
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: n * 0.03 } }}
            className="flex gap-2.5 rounded-2xl bg-muted/40 p-2.5"
          >
            <UserAvatar user={user(r.reporterId)} className="size-7" />
            <div className="min-w-0 flex-1 text-sm">
              <p className="flex flex-wrap items-center gap-1.5">
                <b className="truncate">{displayName(user(r.reporterId))}</b>
                <span className="rounded-full bg-destructive/10 px-1.5 py-px text-[0.65rem] font-bold text-destructive">{t(reasonLabel(r.reason))}</span>
                <span className="ml-auto text-[0.7rem] text-muted-foreground">{ago(lang, toDate(r.createdAt), now)}</span>
              </p>
              {r.note && <p className="mt-0.5 text-sm break-words whitespace-pre-wrap text-foreground/85">{r.note}</p>}
            </div>
          </motion.li>
        ))}
      </ol>
    </section>
  );
}

/** Closing it (resolved or dismissed, with a reason if they like), or opening it again. */
function Resolution({ data, onChanged }: { data: GetModCaseResponse; onChanged: (next: NonNullable<GetModCaseResponse["modCase"]>) => void }) {
  const { t } = useI18n();
  const { instanceKey, server, user } = useScope();
  const id = useId();
  const mc = data.modCase!;
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState<ModCaseStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function change(status: ModCaseStatus) {
    setBusy(status);
    setError(null);
    try {
      const next = await updateCase(instanceKey, server.id, mc.id, status, status === ModCaseStatus.OPEN ? "" : reason.trim());
      if (next) onChanged(next);
      setReason("");
      toast(
        status === ModCaseStatus.RESOLVED
          ? t("chattools.modview.resolvedToast")
          : status === ModCaseStatus.DISMISSED
            ? t("chattools.modview.dismissedToast")
            : t("chattools.modview.reopenedToast"),
      );
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  }

  const open = mc.status === OPEN;
  return (
    <section className="flex flex-col gap-2 border-t pt-4">
      <AnimatePresence mode="popLayout" initial={false}>
        {open ? (
          <motion.div key="open" {...SLIDE_IN} transition={SPRING} className="flex flex-col gap-2">
            <label htmlFor={`${id}-reason`} className="flex items-center justify-between text-sm font-bold">
              {t("chattools.modview.resolution")}
              <span className="text-xs font-normal text-muted-foreground">{t("chattools.modview.resolutionHint")}</span>
            </label>
            <Textarea id={`${id}-reason`} rows={2} maxLength={RESOLUTION_MAX} value={reason} onChange={(e) => setReason(e.target.value)} className="rounded-xl" />
            <div className="flex flex-wrap gap-2">
              <Button type="button" disabled={!!busy} onClick={() => void change(ModCaseStatus.RESOLVED)} className="group btn rounded-xl font-bold">
                {busy === ModCaseStatus.RESOLVED ? <LoaderCircleIcon className="animate-spin" /> : <CircleCheckIcon className="transition-transform group-hover:scale-110" />}
                {t("chattools.modview.resolve")}
              </Button>
              <Button type="button" variant="outline" disabled={!!busy} onClick={() => void change(ModCaseStatus.DISMISSED)} className="group rounded-xl">
                {busy === ModCaseStatus.DISMISSED ? <LoaderCircleIcon className="animate-spin" /> : <CircleSlashIcon className="transition-transform group-hover:rotate-45" />}
                {t("chattools.modview.dismiss")}
              </Button>
            </div>
          </motion.div>
        ) : (
          <motion.div key="closed" {...SLIDE_IN} transition={SPRING} className="flex flex-col gap-2">
            <p className="text-sm">
              {t(mc.status === ModCaseStatus.RESOLVED ? "chattools.modview.resolvedBy" : "chattools.modview.dismissedBy", {
                name: displayName(user(mc.closedById)),
                time: formatStamp(toDate(mc.closedAt)),
              })}
            </p>
            {mc.resolution && <p className="rounded-xl bg-muted/50 px-3 py-2 text-sm break-words whitespace-pre-wrap">{mc.resolution}</p>}
            <Button type="button" variant="outline" disabled={!!busy} onClick={() => void change(ModCaseStatus.OPEN)} className="group self-start rounded-xl">
              {busy === ModCaseStatus.OPEN ? <LoaderCircleIcon className="animate-spin" /> : <RotateCcwIcon className="transition-transform duration-500 group-hover:-rotate-180" />}
              {t("chattools.modview.reopen")}
            </Button>
          </motion.div>
        )}
      </AnimatePresence>
      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
    </section>
  );
}

const MOD_BUTTONS: { action: ModAction; icon: typeof HourglassIcon; label: "workspace.popover.timeout" | "workspace.popover.kick" | "workspace.popover.ban" }[] = [
  { action: "timeout", icon: HourglassIcon, label: "workspace.popover.timeout" },
  { action: "kick", icon: DoorOpenIcon, label: "workspace.popover.kick" },
  { action: "ban", icon: GavelIcon, label: "workspace.popover.ban" },
];

/** The moderation you may do from here: time out, kick or ban its subject, delete the message. Each only with its permission. */
function Shortcuts({ data, onDeleted }: { data: GetModCaseResponse; onDeleted: () => void }) {
  const { t } = useI18n();
  const { instanceKey, server } = useScope();
  const mc = data.modCase!;
  const member = useMember(instanceKey, server.id, mc.subjectId);
  const allowed = useModeration(instanceKey, server.id, member);
  const access = useAccess(instanceKey, server.id);
  const now = useNow();
  const [moderating, setModerating] = useState<ModAction | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const live = data.message;
  const canDelete = !!live && hasIn(access, live.channelId, Permission.MANAGE_MESSAGES);
  const buttons = MOD_BUTTONS.filter((b) => allowed[b.action]);
  if (!buttons.length && !canDelete) return null;

  async function remove() {
    if (!live) return;
    setDeleting(true);
    try {
      await run(deleteMessage(instanceKey, server.id, live.channelId, live.id));
      toast(t("chattools.modview.deleted"));
      onDeleted();
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setDeleting(false);
      setConfirming(false);
    }
  }

  return (
    <section className="flex flex-wrap gap-1.5">
      {member &&
        buttons.map(({ action, icon: Icon, label }) => (
          <Button key={action} type="button" variant="ghost" size="sm" onClick={() => setModerating(action)} className="group rounded-xl text-muted-foreground hover:bg-destructive/10 hover:text-destructive">
            <Icon className="transition-transform duration-300 group-hover:-rotate-12" />
            {action === "timeout" && timedOutUntil(member, now) ? t("workspace.popover.timedOut") : t(label)}
          </Button>
        ))}
      {canDelete && (
        <AnimatePresence mode="popLayout" initial={false}>
          {confirming ? (
            <motion.span key="sure" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={SPRING} className="flex gap-1">
              <Button type="button" variant="destructive" size="sm" disabled={deleting} onClick={() => void remove()} className="rounded-xl font-bold">
                {deleting ? <LoaderCircleIcon className="animate-spin" /> : <Trash2Icon />} {t("chattools.modview.deleteSure")}
              </Button>
              <Button type="button" variant="ghost" size="sm" onClick={() => setConfirming(false)} className="rounded-xl">
                {t("common.cancel")}
              </Button>
            </motion.span>
          ) : (
            <motion.span key="ask" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={SPRING}>
              <Button type="button" variant="ghost" size="sm" onClick={() => setConfirming(true)} className="group rounded-xl text-muted-foreground hover:bg-destructive/10 hover:text-destructive">
                <Trash2Icon className="transition-transform group-hover:-rotate-12" /> {t("chattools.modview.deleteMessage")}
              </Button>
            </motion.span>
          )}
        </AnimatePresence>
      )}
      {member && <ModerateDialog instanceKey={instanceKey} serverId={server.id} member={moderating ? member : null} action={moderating} onClose={() => setModerating(null)} />}
    </section>
  );
}
