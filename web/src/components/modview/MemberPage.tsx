import {
  CakeIcon,
  CalendarIcon,
  ClipboardListIcon,
  CornerDownRightIcon,
  DoorOpenIcon,
  GavelIcon,
  HistoryIcon,
  HourglassIcon,
  LoaderCircleIcon,
  MessagesSquareIcon,
  NotebookPenIcon,
  ScrollTextIcon,
  Trash2Icon,
  UserRoundXIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useEffect, useId, useState, type FormEvent, type ReactNode } from "react";
import type { GetMemberOverviewResponse } from "@/gen/fuwa/v1/moderation_pb";
import { Permission, type Message, type ModCase, type ModNote } from "@/gen/fuwa/v1/types_pb";
import { useAccess, useMember, useRoles } from "@/fuwa/hooks";
import { addNote, closeModView, deleteNote, listCases, memberMessages, memberOverview, watchMod } from "@/fuwa/moderation";
import { requestJump } from "@/fuwa/search";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { Markdown } from "@/components/Markdown";
import { MemberRoles } from "@/components/MemberRoles";
import { ModerateDialog, useModeration, type ModAction } from "@/components/ModerateDialog";
import { goTo } from "@/components/menus/common";
import { Count } from "@/components/motion";
import { AuditLogEntry } from "@/components/settings/server/AuditLog";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { Empty, Handle, Heading, kindLook, ReasonChips, StatusPill, useScope } from "@/components/modview/shared";
import { useI18n } from "@/i18n/react";
import { ago, displayName, formatStamp, memberName, timedOutUntil, toDate } from "@/lib/format";
import { daysSince, kindLabel, NOTE_MAX, snippet, withNote } from "@/lib/mod-view";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { useNow } from "@/lib/notifications";
import { has } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * Everything about one person in the server, for a moderator: who they are
 * here, how long they've been around, where they stand (timed out, waiting
 * on the rules, banned), their cases, what moderators did to them, their
 * messages, and the moderators' notes about them. Works for people who left
 * or were banned.
 */
export function MemberPage({ userId }: { userId: string }) {
  const { t } = useI18n();
  const { instanceKey, server, learn } = useScope();
  const [data, setData] = useState<GetMemberOverviewResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    memberOverview(instanceKey, server.id, userId)
      .then((res) => {
        learn([...res.users, ...(res.user ? [res.user] : [])]);
        setData(res);
        setError(null);
      })
      .catch((err: Error) => setError(err.message));
  }, [instanceKey, server.id, userId, learn]);
  useEffect(() => load(), [load]);

  // Their cases and notes change while it's open.
  useEffect(
    () =>
      watchMod(instanceKey, server.id, (e) => {
        if (e.case === "modCaseUpdated" && e.value.modCase?.subjectId === userId) load();
        if (e.case === "modNoteAdded" && e.value.note?.subjectId === userId) {
          const note = e.value.note;
          setData((d) => (d ? ({ ...d, notes: withNote(d.notes, note) } as GetMemberOverviewResponse) : d));
        }
        if (e.case === "modNoteDeleted" && e.value.subjectId === userId) {
          const id = e.value.noteId;
          setData((d) => (d ? ({ ...d, notes: d.notes.filter((n) => n.id !== id) } as GetMemberOverviewResponse) : d));
        }
      }),
    [instanceKey, server.id, userId, load],
  );

  if (error) return <p className="text-sm text-destructive first-letter:uppercase">{error}</p>;
  if (!data)
    return (
      <div className="flex flex-col gap-3">
        <div className="shimmer h-28 rounded-3xl" />
        <div className="shimmer h-20 rounded-2xl" />
        <div className="shimmer h-40 rounded-2xl" />
      </div>
    );
  return (
    <div className="flex flex-col gap-6">
      <Who data={data} userId={userId} />
      <Stats data={data} userId={userId} />
      <Notes data={data} userId={userId} onNotes={(notes) => setData((d) => (d ? ({ ...d, notes } as GetMemberOverviewResponse) : d))} />
      <Cases userId={userId} total={data.totalCases} />
      <History data={data} />
      <Messages userId={userId} count={Number(data.messageCount)} />
      {!data.user && <p className="text-sm text-muted-foreground">{t("chattools.modview.noAccount")}</p>}
    </div>
  );
}

const MOD_BUTTONS: { action: ModAction; icon: typeof HourglassIcon; label: "workspace.popover.timeout" | "workspace.popover.kick" | "workspace.popover.ban" }[] = [
  { action: "timeout", icon: HourglassIcon, label: "workspace.popover.timeout" },
  { action: "kick", icon: DoorOpenIcon, label: "workspace.popover.kick" },
  { action: "ban", icon: GavelIcon, label: "workspace.popover.ban" },
];

