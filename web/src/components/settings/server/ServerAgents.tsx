import { AtSignIcon, BotIcon, CheckIcon, LoaderCircleIcon, PlugZapIcon, PlusIcon, UserMinusIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import { McpAccessMode, type Agent, type McpAccess } from "@/gen/fuwa/v1/agent_pb";
import { addAgent, getMcpAccess, kickMember, listAgents, run, setMcpAccess } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { AppBadge } from "@/components/AppBadge";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { ago, displayName, isAgent, toDate } from "@/lib/format";
import { type Key, T, useI18n } from "@/i18n/react";
import { openSettings, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * The agents in a server, for people who manage it: who's here, adding one
 * by username (or one of your own with a tap), and taking one out.
 */
export function ServerAgents({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const lang = useI18n();
  const { t } = lang;
  const inst = useInstance(instanceKey);
  const here = useMemo(() => (inst?.members[serverId] ?? []).filter((m) => isAgent(m.user)), [inst?.members, serverId]);
  const [mine, setMine] = useState<Agent[]>([]);
  const [username, setUsername] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [justAdded, setJustAdded] = useState<string | null>(null);
  const shake = useAnimationControls();
  const mcpOn = useFuwa((s) => !!s.instances[instanceKey]?.node?.mcp);
  const [mcp, setMcp] = useState<McpAccess | null>(null);

  useEffect(() => {
    run(listAgents(instanceKey)).then(setMine, () => setMine([]));
  }, [instanceKey]);

  useEffect(() => {
    if (!mcpOn) return;
    run(getMcpAccess(instanceKey, serverId)).then(setMcp, () => setMcp(null));
  }, [instanceKey, serverId, mcpOn]);

  async function saveMcp(mode: McpAccessMode, agentIds: string[]) {
    const before = mcp;
    // Shown at once; put back if the server says no.
    setMcp((m) => (m ? { ...m, mode, agentIds } : m));
    try {
      setMcp(await run(setMcpAccess(instanceKey, serverId, mode, agentIds)));
    } catch (err) {
      setMcp(before);
      toast((err as FuwaError).message);
    }
  }

  const chosen = mcp?.mode === McpAccessMode.CHOSEN;
  const mcpIds = new Set(mcp?.agentIds ?? []);

  const hereIds = new Set(here.map((m) => m.user?.id));
  const suggestions = mine.filter((a) => !hereIds.has(a.user?.id));

  async function add(name: string) {
    const clean = name.trim().replace(/^@/, "").toLowerCase();
    if (!clean) {
      void shake.start({ x: [0, -8, 6, -4, 0], transition: { duration: 0.35 } });
      return;
    }
    setBusy(`add:${clean}`);
    try {
      const member = await run(addAgent(instanceKey, serverId, clean));
      setUsername("");
      setJustAdded(member.user?.id ?? null);
      setTimeout(() => setJustAdded(null), 1800);
    } catch (err) {
      toast((err as FuwaError).message);
      void shake.start({ x: [0, -8, 6, -4, 0], transition: { duration: 0.35 } });
    }
    setBusy(null);
  }

  async function remove(userId: string) {
    setConfirm(null);
    setBusy(`remove:${userId}`);
    try {
      await run(kickMember(instanceKey, serverId, userId, "Removed from Integrations"));
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setBusy(null);
  }

  return (
    <section data-setting="agents" className="flex flex-col gap-3">
      <div className="flex items-center gap-2">
        <motion.span
          animate={{ rotate: [0, -10, 10, 0] }}
          transition={{ duration: 1.6, repeat: Infinity, repeatDelay: 3 }}
          className="grid size-8 place-items-center rounded-xl bg-violet-500/15 text-violet-500"
        >
          <BotIcon className="size-4" />
        </motion.span>
        <div className="min-w-0 flex-1">
          <p className="font-extrabold">{t("settings.nav.agents")}</p>
          <p className="text-sm text-muted-foreground">{t("serversettings.agents.intro")}</p>
        </div>
      </div>

      <form
        onSubmit={(e: FormEvent) => {
          e.preventDefault();
          void add(username);
        }}
      >
        <motion.div animate={shake} className="flex items-center gap-2">
          <span className="relative min-w-0 flex-1">
            <AtSignIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={username}
              onChange={(e) => setUsername(e.target.value)}
              placeholder={t("serversettings.agents.username")}
              spellCheck={false}
              maxLength={33}
              className="h-10 rounded-xl pl-9"
            />
          </span>
          <Button type="submit" className="btn h-10 rounded-xl font-bold" disabled={!!busy}>
            {busy?.startsWith("add:") ? <LoaderCircleIcon className="animate-spin" /> : <PlusIcon />} {t("serversettings.channelPermissions.add")}
          </Button>
        </motion.div>
      </form>

      <AnimatePresence initial={false}>
        {suggestions.length > 0 && (
          <motion.div
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={SPRING}
            className="flex flex-wrap items-center gap-1.5 overflow-hidden"
          >
            <span className="text-xs font-bold text-muted-foreground">{t("serversettings.agents.yours")}</span>
            {suggestions.map((a, n) => (
              <motion.button
                key={a.user?.id}
                type="button"
                layout
                initial={{ opacity: 0, scale: 0.8 }}
                animate={{ opacity: 1, scale: 1, transition: { ...SPRING, delay: n * 0.04 } }}
                exit={{ opacity: 0, scale: 0.6 }}
                whileHover={{ y: -2 }}
                whileTap={{ scale: 0.94 }}
                disabled={!!busy}
                onClick={() => void add(a.user?.username ?? "")}
                className="flex items-center gap-1.5 rounded-full border bg-background/60 py-1 pr-2.5 pl-1 text-xs font-bold transition-colors hover:border-primary/40 hover:bg-primary/5"
              >
                <UserAvatar user={a.user} className="size-5" />
                {displayName(a.user)}
                <PlusIcon className="size-3 text-primary" />
              </motion.button>
            ))}
          </motion.div>
        )}
      </AnimatePresence>

      {mcpOn && mcp && <McpChoice mode={mcp.mode} onChange={(mode) => void saveMcp(mode, mode === McpAccessMode.CHOSEN ? [...mcpIds] : [])} />}

      {here.length === 0 ? (
        <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} className="flex items-center gap-3 rounded-2xl border border-dashed p-4 text-sm text-muted-foreground">
          <motion.span animate={{ y: [0, -4, 0] }} transition={{ duration: 2, repeat: Infinity, repeatDelay: 1 }} className="text-2xl">
            🤖
          </motion.span>
          <span>
            <T
              k="serversettings.agents.none"
              values={{
                settings: (
                  <button type="button" className="font-bold text-primary hover:underline" onClick={() => openSettings("agents")}>
                    {t("serversettings.agents.settingsAgents")}
                  </button>
                ),
              }}
            />
          </span>
        </motion.div>
      ) : (
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false}>
            {here.map((m) => {
              const id = m.user?.id ?? "";
              const fresh = justAdded === id;
              return (
                <motion.li
                  key={id}
                  layout
                  initial={{ opacity: 0, y: -8, scale: 0.96 }}
                  animate={{ opacity: 1, y: 0, scale: 1 }}
                  exit={{ opacity: 0, x: -24, filter: "blur(4px)" }}
                  transition={SPRING}
                  className={cn(
                    "group flex items-center gap-3 rounded-2xl border bg-background/50 p-2.5 transition-colors hover:border-primary/30",
                    fresh && "border-emerald-500/50 bg-emerald-500/5",
                  )}
                >
                  <motion.span animate={fresh ? { rotate: [0, -12, 12, 0], scale: [1, 1.15, 1] } : {}} transition={{ duration: 0.6 }}>
                    <UserAvatar user={m.user} className="size-9" />
                  </motion.span>
                  <span className="min-w-0 flex-1">
                    <span className="flex items-center gap-1.5">
                      <span className="truncate font-bold">{m.nickname || displayName(m.user)}</span>
                      <AppBadge agent />
                      <AnimatePresence>
                        {fresh && (
                          <motion.span initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} className="text-emerald-500">
                            <CheckIcon className="size-3.5" strokeWidth={3} />
                          </motion.span>
                        )}
                      </AnimatePresence>
                    </span>
                    <span className="block truncate text-xs text-muted-foreground">
                      {t("serversettings.agents.line", { username: m.user?.username ?? "", when: ago(lang, toDate(m.joinedAt)) })}
                    </span>
                  </span>
                  <AnimatePresence initial={false}>
                    {chosen && (
                      <motion.label
                        key="mcp"
                        initial={{ opacity: 0, scale: 0.9 }}
                        animate={{ opacity: 1, scale: 1 }}
                        exit={{ opacity: 0, scale: 0.9 }}
                        transition={SPRING}
                        className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground"
                      >
                        MCP
                        <Switch
                          checked={mcpIds.has(id)}
                          aria-label={t("serversettings.agents.mcpFor", { name: displayName(m.user) })}
                          onCheckedChange={(on) => {
                            const ids = new Set(mcpIds);
                            if (on) ids.add(id);
                            else ids.delete(id);
                            void saveMcp(McpAccessMode.CHOSEN, [...ids]);
                          }}
                        />
                      </motion.label>
                    )}
                  </AnimatePresence>
                  <AnimatePresence mode="popLayout" initial={false}>
                    {confirm === id ? (
                      <motion.span key="confirm" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={SPRING} className="flex items-center gap-1">
                        <Button type="button" size="sm" variant="destructive" className="h-8 rounded-full px-3 text-xs font-bold" onClick={() => void remove(id)}>
                          {t("serversettings.channelPermissions.remove")}
                        </Button>
                        <Button type="button" size="icon" variant="ghost" aria-label={t("serversettings.shared.neverMind")} className="size-8 rounded-full" onClick={() => setConfirm(null)}>
                          <XIcon />
                        </Button>
                      </motion.span>
                    ) : (
                      <motion.span key="remove" initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING}>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="rounded-xl text-muted-foreground hover:text-destructive"
                          disabled={busy === `remove:${id}`}
                          onClick={() => setConfirm(id)}
                        >
                          {busy === `remove:${id}` ? <LoaderCircleIcon className="animate-spin" /> : <UserMinusIcon />}
                          <span className="hidden sm:inline">{t("serversettings.channelPermissions.remove")}</span>
                        </Button>
                      </motion.span>
                    )}
                  </AnimatePresence>
                </motion.li>
              );
            })}
          </AnimatePresence>
        </ul>
      )}
    </section>
  );
}

