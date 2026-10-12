import { ArrowLeftIcon, CircleCheckIcon, CircleSlashIcon, FilterXIcon, InboxIcon, LayersIcon, LoaderCircleIcon, ShieldCheckIcon, UserRoundIcon } from "lucide-react";
import { AnimatePresence, LayoutGroup, m as motion } from "motion/react";
import { useCallback, useEffect, useId, useMemo, useState } from "react";
import type { Emoji, Member, ModCase, Server, User } from "@/gen/fuwa/v1/types_pb";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { useAccess, useRoles } from "@/fuwa/hooks";
import { ServerLookProvider, type ServerLook } from "@/components/chat/mentions";
import { closeModView, listCases, useModView, useOpenCases, watchMod, type ModViewPlace } from "@/fuwa/moderation";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { SettingsScreen, type SettingsGroup } from "@/components/settings/SettingsScreen";
import { Button } from "@/components/ui/button";
import { CaseDetail } from "@/components/modview/CaseDetail";
import { MemberPage } from "@/components/modview/MemberPage";
import { Empty, kindLook, ModViewContext, ReasonChips, Shimmer, StatusPill, useScope, type ModViewScope } from "@/components/modview/shared";
import { useI18n } from "@/i18n/react";
import { instanceHas } from "@/lib/compat";
import { ago, displayName, toDate } from "@/lib/format";
import { AUTO_MOD, isTab, kindLabel, MEMBER_REPORT, snippet, statusOfTab, TABS, withCase, type QueueTab } from "@/lib/mod-view";
import { SPRING } from "@/lib/motion";
import { useNow } from "@/lib/notifications";
import { has } from "@/lib/permissions";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

/*
 * Mod View (docs/mod-view.md): a server's moderators work through the cases
 * AutoMod and members' reports open, and look into one person. It takes
 * over the window like server settings: the queue's tabs, and the person
 * being looked at, on the side; the list and the chosen case beside them.
 * It only shows things: acting on someone goes through the same dialogs and
 * permissions as everywhere else.
 */

const NO_MEMBERS: Member[] = [];
const NO_EMOJI: Emoji[] = [];

const TAB_ICONS = { open: InboxIcon, resolved: CircleCheckIcon, dismissed: CircleSlashIcon, all: LayersIcon } as const;
const TAB_LABELS = {
  open: "chattools.modview.tab.open",
  resolved: "chattools.modview.tab.resolved",
  dismissed: "chattools.modview.tab.dismissed",
  all: "chattools.modview.tab.all",
} as const;

/** Mounted once (components/menus/MenuDialogs.tsx); shows Mod View where `openModView` last asked. */
export function ModViewHost({ open }: { open: boolean }) {
  const { last } = useModView();
  if (!last) return null;
  return <ModView key={`${last.instanceKey}|${last.server.id}`} open={open} instanceKey={last.instanceKey} server={last.server} place={last.place} n={last.n} />;
}

function ModView({ open, instanceKey, server: given, place, n }: { open: boolean; instanceKey: string; server: Server; place: ModViewPlace; n: number }) {
  const { t } = useI18n();
  const access = useAccess(instanceKey, given.id);
  const server = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === given.id)) ?? given;
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id ?? "");
  const me = useFuwa((s) => s.instances[instanceKey]?.me ?? undefined);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[given.id] ?? NO_MEMBERS);
  const emojis = useFuwa((s) => s.instances[instanceKey]?.emojis[given.id] ?? NO_EMOJI);
  const roles = useRoles(instanceKey, given.id);
  const known = useFuwa((s) => s.instances[instanceKey]?.users);
  const here = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "mod-view"));
  const allowed = here && has(access, Permission.MODERATE);
  const openCount = useOpenCases(instanceKey, server.id, allowed && open);

  const [section, setSection] = useState<string>("open");
  const [subject, setSubject] = useState("");
  const [person, setPerson] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [users, setUsers] = useState<Record<string, User>>({});

  // Each time it's opened: at the place it was asked for.
  const [applied, setApplied] = useState(0);
  if (applied !== n) {
    setApplied(n);
    setSubject("");
    setSelected(place.caseId ?? null);
    if (place.userId) {
      setPerson(place.userId);
      setSection("person");
    } else {
      setSection(place.tab && isTab(place.tab) ? place.tab : place.caseId ? "all" : "open");
    }
  }

  // Losing Use Mod View (or the server) closes it.
  useEffect(() => {
    if (open && !allowed) closeModView();
  }, [open, allowed]);

  const learn = useCallback((list: readonly User[]) => {
    if (!list.length) return;
    setUsers((had) => ({ ...had, ...Object.fromEntries(list.map((u) => [u.id, u])) }));
  }, []);
  const user = useCallback((id: string) => known?.[id] ?? users[id], [known, users]);

  const scope = useMemo<ModViewScope>(
    () => ({
      instanceKey,
      server,
      meId,
      user,
      learn,
      openPerson: (id) => {
        setPerson(id);
        setSection("person");
      },
      showCases: (id) => {
        setSubject(id);
        setSelected(null);
        setSection("all");
      },
      openCase: (id) => {
        setSelected(id);
        setSection("all");
      },
    }),
    [instanceKey, server, meId, user, learn],
  );

  // Messages' mentions and emoji, drawn as chat draws them.
  const look = useMemo<ServerLook>(() => {
    const myRoles = members.find((m) => m.user?.id === me?.id)?.roleIds ?? [];
    return { instanceKey, ownerId: server.ownerId, roles, members, emojis, me: me ? { id: me.id, username: me.username, roleIds: myRoles } : undefined };
  }, [instanceKey, server.ownerId, roles, members, emojis, me]);

  const personName = person ? displayName(user(person)) : "";
  const groups: SettingsGroup[] = [
    {
      label: t("chattools.modview.queue"),
      sections: TABS.map((tab) => ({ id: tab, label: t(TAB_LABELS[tab]), icon: TAB_ICONS[tab], badge: tab === "open" ? openCount : undefined })),
    },
    ...(person ? [{ label: t("chattools.modview.people"), sections: [{ id: "person", label: personName, icon: UserRoundIcon }] }] : []),
  ];
  const tab = isTab(section) ? section : null;

  return (
    <ModViewContext.Provider value={scope}>
      <ServerLookProvider value={look}>
        <SettingsScreen
          open={open && allowed}
          onOpenChange={(o) => !o && closeModView()}
          title={t("chattools.modview.title")}
          subtitle={server.name}
          groups={groups}
          section={section}
          onSectionChange={setSection}
          openToSection
        >
          {tab ? (
            <Queue
              key={`${tab}|${subject}`}
              tab={tab}
              subjectId={subject}
              onClearSubject={() => setSubject("")}
              selected={selected}
              onSelect={setSelected}
            />
          ) : person ? (
            <MemberPage key={person} userId={person} />
          ) : null}
        </SettingsScreen>
      </ServerLookProvider>
    </ModViewContext.Provider>
  );
}