/** Their name, roles and where they stand, with the moderation you may do. */
function Who({ data, userId }: { data: GetMemberOverviewResponse; userId: string }) {
  const { t } = useI18n();
  const { instanceKey, server, user, meId } = useScope();
  const now = useNow(1000);
  // Their membership as it is now, so a time-out or a role given here shows at once.
  const live = useMember(instanceKey, server.id, userId);
  const member = live ?? data.member;
  const person = data.user ?? user(userId);
  const allowed = useModeration(instanceKey, server.id, member);
  const [moderating, setModerating] = useState<ModAction | null>(null);
  const until = timedOutUntil(member, now);
  const buttons = member ? MOD_BUTTONS.filter((b) => allowed[b.action]) : [];
  return (
    <section className="relative overflow-hidden rounded-3xl border bg-card/60 p-4">
      <div className="flex items-center gap-4">
        <motion.span initial={{ scale: 0.6, rotate: -12, opacity: 0 }} animate={{ scale: 1, rotate: 0, opacity: 1 }} transition={{ type: "spring", stiffness: 420, damping: 18 }}>
          <UserAvatar user={person} className="size-16 text-2xl" />
        </motion.span>
        <div className="min-w-0 flex-1">
          <p className="truncate text-xl font-extrabold">{member ? memberName(member) : displayName(person)}</p>
          <p className="flex min-w-0 gap-1.5 text-sm text-muted-foreground">
            <Handle user={person} me={userId === meId} />
            {member?.nickname && member.nickname !== displayName(person) && <span className="truncate">· {displayName(person)}</span>}
          </p>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap gap-1.5">
        <AnimatePresence initial={false}>
          {!member && !data.ban && (
            <Badge key="gone" tint="bg-muted text-muted-foreground" icon={<UserRoundXIcon />}>
              {t("chattools.modview.notMember")}
            </Badge>
          )}
          {until && (
            <Badge key="timeout" tint="bg-amber-500/15 text-amber-600 dark:text-amber-400" icon={<HourglassIcon />}>
              {t("chattools.modview.timedOutUntil", { time: formatStamp(until) })}
            </Badge>
          )}
          {member?.pending && (
            <Badge key="pending" tint="bg-sky-500/15 text-sky-600 dark:text-sky-400" icon={<ScrollTextIcon />}>
              {t("chattools.modview.pending")}
            </Badge>
          )}
          {data.ban && (
            <Badge key="ban" tint="bg-destructive/15 text-destructive" icon={<GavelIcon />}>
              {t("chattools.modview.bannedBy", { name: displayName(user(data.ban.bannedById)), time: formatStamp(toDate(data.ban.createdAt)) })}
            </Badge>
          )}
        </AnimatePresence>
      </div>
      {data.ban?.reason && (
        <p className="mt-2 rounded-xl bg-destructive/5 px-3 py-2 text-sm break-words">
          <span className="text-muted-foreground">{t("serversettings.audit.reason")} </span>
          {data.ban.reason}
        </p>
      )}
      {member && (
        <div className="mt-3">
          <MemberRoles instanceKey={instanceKey} member={member} />
        </div>
      )}
      {buttons.length > 0 && member && (
        <div className="mt-3 flex flex-wrap gap-1.5 border-t pt-3">
          {buttons.map(({ action, icon: Icon, label }) => (
            <Button key={action} type="button" variant="ghost" size="sm" onClick={() => setModerating(action)} className="group rounded-xl text-muted-foreground hover:bg-destructive/10 hover:text-destructive">
              <Icon className="transition-transform duration-300 group-hover:-rotate-12" />
              {action === "timeout" && until ? t("workspace.popover.timedOut") : t(label)}
            </Button>
          ))}
          <ModerateDialog instanceKey={instanceKey} serverId={server.id} member={moderating ? member : null} action={moderating} onClose={() => setModerating(null)} />
        </div>
      )}
    </section>
  );
}

function Badge({ tint, icon, children }: { tint: string; icon: ReactNode; children: ReactNode }) {
  return (
    <motion.span
      layout="position"
      initial={{ opacity: 0, scale: 0.7 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.7 }}
      transition={SPRING}
      className={cn("flex items-center gap-1 rounded-full px-2 py-1 text-xs font-bold [&_svg]:size-3.5", tint)}
    >
      {icon}
      {children}
    </motion.span>
  );
}