/** Who may use the server through MCP; labels are catalog keys. */
const MCP_CHOICES: readonly { mode: McpAccessMode; label: Key }[] = [
  { mode: McpAccessMode.ALL, label: "serversettings.agents.mcpAll" },
  { mode: McpAccessMode.CHOSEN, label: "serversettings.agents.mcpChosen" },
  { mode: McpAccessMode.OFF, label: "serversettings.agents.mcpNone" },
];

/**
 * Which agents may use this server through the instance's MCP endpoint: AI
 * apps (Claude and others) reach it there with an agent's token.
 */
function McpChoice({ mode, onChange }: { mode: McpAccessMode; onChange: (mode: McpAccessMode) => void }) {
  const { t } = useI18n();
  const current = mode === McpAccessMode.UNSPECIFIED ? McpAccessMode.ALL : mode;
  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      data-setting="mcp-access"
      className="flex flex-col gap-2 rounded-2xl border bg-background/40 p-3"
    >
      <div className="flex items-center gap-2">
        <PlugZapIcon className="size-4 text-violet-500" />
        <p className="flex-1 text-sm font-bold">{t("serversettings.agents.mcp")}</p>
      </div>
      <p className="text-xs text-muted-foreground">{t("serversettings.agents.mcpHint")}</p>
      <div role="radiogroup" aria-label={t("serversettings.agents.mcpWho")} className="grid grid-cols-3 gap-1 rounded-xl bg-muted/60 p-1">
        {MCP_CHOICES.map((choice) => {
          const on = current === choice.mode;
          return (
            <button
              key={choice.mode}
              type="button"
              role="radio"
              aria-checked={on}
              onClick={() => !on && onChange(choice.mode)}
              className={cn("relative rounded-lg px-2 py-1.5 text-xs font-bold transition-colors", on ? "text-foreground" : "text-muted-foreground hover:text-foreground")}
            >
              {on && <motion.span layoutId="mcp-choice" transition={SPRING} className="absolute inset-0 rounded-lg bg-background shadow-sm" />}
              <span className="relative">{t(choice.label)}</span>
            </button>
          );
        })}
      </div>
    </motion.div>
  );
}
