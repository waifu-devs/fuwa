import {
  BotIcon,
  CheckIcon,
  ChevronDownIcon,
  CopyIcon,
  EyeIcon,
  EyeOffIcon,
  KeyRoundIcon,
  LoaderCircleIcon,
  PlusIcon,
  RefreshCwIcon,
  ServerIcon,
  TerminalIcon,
  Trash2Icon,
  TriangleAlertIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import type { Agent } from "@/gen/fuwa/v1/agent_pb";
import { AgentCreation, type Server } from "@/gen/fuwa/v1/types_pb";
import { addAgent, createAgent, deleteAgent, listAgents, resetAgentToken, run, updateAgent } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { getInstance, useInstance } from "@/fuwa/hooks";
import { AppBadge } from "@/components/AppBadge";
import { UserAvatar } from "@/components/Icons";
import { PictureField } from "@/components/PictureField";
import { Private } from "@/components/Private";
import { Count } from "@/components/motion";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { T, useI18n } from "@/i18n/react";
import { ago, displayName, toDate } from "@/lib/format";
import { accessOf, has } from "@/lib/permissions";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { usePrefs } from "@/lib/prefs";
import { hidesPersonal } from "@/lib/streamer";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const BIO_MAX = 2000;
const USERNAME = /^[a-z0-9][a-z0-9_.]{1,31}$/;

/** The servers you manage, where you can add an agent. */
function managedServers(key: string): Server[] {
  const inst = getInstance(key);
  if (!inst?.me) return [];
  const me = inst.me;
  return inst.servers.filter((s) => {
    const member = inst.members[s.id]?.find((m) => m.user?.id === me.id);
    const access = accessOf(s.id, s.ownerId, inst.roles[s.id] ?? [], inst.channels[s.id] ?? [], me.id, member?.roleIds ?? []);
    return has(access, Permission.MANAGE_SERVER);
  });
}

/**
 * Your agents on this instance: accounts your programs drive (bots,
 * assistants, integrations). Each has a token it signs in with, and goes
 * into the servers you (or, when it's public, anyone managing one) add it to.
 */
export function Agents({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const creation = inst?.node?.agentCreation ?? AgentCreation.EVERYONE;
  const allowed = creation === AgentCreation.EVERYONE || (creation === AgentCreation.ADMINS && !!inst?.admin);
  const [agents, setAgents] = useState<Agent[] | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [making, setMaking] = useState(false);
  const [howTo, setHowTo] = useState(false);
  /** A token just made, shown until it's put away: only its hash is kept. */
  const [fresh, setFresh] = useState<{ agentId: string; token: string } | null>(null);

  useEffect(() => {
    run(listAgents(instanceKey)).then(setAgents, (err: FuwaError) => {
      toast(err.message);
      setAgents([]);
    });
  }, [instanceKey]);

  const replace = (a: Agent) => setAgents((list) => list?.map((x) => (x.user?.id === a.user?.id ? a : x)) ?? null);

  return (
    <div className="flex flex-col gap-5">
      <div className="relative overflow-hidden rounded-3xl border bg-gradient-to-br from-violet-500/10 via-primary/5 to-transparent p-5">
        <Circuit />
        <div className="relative flex flex-wrap items-center gap-4">
          <motion.span
            animate={{ y: [0, -4, 0], rotate: [0, -6, 6, 0] }}
            transition={{ duration: 3.2, repeat: Infinity, repeatDelay: 1.4 }}
            className="relative grid size-12 shrink-0 place-items-center rounded-2xl bg-violet-500/15 text-violet-500"
          >
            <BotIcon className="size-6" />
            <motion.span
              aria-hidden
              animate={{ opacity: [0, 1, 0], scale: [0.6, 1.2, 0.6] }}
              transition={{ duration: 1.8, repeat: Infinity, repeatDelay: 2 }}
              className="absolute -top-1 -right-1 size-2.5 rounded-full bg-emerald-400"
            />
          </motion.span>
          <div className="min-w-0 flex-1 basis-60">
            <p className="font-extrabold">{t("settings.nav.agents")}</p>
            <p className="text-sm text-muted-foreground">{t("accountsettings.agents.intro")}</p>
          </div>
          <Button type="button" className="btn rounded-xl font-bold" disabled={!allowed || !agents} onClick={() => setMaking((m) => !m)}>
            <motion.span animate={{ rotate: making ? 45 : 0 }} transition={SPRING} className="flex">
              <PlusIcon />
            </motion.span>
            {t("accountsettings.agents.new")}
          </Button>
        </div>
        {!allowed && (
          <p className="relative mt-3 text-sm font-bold text-muted-foreground">
            {creation === AgentCreation.ADMINS ? t("accountsettings.agents.adminsOnly") : t("accountsettings.agents.notAllowed")}
          </p>
        )}
      </div>

      <AnimatePresence mode="popLayout" initial={false}>
        {making && (
          <motion.div key="new" initial={{ ...SLIDE_IN.initial, scale: 0.97 }} animate={{ ...SLIDE_IN.animate, scale: 1 }} exit={{ ...SLIDE_IN.exit, scale: 0.97 }} transition={SPRING}>
            <NewAgent
              instanceKey={instanceKey}
              onCancel={() => setMaking(false)}
              onMade={(agent, token) => {
                setAgents((list) => [...(list ?? []), agent]);
                setMaking(false);
                setOpen(agent.user?.id ?? null);
                setFresh({ agentId: agent.user?.id ?? "", token });
              }}
            />
          </motion.div>
        )}
      </AnimatePresence>

      {!agents ? (
        <div className="flex flex-col gap-2">
          {[0, 1].map((n) => (
            <div key={n} className="shimmer h-16 rounded-2xl" />
          ))}
        </div>
      ) : agents.length === 0 ? (
        !making && (
          <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="flex flex-col items-center gap-2 py-8 text-center">
            <motion.span animate={{ y: [0, -6, 0], rotate: [0, -8, 8, 0] }} transition={{ duration: 2.4, repeat: Infinity, repeatDelay: 0.8 }} className="text-4xl">
              🤖
            </motion.span>
            <p className="font-bold">{t("accountsettings.agents.none")}</p>
            <p className="text-sm text-muted-foreground">{t("accountsettings.agents.noneHint")}</p>
          </motion.div>
        )
      ) : (
        <motion.ul layout="position" transition={SPRING} className="flex flex-col gap-2">
          <AnimatePresence initial={false}>
            {agents.map((a) => (
              <AgentCard
                key={a.user?.id}
                instanceKey={instanceKey}
                agent={a}
                token={fresh && fresh.agentId === a.user?.id ? fresh.token : null}
                open={open === a.user?.id}
                onToggle={() => setOpen((o) => (o === a.user?.id ? null : (a.user?.id ?? null)))}
                onChange={replace}
                onToken={(token) => setFresh({ agentId: a.user?.id ?? "", token })}
                onTokenSeen={() => setFresh(null)}
                onDelete={() => setAgents((list) => list?.filter((x) => x.user?.id !== a.user?.id) ?? null)}
              />
            ))}
          </AnimatePresence>
        </motion.ul>
      )}

      <motion.div layout="position" transition={SPRING} className="rounded-2xl border">
        <button type="button" onClick={() => setHowTo((h) => !h)} className="flex w-full items-center gap-2 p-3 text-left text-sm font-bold">
          <TerminalIcon className="size-4 text-primary" />
          <span className="flex-1">{t("accountsettings.agents.howTo")}</span>
          <ChevronDownIcon className={cn("size-4 text-muted-foreground transition-transform duration-300", howTo && "rotate-180")} />
        </button>
        <AnimatePresence mode="popLayout" initial={false}>
          {howTo && (
            <motion.div {...SLIDE_IN} transition={SPRING}>
              <div className="flex flex-col gap-2 px-3 pb-3 text-sm text-muted-foreground">
                <p>
                  <T
                    k="accountsettings.agents.howToApi"
                    values={{ subscribe: <code>EventService/Subscribe</code>, send: <code>MessageService/SendMessage</code> }}
                  />
                </p>
                <pre className="overflow-x-auto rounded-xl bg-muted p-3 font-mono text-xs leading-relaxed text-foreground">
                  {`grpcurl -H 'authorization: Bearer <token>' \\\n  -d '{"server_id": "…", "channel_id": "…", "content": "Hello! 🤖"}' \\\n  `}
                  <Private text={(inst?.url ?? "https://fuwa.example").replace(/^https?:\/\//, "")} />
                  {`:443 fuwa.v1.MessageService/SendMessage`}
                </pre>
                <p>{t("accountsettings.agents.howToRules")}</p>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </motion.div>
    </div>
  );
}

/** Faint traces with pulses running along them, behind the header. */
function Circuit() {
  const paths = ["M0 30 H120 L140 50 H260 L280 30 H400", "M0 75 H90 L110 60 H300 L320 75 H400"];
  return (
    <svg aria-hidden className="pointer-events-none absolute inset-0 size-full text-violet-500/20" preserveAspectRatio="none" viewBox="0 0 400 100">
      {paths.map((d, i) => (
        <g key={d}>
          <path d={d} fill="none" stroke="currentColor" strokeWidth="1" />
          <motion.circle
            r="2.5"
            className="fill-violet-500/60"
            initial={{ offsetDistance: "0%" }}
            animate={{ offsetDistance: "100%" }}
            transition={{ duration: 5 + i * 1.5, repeat: Infinity, ease: "linear", delay: i * 1.1 }}
            style={{ offsetPath: `path("${d}")` }}
          />
        </g>
      ))}
    </svg>
  );
}

/** Picking an agent's username and name. */
function NewAgent({ instanceKey, onCancel, onMade }: { instanceKey: string; onCancel: () => void; onMade: (agent: Agent, token: string) => void }) {
  const { t } = useI18n();
  const [displayName, setDisplayName] = useState("");
  const [username, setUsername] = useState("");
  const [edited, setEdited] = useState(false);
  const [busy, setBusy] = useState(false);
  const shake = useAnimationControls();
  // The username follows the name until it's typed on its own.
  const suggested = displayName.trim().toLowerCase().replace(/\s+/g, "_").replace(/[^a-z0-9_.]/g, "").replace(/^[_.]+/, "").slice(0, 32);
  const shownUsername = edited ? username : suggested;
  const valid = USERNAME.test(shownUsername) && displayName.trim().length > 0;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!valid) {
      void shake.start({ x: [0, -8, 6, -4, 0], transition: { duration: 0.35 } });
      return;
    }
    setBusy(true);
    try {
      const { agent, token } = await run(createAgent(instanceKey, shownUsername, displayName.trim()));
      onMade(agent, token);
    } catch (err) {
      toast((err as FuwaError).message);
      void shake.start({ x: [0, -8, 6, -4, 0], transition: { duration: 0.35 } });
    }
    setBusy(false);
  }

  return (
    <form onSubmit={(e) => void submit(e)}>
      <motion.div animate={shake} className="flex flex-col gap-3 rounded-2xl border border-primary/40 bg-background/60 p-4 shadow-lg shadow-primary/5">
        <div className="flex flex-wrap gap-3">
          <label className="flex min-w-0 flex-1 basis-48 flex-col gap-1">
            <span className="text-xs font-bold text-muted-foreground uppercase">{t("accountsettings.agents.name")}</span>
            <Input autoFocus value={displayName} maxLength={64} placeholder={t("accountsettings.agents.namePlaceholder")} onChange={(e) => setDisplayName(e.target.value)} className="h-10 rounded-xl font-bold" />
          </label>
          <label className="flex min-w-0 flex-1 basis-48 flex-col gap-1">
            <span className="text-xs font-bold text-muted-foreground uppercase">{t("accountsettings.agents.username")}</span>
            <span className="relative">
              <span className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted-foreground">@</span>
              <Input
                value={shownUsername}
                maxLength={32}
                placeholder="helper_bot"
                spellCheck={false}
                onChange={(e) => {
                  setEdited(true);
                  setUsername(e.target.value.toLowerCase());
                }}
                className={cn("h-10 rounded-xl pl-7 font-mono", shownUsername && !USERNAME.test(shownUsername) && "border-destructive/60")}
              />
            </span>
          </label>
        </div>
        <p className="text-xs text-muted-foreground">{t("accountsettings.agents.usernameRule")}</p>
        <div className="flex justify-end gap-2">
          <Button type="button" variant="ghost" className="rounded-xl" onClick={onCancel}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" className="btn rounded-xl font-bold" disabled={busy}>
            {busy ? <LoaderCircleIcon className="animate-spin" /> : <BotIcon />} {t("accountsettings.agents.make")}
          </Button>
        </div>
      </motion.div>
    </form>
  );
}

/** A token on screen: shown once, copied, then put away. Streamer mode keeps it blurred. */
function TokenReveal({ token, onDone }: { token: string; onDone: () => void }) {
  const { t } = useI18n();
  const streaming = usePrefs(hidesPersonal);
  const [shown, setShown] = useState(!streaming);
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (streaming) setShown(false);
  }, [streaming]);

  function copy() {
    void navigator.clipboard?.writeText(token).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      },
      () => toast(t("accountsettings.agents.copyFailed")),
    );
  }

  return (
    <div className="relative flex flex-col gap-2 overflow-hidden rounded-2xl border border-amber-500/40 bg-amber-500/10 p-3">
      <motion.span
        aria-hidden
        initial={{ x: "-100%" }}
        animate={{ x: "200%" }}
        transition={{ duration: 1.4, ease: "easeInOut", delay: 0.2 }}
        className="pointer-events-none absolute inset-y-0 w-1/3 bg-gradient-to-r from-transparent via-amber-300/30 to-transparent"
      />
      <p className="relative flex items-center gap-1.5 text-sm font-extrabold text-amber-600 dark:text-amber-400">
        <KeyRoundIcon className="size-4" /> {t("accountsettings.agents.copyNow")}
      </p>
      <div className="relative flex items-center gap-1 rounded-xl border bg-background/70 p-1 pl-3">
        <AnimatePresence mode="wait" initial={false}>
          <motion.code
            key={shown ? "shown" : "hidden"}
            initial={{ opacity: 0, filter: "blur(4px)" }}
            animate={{ opacity: 1, filter: "blur(0px)" }}
            exit={{ opacity: 0, filter: "blur(4px)" }}
            transition={{ duration: 0.18 }}
            className={cn("min-w-0 flex-1 font-mono text-xs", shown ? "break-all" : "truncate")}
          >
            {shown ? token : "•".repeat(32)}
          </motion.code>
        </AnimatePresence>
        <Button type="button" variant="ghost" size="icon" className="size-8 shrink-0 rounded-lg" aria-label={shown ? t("accountsettings.agents.hideToken") : t("accountsettings.agents.showToken")} onClick={() => setShown((s) => !s)}>
          {shown ? <EyeOffIcon /> : <EyeIcon />}
        </Button>
        <Button type="button" size="sm" className="btn h-8 shrink-0 rounded-lg px-3 font-bold" onClick={copy}>
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span key={copied ? "done" : "copy"} initial={{ y: 12, opacity: 0 }} animate={{ y: 0, opacity: 1 }} exit={{ y: -12, opacity: 0 }} transition={SPRING} className="flex items-center gap-1.5">
              {copied ? <CheckIcon strokeWidth={3} /> : <CopyIcon />} {copied ? t("accountsettings.shared.copied") : t("accountsettings.shared.copy")}
            </motion.span>
          </AnimatePresence>
        </Button>
      </div>
      <div className="relative flex items-center justify-between gap-2">
        <p className="text-xs text-muted-foreground">{t("accountsettings.agents.tokenWarning")}</p>
        <Button type="button" size="sm" variant="ghost" className="h-7 shrink-0 rounded-lg text-xs font-bold" onClick={onDone}>
          {t("accountsettings.shared.savedIt")}
        </Button>
      </div>
    </div>
  );
}