/** When they joined, how old their account is, their cases and their messages, at a glance. */
function Stats({ data, userId }: { data: GetMemberOverviewResponse; userId: string }) {
  const lang = useI18n();
  const { t, date } = lang;
  const { showCases } = useScope();
  const now = useNow();
  const joined = data.member?.joinedAt ? toDate(data.member.joinedAt) : null;
  const created = data.accountCreatedAt ? toDate(data.accountCreatedAt) : null;
  const day = (d: Date) => date(d, { year: "numeric", month: "short", day: "numeric" });
  const days = created ? daysSince(created, now) : 0;
  return (
    <section className="grid grid-cols-2 gap-2 sm:grid-cols-4">
      <Stat icon={CalendarIcon} label={t("chattools.modview.joined")} value={joined ? day(joined) : "—"} hint={joined ? ago(lang, joined, now) : undefined} index={0} />
      <Stat
        icon={CakeIcon}
        label={t("chattools.modview.accountAge")}
        value={created ? t("chattools.modview.days", { count: days }) : "—"}
        hint={created ? day(created) : undefined}
        warn={!!created && days < 7}
        index={1}
      />
      <Stat
        icon={ClipboardListIcon}
        label={t("chattools.modview.cases")}
        value={
          <>
            <Count value={data.openCases} /> / <Count value={data.totalCases} />
          </>
        }
        hint={t("chattools.modview.openOfTotal")}
        onClick={data.totalCases > 0 ? () => showCases(userId) : undefined}
        warn={data.openCases > 0}
        index={2}
      />
      <Stat icon={MessagesSquareIcon} label={t("chattools.modview.messages")} value={<Count value={Number(data.messageCount)} />} index={3} />
    </section>
  );
}

function Stat({
  icon: Icon,
  label,
  value,
  hint,
  warn = false,
  onClick,
  index,
}: {
  icon: typeof CalendarIcon;
  label: string;
  value: ReactNode;
  hint?: string;
  warn?: boolean;
  onClick?: () => void;
  index: number;
}) {
  const body = (
    <>
      <span className="flex items-center gap-1.5 text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">
        <Icon className="size-3.5" /> {label}
      </span>
      <span className={cn("mt-1 block truncate text-lg font-extrabold", warn && "text-amber-600 dark:text-amber-400")}>{value}</span>
      {hint && <span className="block truncate text-xs text-muted-foreground">{hint}</span>}
    </>
  );
  const motionProps = {
    initial: { opacity: 0, y: 8 },
    animate: { opacity: 1, y: 0, transition: { ...SPRING, delay: index * 0.04 } },
  };
  return onClick ? (
    <motion.button type="button" {...motionProps} whileTap={{ scale: 0.97 }} onClick={onClick} className="rounded-2xl border bg-background/40 p-3 text-left transition hover:border-primary/40 hover:bg-muted/30">
      {body}
    </motion.button>
  ) : (
    <motion.div {...motionProps} className="rounded-2xl border bg-background/40 p-3">
      {body}
    </motion.div>
  );
}