/** One tab of the queue: its cases newest first, a page at a time, and the chosen one beside them. */
function Queue({
  tab,
  subjectId,
  onClearSubject,
  selected,
  onSelect,
}: {
  tab: QueueTab;
  subjectId: string;
  onClearSubject: () => void;
  selected: string | null;
  onSelect: (id: string | null) => void;
}) {
  const { t } = useI18n();
  const { instanceKey, server, user, learn } = useScope();
  const side = useMediaQuery("(min-width: 1180px)");
  const filter = useMemo(() => ({ status: statusOfTab(tab), subjectId }), [tab, subjectId]);
  const [cases, setCases] = useState<ModCase[] | null>(null);
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const glide = useId();

  const load = useCallback(
    (beforeId = "") => {
      setLoading(true);
      listCases(instanceKey, server.id, { ...filter, beforeId })
        .then((res) => {
          learn(res.users);
          setCases((had) => (beforeId && had ? [...had, ...res.cases.filter((c) => !had.some((h) => h.id === c.id))] : res.cases));
          setMore(res.hasMore);
          setError(null);
        })
        .catch((err: Error) => setError(err.message))
        .finally(() => setLoading(false));
    },
    [instanceKey, server.id, filter, learn],
  );

  useEffect(() => load(), [load]);

  // Cases opened, reported again or closed while it's open.
  useEffect(
    () =>
      watchMod(instanceKey, server.id, (event) => {
        if (event.case !== "modCaseUpdated" || !event.value.modCase) return;
        const changed = event.value.modCase;
        setCases((list) => (list ? withCase(list, changed, filter, !more) : list));
      }),
    [instanceKey, server.id, filter, more],
  );

  const showList = side || !selected;
  const showDetail = !!selected;

  return (
    <div className={cn("flex flex-col gap-3", side && selected && "grid grid-cols-[minmax(0,22rem)_minmax(0,1fr)] items-start gap-5")}>
      {showList && (
        <div className="flex min-w-0 flex-col gap-2">
          <AnimatePresence mode="popLayout" initial={false}>
            {subjectId && (
              <motion.button
                key="subject"
                type="button"
                onClick={onClearSubject}
                initial={{ opacity: 0, scale: 0.9 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.9 }}
                transition={SPRING}
                className="group flex items-center gap-2 self-start rounded-full border bg-muted/50 py-1 pr-3 pl-1 text-xs font-bold transition hover:border-primary/40"
              >
                <UserAvatar user={user(subjectId)} className="size-5" />
                {t("chattools.modview.casesAbout", { name: displayName(user(subjectId)) })}
                <FilterXIcon className="size-3.5 text-muted-foreground transition-transform group-hover:rotate-12" />
              </motion.button>
            )}
          </AnimatePresence>
          {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
          {!cases && !error && <Shimmer />}
          {cases && !cases.length && (
            <Empty
              icon={tab === "open" ? ShieldCheckIcon : InboxIcon}
              title={tab === "open" ? t("chattools.modview.allClear") : t("chattools.modview.noCases")}
              hint={tab === "open" ? t("chattools.modview.allClearHint") : undefined}
            />
          )}
          {cases && cases.length > 0 && (
            <LayoutGroup id={glide}>
              <ol className="flex flex-col gap-1.5">
                <AnimatePresence initial={false}>
                  {cases.map((c, i) => (
                    <CaseRow key={c.id} mc={c} index={i} active={c.id === selected} onSelect={() => onSelect(c.id === selected && side ? null : c.id)} />
                  ))}
                </AnimatePresence>
              </ol>
            </LayoutGroup>
          )}
          {more && cases && (
            <Button type="button" variant="outline" disabled={loading} onClick={() => load(cases.at(-1)?.id)} className="self-center rounded-xl">
              {loading && <LoaderCircleIcon className="animate-spin" />} {t("chattools.modview.older")}
            </Button>
          )}
        </div>
      )}
      <AnimatePresence mode="popLayout" initial={false}>
        {showDetail && (
          <motion.div
            key={selected}
            initial={{ opacity: 0, x: 16 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: 16, transition: { duration: 0.15 } }}
            transition={SPRING}
            className={cn("min-w-0", side && "sticky top-4")}
          >
            {!side && (
              <button
                type="button"
                onClick={() => onSelect(null)}
                className="group mb-3 flex items-center gap-1.5 text-sm font-bold text-muted-foreground transition hover:text-foreground"
              >
                <ArrowLeftIcon className="size-4 transition-transform group-hover:-translate-x-0.5" /> {t("chattools.modview.backToQueue")}
              </button>
            )}
            <CaseDetail caseId={selected} onClose={() => onSelect(null)} />
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** A case in the queue: what kind, who it's about, where, what it said, who reported it and why, and when. */
function CaseRow({ mc, index, active, onSelect }: { mc: ModCase; index: number; active: boolean; onSelect: () => void }) {
  const lang = useI18n();
  const { t } = lang;
  const { instanceKey, server, user } = useScope();
  const now = useNow();
  const channel = useFuwa((s) => (mc.channelId ? s.instances[instanceKey]?.channels[server.id]?.find((c) => c.id === mc.channelId) : undefined));
  const look = kindLook(mc.kind);
  const subject = user(mc.subjectId);
  const text = snippet(mc.content);
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, x: -12 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 14) * 0.025 } }}
      exit={{ opacity: 0, scale: 0.96, transition: { duration: 0.15 } }}
      className="relative"
    >
      {active && <motion.span layoutId="mod-case-active" transition={SPRING} className="absolute inset-0 rounded-2xl border border-primary/50 bg-primary/10" />}
      <button
        type="button"
        onClick={onSelect}
        aria-current={active ? "true" : undefined}
        className={cn("group relative flex w-full items-start gap-3 rounded-2xl border p-3 text-left transition-colors", active ? "border-transparent" : "bg-background/40 hover:border-primary/30 hover:bg-muted/30")}
      >
        <span className={cn("relative grid size-9 shrink-0 place-items-center rounded-xl transition-transform duration-300 group-hover:scale-110", look.tint)} title={t(kindLabel(mc.kind))}>
          <look.icon className="size-4" />
          <UserAvatar user={subject} className="absolute -right-1.5 -bottom-1.5 size-5 ring-2 ring-background" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex min-w-0 items-center gap-1.5 text-sm">
            <b className="truncate">{displayName(subject)}</b>
            {channel && <span className="truncate text-xs text-muted-foreground">#{channel.name}</span>}
            <span className="ml-auto shrink-0 text-[0.7rem] text-muted-foreground" title={toDate(mc.updatedAt).toLocaleString(lang.locale)}>
              {ago(lang, toDate(mc.updatedAt), now)}
            </span>
          </span>
          {mc.kind !== MEMBER_REPORT && (
            <span className={cn("mt-0.5 line-clamp-2 text-xs break-words", text ? "text-foreground/80" : "text-muted-foreground italic")}>{text || t("chattools.modview.noText")}</span>
          )}
          <span className="mt-1.5 flex flex-wrap items-center gap-1">
            {mc.status !== 1 && <StatusPill status={mc.status} />}
            {mc.kind === AUTO_MOD && mc.autoMod && (
              <span className="rounded-full bg-violet-500/10 px-1.5 py-px text-[0.65rem] font-bold text-violet-600 dark:text-violet-400">
                {mc.autoMod.ruleName || t("chattools.modview.autoModRule")}
                {mc.autoMod.blocked && ` · ${t("chattools.modview.blocked")}`}
              </span>
            )}
            {mc.reportCount > 0 && (
              <span className="rounded-full bg-muted px-1.5 py-px text-[0.65rem] font-bold text-muted-foreground">
                <Count value={mc.reportCount} /> {t("chattools.modview.reportsWord", { count: mc.reportCount })}
              </span>
            )}
            <ReasonChips reasons={mc.reasons} />
          </span>
        </span>
      </button>
    </motion.li>
  );
}