/** One agent: folded, who it is and where it's been; open, everything you can change about it. */
function AgentCard({
  instanceKey,
  agent: a,
  token,
  open,
  onToggle,
  onChange,
  onToken,
  onTokenSeen,
  onDelete,
}: {
  instanceKey: string;
  agent: Agent;
  token: string | null;
  open: boolean;
  onToggle: () => void;
  onChange: (a: Agent) => void;
  onToken: (token: string) => void;
  onTokenSeen: () => void;
  onDelete: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const user = a.user!;
  const [name, setName] = useState(user.displayName);
  const [bio, setBio] = useState(a.bio);
  const [busy, setBusy] = useState<null | "save" | "reset" | "add">(null);
  const [confirm, setConfirm] = useState<null | "reset" | "delete">(null);
  const [added, setAdded] = useState<string | null>(null);
  const shake = useAnimationControls();
  useEffect(() => setName(user.displayName), [user.displayName]);
  useEffect(() => setBio(a.bio), [a.bio]);
  const servers = useMemo(() => (open ? managedServers(instanceKey) : []), [open, instanceKey]);
  const inst = useInstance(instanceKey);

  async function save(change: { displayName?: string; avatarUrl?: string; bio?: string; public?: boolean }) {
    setBusy("save");
    try {
      onChange(await run(updateAgent(instanceKey, user.id, change)));
    } catch (err) {
      toast((err as FuwaError).message);
      setName(user.displayName);
      setBio(a.bio);
    }
    setBusy(null);
  }

  function rename() {
    const trimmed = name.trim();
    if (!trimmed) {
      void shake.start({ x: [0, -6, 5, -3, 0], transition: { duration: 0.35 } });
      return setName(user.displayName);
    }
    if (trimmed !== user.displayName) void save({ displayName: trimmed });
  }

  async function reset() {
    setConfirm(null);
    setBusy("reset");
    try {
      onToken(await run(resetAgentToken(instanceKey, user.id)));
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setBusy(null);
  }

  async function remove() {
    try {
      await run(deleteAgent(instanceKey, user.id));
      onDelete();
    } catch (err) {
      toast((err as FuwaError).message);
    }
  }

  async function addTo(server: Server) {
    setBusy("add");
    try {
      await run(addAgent(instanceKey, server.id, user.username));
      setAdded(server.id);
      setTimeout(() => setAdded(null), 1600);
      onChange({ ...a, servers: a.servers + 1 } as Agent);
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setBusy(null);
  }

  const inServer = (s: Server) => !!inst?.members[s.id]?.some((m) => m.user?.id === user.id);

  return (
    <motion.li
      layout
      initial={{ opacity: 0, y: 10, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.9, filter: "blur(4px)" }}
      transition={SPRING}
      className={cn("overflow-hidden rounded-2xl border bg-background/50 transition-colors", open ? "border-primary/40 shadow-lg shadow-primary/5" : "hover:border-primary/30")}
    >
      <AgentSummary agent={a} open={open} saving={busy === "save"} onToggle={onToggle} />

      <AnimatePresence mode="popLayout" initial={false}>
        {open && (
          <motion.div {...SLIDE_IN} transition={SPRING}>
            <div className="flex flex-col gap-4 border-t p-4">
              <AnimatePresence mode="popLayout" initial={false}>
                {token && (
                  <motion.div key={token} initial={{ ...SLIDE_IN.initial, scale: 0.97 }} animate={{ ...SLIDE_IN.animate, scale: 1 }} exit={{ ...SLIDE_IN.exit, scale: 0.97 }} transition={SPRING}>
                    <TokenReveal token={token} onDone={onTokenSeen} />
                  </motion.div>
                )}
              </AnimatePresence>

              <motion.div layout="position" transition={SPRING} className="flex flex-wrap items-start gap-4">
                <PictureField
                  instanceKey={instanceKey}
                  kind="avatar"
                  compact
                  value={user.avatarUrl}
                  onChange={(avatarUrl) => void save({ avatarUrl })}
                  fallback={<UserAvatar user={{ ...user, avatarUrl: "" } as typeof user} className="size-full text-2xl" />}
                />
                <div className="flex min-w-0 flex-1 basis-56 flex-col gap-3">
                  <label className="flex flex-col gap-1">
                    <span className="text-xs font-bold text-muted-foreground uppercase">{t("accountsettings.agents.name")}</span>
                    <motion.span animate={shake}>
                      <Input
                        value={name}
                        maxLength={64}
                        onChange={(e) => setName(e.target.value)}
                        onBlur={rename}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") e.currentTarget.blur();
                          if (e.key === "Escape") setName(user.displayName);
                        }}
                        className="h-9 rounded-xl font-bold"
                      />
                    </motion.span>
                  </label>
                  <label className="flex flex-col gap-1">
                    <span className="flex items-center justify-between text-xs font-bold text-muted-foreground uppercase">
                      {t("accountsettings.agents.about")}
                      <span className={cn("tabular-nums normal-case", bio.length > BIO_MAX * 0.9 && "text-amber-500")}>
                        <Count value={bio.length} /> / {BIO_MAX}
                      </span>
                    </span>
                    <Textarea
                      value={bio}
                      maxLength={BIO_MAX}
                      placeholder={t("accountsettings.agents.bioPlaceholder")}
                      onChange={(e) => setBio(e.target.value)}
                      onBlur={() => bio !== a.bio && void save({ bio })}
                      className="min-h-20 rounded-xl"
                    />
                  </label>
                </div>
              </motion.div>

              <motion.div layout="position" transition={SPRING}>
                <Toggle
                  checked={a.public}
                  onChange={(pub) => void save({ public: pub })}
                  label={t("accountsettings.agents.public")}
                  hint={t("accountsettings.agents.publicHint")}
                />
              </motion.div>

              <motion.div layout="position" transition={SPRING} className="flex flex-wrap items-center gap-2">
                <AddToServer servers={servers} busy={busy} added={added} inServer={inServer} onAdd={(server) => void addTo(server)} />
                <DangerActions confirm={confirm} busy={busy} onConfirm={setConfirm} onReset={() => void reset()} onRemove={() => void remove()} />
                <span className="ml-auto text-xs text-muted-foreground">{t("accountsettings.agents.made", { when: ago(lang, toDate(a.createdAt)) })}</span>
              </motion.div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** The folded card: who the agent is, whether it's been used lately, and where it's been. */
function AgentSummary({ agent: a, open, saving, onToggle }: { agent: Agent; open: boolean; saving: boolean; onToggle: () => void }) {
  const lang = useI18n();
  const { t } = lang;
  const user = a.user!;
  return (
    <button type="button" onClick={onToggle} aria-expanded={open} className="flex w-full items-center gap-3 p-3 text-left">
      <motion.span whileHover={{ rotate: -8, scale: 1.08 }} transition={{ type: "spring", stiffness: 600, damping: 14 }} className="relative">
        <UserAvatar user={user} className="size-10" />
        <AnimatePresence>
          {a.lastActiveAt && Date.now() - toDate(a.lastActiveAt).getTime() < 10 * 60_000 && (
            <motion.span
              initial={{ scale: 0 }}
              animate={{ scale: 1 }}
              exit={{ scale: 0 }}
              title={t("accountsettings.agents.recentlyUsed")}
              className="absolute -right-0.5 -bottom-0.5 size-3 rounded-full border-2 border-background bg-emerald-500"
            />
          )}
        </AnimatePresence>
      </motion.span>
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-1.5">
          <span className="truncate font-bold">{displayName(user)}</span>
          <AppBadge agent />
          {a.public && (
            <span className="rounded-full bg-muted px-1.5 py-px text-[0.6rem] font-bold text-muted-foreground uppercase">
              {t("accountsettings.agents.public")}
            </span>
          )}
        </span>
        <span className="block truncate text-xs text-muted-foreground">
          @{user.username} · <T k="accountsettings.agents.servers" values={{ count: <Count value={a.servers} /> }} count={a.servers} /> ·{" "}
          {a.lastActiveAt ? t("accountsettings.agents.active", { when: ago(lang, toDate(a.lastActiveAt)) }) : t("accountsettings.agents.neverSignedIn")}
        </span>
      </span>
      {saving && <LoaderCircleIcon className="size-4 animate-spin text-muted-foreground" />}
      <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
    </button>
  );
}

/** Adding the agent to a server you manage. */
function AddToServer({
  servers,
  busy,
  added,
  inServer,
  onAdd,
}: {
  servers: Server[];
  busy: null | "save" | "reset" | "add";
  added: string | null;
  inServer: (server: Server) => boolean;
  onAdd: (server: Server) => void;
}) {
  const { t } = useI18n();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button type="button" size="sm" className="btn rounded-xl font-bold" disabled={!!busy}>
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span
              key={added ? "added" : busy === "add" ? "adding" : "add"}
              initial={{ y: 10, opacity: 0 }}
              animate={{ y: 0, opacity: 1 }}
              exit={{ y: -10, opacity: 0 }}
              transition={SPRING}
              className="flex items-center gap-1.5"
            >
              {added ? <CheckIcon strokeWidth={3} /> : busy === "add" ? <LoaderCircleIcon className="animate-spin" /> : <ServerIcon />}
              {added ? t("accountsettings.agents.added") : t("accountsettings.agents.addToServer")}
            </motion.span>
          </AnimatePresence>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-72 w-64 overflow-y-auto">
        <DropdownMenuLabel className="text-xs text-muted-foreground">{t("accountsettings.agents.managed")}</DropdownMenuLabel>
        {servers.length === 0 && <p className="px-2 py-1.5 text-sm text-muted-foreground">{t("accountsettings.agents.noManaged")}</p>}
        {servers.map((s) => {
          const here = inServer(s);
          return (
            <DropdownMenuItem key={s.id} disabled={here} onSelect={() => onAdd(s)}>
              <ServerIcon /> <span className="truncate">{s.name}</span>
              {here && <CheckIcon className="ml-auto" />}
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** A new token or deleting the agent, each asked once more. */
function DangerActions({
  confirm,
  busy,
  onConfirm,
  onReset,
  onRemove,
}: {
  confirm: null | "reset" | "delete";
  busy: null | "save" | "reset" | "add";
  onConfirm: (confirm: null | "reset" | "delete") => void;
  onReset: () => void;
  onRemove: () => void;
}) {
  const { t } = useI18n();
  return (
    <AnimatePresence mode="popLayout" initial={false}>
      {confirm ? (
        <motion.span
          key="confirm"
          initial={{ opacity: 0, x: 8 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: 8 }}
          transition={SPRING}
          className="flex items-center gap-1"
        >
          <span className="flex items-center gap-1 px-1 text-xs font-bold">
            <TriangleAlertIcon className="size-3.5 text-amber-500" />
            {confirm === "reset" ? t("accountsettings.agents.resetWarning") : t("accountsettings.agents.deleteWarning")}
          </span>
          <Button
            type="button"
            size="sm"
            variant="destructive"
            className="h-8 rounded-full px-3 text-xs font-bold"
            onClick={confirm === "reset" ? onReset : onRemove}
          >
            {confirm === "reset" ? t("accountsettings.agents.newToken") : t("accountsettings.agents.delete")}
          </Button>
          <Button
            type="button"
            size="icon"
            variant="ghost"
            aria-label={t("accountsettings.agents.neverMind")}
            className="size-8 rounded-full"
            onClick={() => onConfirm(null)}
          >
            <XIcon />
          </Button>
        </motion.span>
      ) : (
        <motion.span
          key="actions"
          initial={{ opacity: 0, x: -8 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: -8 }}
          transition={SPRING}
          className="flex gap-2"
        >
          <Button type="button" variant="ghost" size="sm" className="rounded-xl" disabled={!!busy} onClick={() => onConfirm("reset")}>
            <RefreshCwIcon className={cn(busy === "reset" && "animate-spin")} /> {t("accountsettings.agents.newToken")}
          </Button>
          <Button type="button" variant="ghost" size="sm" className="rounded-xl text-destructive hover:text-destructive" onClick={() => onConfirm("delete")}>
            <Trash2Icon /> {t("accountsettings.agents.delete")}
          </Button>
        </motion.span>
      )}
    </AnimatePresence>
  );
}