/** The moderators' notes about them: add one, and delete your own (or any, with Manage Server). */
function Notes({ data, userId, onNotes }: { data: GetMemberOverviewResponse; userId: string; onNotes: (notes: ModNote[]) => void }) {
  const lang = useI18n();
  const { t } = lang;
  const { instanceKey, server, user, meId } = useScope();
  const access = useAccess(instanceKey, server.id);
  const manage = has(access, Permission.MANAGE_SERVER);
  const id = useId();
  const now = useNow();
  const [draft, setDraft] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function add(e: FormEvent) {
    e.preventDefault();
    const content = draft.trim();
    if (!content || busy) return;
    setBusy(true);
    setError(null);
    try {
      const note = await addNote(instanceKey, server.id, userId, content);
      if (note) onNotes(withNote(data.notes, note));
      setDraft("");
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  async function remove(note: ModNote) {
    try {
      await deleteNote(instanceKey, server.id, note.id);
      onNotes(data.notes.filter((n) => n.id !== note.id));
    } catch (err) {
      toast((err as Error).message);
    }
  }

  return (
    <section>
      <Heading icon={NotebookPenIcon} aside={<Count value={data.notes.length} />}>
        {t("chattools.modview.notes")}
      </Heading>
      <form onSubmit={(e) => void add(e)} className="flex flex-col gap-2">
        <Textarea
          id={`${id}-note`}
          aria-label={t("chattools.modview.addNote")}
          rows={2}
          maxLength={NOTE_MAX}
          value={draft}
          placeholder={t("chattools.modview.notePlaceholder")}
          onChange={(e) => setDraft(e.target.value)}
          className="rounded-xl"
        />
        <div className="flex items-center gap-2">
          <span className="flex-1 text-xs text-muted-foreground">{t("chattools.modview.notesPrivate")}</span>
          <span className="text-xs text-muted-foreground tabular-nums">
            <Count value={NOTE_MAX - draft.length} />
          </span>
          <Button type="submit" size="sm" disabled={!draft.trim() || busy} className="btn rounded-xl font-bold">
            {busy ? <LoaderCircleIcon className="animate-spin" /> : <NotebookPenIcon />} {t("chattools.modview.addNote")}
          </Button>
        </div>
        {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
      </form>
      <ol className="mt-3 flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {data.notes.map((note) => (
            <motion.li
              key={note.id}
              layout="position"
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, scale: 0.96 }}
              transition={SPRING}
              className="group flex gap-2.5 rounded-2xl bg-muted/40 p-2.5"
            >
              <UserAvatar user={user(note.authorId)} className="size-7" />
              <div className="min-w-0 flex-1 text-sm">
                <p className="flex items-center gap-1.5">
                  <b className="truncate">{displayName(user(note.authorId))}</b>
                  <span className="text-[0.7rem] text-muted-foreground" title={formatStamp(toDate(note.createdAt))}>
                    {ago(lang, toDate(note.createdAt), now)}
                  </span>
                </p>
                <p className="mt-0.5 break-words whitespace-pre-wrap">{note.content}</p>
              </div>
              {(note.authorId === meId || manage) && (
                <button
                  type="button"
                  onClick={() => void remove(note)}
                  aria-label={t("chattools.modview.deleteNote")}
                  title={t("chattools.modview.deleteNote")}
                  className="grid size-7 shrink-0 place-items-center rounded-lg text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:bg-destructive/10 hover:text-destructive focus-visible:opacity-100"
                >
                  <Trash2Icon className="size-3.5" />
                </button>
              )}
            </motion.li>
          ))}
        </AnimatePresence>
      </ol>
    </section>
  );
}

/** The latest cases about them; all of them open in the queue. */
function Cases({ userId, total }: { userId: string; total: number }) {
  const lang = useI18n();
  const { t } = lang;
  const { instanceKey, server, learn, openCase, showCases } = useScope();
  const now = useNow();
  const [cases, setCases] = useState<ModCase[] | null>(null);
  useEffect(() => {
    let live = true;
    listCases(instanceKey, server.id, { status: 0, subjectId: userId, limit: 5 })
      .then((res) => {
        if (!live) return;
        learn(res.users);
        setCases(res.cases);
      })
      .catch(() => live && setCases([]));
    return () => {
      live = false;
    };
  }, [instanceKey, server.id, userId, learn, total]);
  if (!total && !cases?.length) return null;
  return (
    <section>
      <Heading
        icon={ClipboardListIcon}
        aside={
          total > 0 && (
            <button type="button" onClick={() => showCases(userId)} className="text-[0.7rem] font-bold text-primary normal-case hover:underline">
              {t("chattools.modview.seeAllCases")}
            </button>
          )
        }
      >
        {t("chattools.modview.theirCases")}
      </Heading>
      {!cases ? (
        <div className="shimmer h-24 rounded-2xl" />
      ) : (
        <ol className="flex flex-col gap-1.5">
          {cases.map((c, n) => {
            const look = kindLook(c.kind);
            return (
              <motion.li key={c.id} initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: n * 0.03 } }}>
                <button type="button" onClick={() => openCase(c.id)} className="group flex w-full items-center gap-2.5 rounded-2xl border bg-background/40 p-2.5 text-left transition hover:border-primary/30 hover:bg-muted/30">
                  <span className={cn("grid size-8 shrink-0 place-items-center rounded-lg transition-transform group-hover:scale-110", look.tint)}>
                    <look.icon className="size-4" />
                  </span>
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center gap-1.5 text-sm">
                      <b className="truncate">{t(kindLabel(c.kind))}</b>
                      <StatusPill status={c.status} />
                      <span className="ml-auto shrink-0 text-[0.7rem] text-muted-foreground">{ago(lang, toDate(c.updatedAt), now)}</span>
                    </span>
                    {c.content && <span className="block truncate text-xs text-muted-foreground">{snippet(c.content, 100)}</span>}
                    <ReasonChips reasons={c.reasons} className="mt-1" />
                  </span>
                </button>
              </motion.li>
            );
          })}
        </ol>
      )}
    </section>
  );
}

/** What moderators, and AutoMod, did to them: the audit log's entries about them. */
function History({ data }: { data: GetMemberOverviewResponse }) {
  const { t } = useI18n();
  const { instanceKey, server, user } = useScope();
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[server.id]) ?? [];
  const roles = useRoles(instanceKey, server.id);
  const [open, setOpen] = useState<string | null>(null);
  const users = Object.fromEntries(
    [...new Set(data.history.flatMap((e) => [e.actorId, e.targetId]))].flatMap((id) => {
      const u = user(id);
      return u ? [[id, u]] : [];
    }),
  );
  return (
    <section>
      <Heading icon={HistoryIcon}>{t("chattools.modview.history")}</Heading>
      {data.history.length ? (
        <ol className="flex flex-col gap-1.5">
          <AnimatePresence initial={false}>
            {data.history.map((entry, n) => (
              <AuditLogEntry
                key={entry.id}
                entry={entry}
                index={n}
                users={users}
                channels={channels}
                roles={roles}
                open={open === entry.id}
                onToggle={() => setOpen(open === entry.id ? null : entry.id)}
              />
            ))}
          </AnimatePresence>
        </ol>
      ) : (
        <p className="rounded-2xl border border-dashed p-4 text-center text-sm text-muted-foreground">{t("chattools.modview.noHistory")}</p>
      )}
    </section>
  );
}

/** What they wrote, newest first, in the channels you can see, a page at a time. */
function Messages({ userId, count }: { userId: string; count: number }) {
  const lang = useI18n();
  const { t } = lang;
  const { instanceKey, server, learn } = useScope();
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[server.id]);
  const now = useNow();
  const [messages, setMessages] = useState<Message[] | null>(null);
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(
    (beforeId = "") => {
      setLoading(true);
      memberMessages(instanceKey, server.id, userId, beforeId)
        .then((res) => {
          learn(res.authors);
          setMessages((had) => (beforeId && had ? [...had, ...res.messages.filter((m) => !had.some((h) => h.id === m.id))] : res.messages));
          setMore(res.hasMore);
          setError(null);
        })
        .catch((err: Error) => setError(err.message))
        .finally(() => setLoading(false));
    },
    [instanceKey, server.id, userId, learn],
  );
  useEffect(() => load(), [load]);

  function jump(message: Message) {
    const target = message.threadId && !message.alsoInChannel ? message.threadId : message.id;
    requestJump(instanceKey, message.channelId, target);
    goTo({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: server.id, channel: message.channelId } });
    closeModView();
  }

  return (
    <section>
      <Heading icon={MessagesSquareIcon} aside={<Count value={count} />}>
        {t("chattools.modview.recentMessages")}
      </Heading>
      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
      {!messages && !error && <div className="shimmer h-32 rounded-2xl" />}
      {messages && !messages.length && <Empty icon={MessagesSquareIcon} title={t("chattools.modview.noMessages")} />}
      {messages && messages.length > 0 && (
        <ol className="flex flex-col gap-1.5">
          {messages.map((m, n) => {
            const channel = channels?.find((c) => c.id === m.channelId);
            return (
              <motion.li
                key={m.id}
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: Math.min(n % 50, 14) * 0.02 } }}
                className="group rounded-2xl border bg-background/40 p-3"
              >
                <p className="mb-1 flex items-center gap-1.5 text-xs text-muted-foreground">
                  {channel && <b className="truncate text-foreground/80">#{channel.name}</b>}
                  <span title={formatStamp(toDate(m.createdAt))}>{ago(lang, toDate(m.createdAt), now)}</span>
                  <button
                    type="button"
                    onClick={() => jump(m)}
                    className="ml-auto flex items-center gap-1 rounded-lg px-1.5 py-0.5 font-bold text-primary opacity-0 transition group-hover:opacity-100 hover:bg-primary/10 focus-visible:opacity-100"
                  >
                    <CornerDownRightIcon className="size-3.5" /> {t("chattools.modview.jump")}
                  </button>
                </p>
                {m.content ? <Markdown className="text-sm break-words">{m.content}</Markdown> : <p className="text-sm text-muted-foreground italic">{t("chattools.modview.noText")}</p>}
              </motion.li>
            );
          })}
        </ol>
      )}
      <AnimatePresence initial={false}>
        {more && messages && (
          <motion.div key="more" {...SLIDE_IN} transition={SPRING} className="mt-2 flex justify-center">
            <Button type="button" variant="outline" disabled={loading} onClick={() => load(messages.at(-1)?.id)} className="rounded-xl">
              {loading && <LoaderCircleIcon className="animate-spin" />} {t("chattools.modview.older")}
            </Button>
          </motion.div>
        )}
      </AnimatePresence>
    </section>
  );
}
