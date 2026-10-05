import { BotIcon, ExternalLinkIcon, LoaderCircleIcon, SlashIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useContext, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { create } from "@bufbuild/protobuf";
import type { Command, ListCommandsResponse } from "@/gen/fuwa/v1/command_pb";
import { CommandOptionType } from "@/gen/fuwa/v1/command_pb";
import {
  ButtonStyle,
  ChannelType,
  CommandArgumentSchema,
  InteractionKind,
  type Button,
  type Channel,
  type Member,
  type Message,
  type User,
} from "@/gen/fuwa/v1/types_pb";
import { listCommands, pressButton, run, runCommand } from "@/fuwa/actions";
import { useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { PollPlace } from "@/components/chat/pollPlace";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { T, useI18n } from "@/i18n/react";
import { displayName, memberName } from "@/lib/format";
import { hidesPersonal } from "@/lib/streamer";
import { usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * Agents' slash commands and buttons (docs/commands.md). Typing "/" at the
 * start of the composer lists the commands of the agents in this server;
 * picking one swaps the box for its options, and sending runs it. The agent
 * answers with an ordinary message, headed "used /name". Buttons on agents'
 * messages tell the agent who pressed them; link buttons just open the link.
 */

const MAX = 8;
/** How long a server's list is reused before asking again. */
const FRESH_MS = 30_000;
const NO_MEMBERS: Member[] = [];
const NO_CHANNELS: Channel[] = [];

type Listed = { at: number; list: ListCommandsResponse };
const lists = new Map<string, Listed>();

export type CommandChoice = { key: string; agent: User | undefined; agentId: string; command: Command };

/** This server's commands, asked for the first time you type "/" and then every so often. */
function useServerCommands(instanceKey: string, serverId: string, wanted: boolean) {
  const cacheKey = `${instanceKey}\n${serverId}`;
  const [listed, setListed] = useState<Listed | undefined>(() => lists.get(cacheKey));
  const [loading, setLoading] = useState(false);
  useEffect(() => setListed(lists.get(cacheKey)), [cacheKey]);
  useEffect(() => {
    if (!wanted) return;
    const had = lists.get(cacheKey);
    if (had && Date.now() - had.at < FRESH_MS) return;
    let live = true;
    setLoading(true);
    run(listCommands(instanceKey, serverId))
      .then((list) => {
        const next = { at: Date.now(), list };
        lists.set(cacheKey, next);
        if (live) setListed(next);
      })
      .catch(() => {})
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
  }, [wanted, cacheKey, instanceKey, serverId]);
  const choices = useMemo<CommandChoice[]>(() => {
    const agents = new Map((listed?.list.agents ?? []).map((a) => [a.id, a]));
    return (listed?.list.commands ?? [])
      .filter((c) => c.command)
      .map((c) => ({ key: `${c.agentId}/${c.command!.name}`, agentId: c.agentId, agent: agents.get(c.agentId), command: c.command! }));
  }, [listed]);
  return { choices, loading: loading && !listed };
}

export type CommandPickerState = ReturnType<typeof useCommandPicker>;

/**
 * The "/" list and the command you've picked. `text` is the composer's box:
 * the list is open while it's "/" and a name with no space yet.
 */
export function useCommandPicker(instanceKey: string, serverId: string, channelId: string, text: string, setText: (text: string) => void) {
  const query = /^\/([a-z0-9_-]{0,32})$/i.exec(text)?.[1]?.toLowerCase();
  const typing = query !== undefined;
  const { choices, loading } = useServerCommands(instanceKey, serverId, typing);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const [chosen, setChosen] = useState<CommandChoice | null>(null);
  const [values, setValues] = useState<Record<string, string>>({});
  const [running, setRunning] = useState(false);

  useEffect(() => {
    if (!typing) setDismissed(false);
  }, [typing]);
  useEffect(() => setActive(0), [query]);
  // A different channel or server starts over.
  useEffect(() => {
    setChosen(null);
    setValues({});
  }, [channelId, serverId]);

  const options = useMemo(() => {
    if (query === undefined || dismissed) return [];
    const starts = choices.filter((c) => c.command.name.startsWith(query));
    const rest = choices.filter((c) => !c.command.name.startsWith(query) && c.command.name.includes(query));
    return [...starts, ...rest].slice(0, MAX);
  }, [choices, query, dismissed]);

  const open = typing && !dismissed && !chosen && (options.length > 0 || loading || choices.length === 0);

  const pick = useCallback(
    (choice: CommandChoice) => {
      setChosen(choice);
      setValues({});
      setText("");
    },
    [setText],
  );

  const cancel = useCallback(() => {
    setChosen(null);
    setValues({});
  }, []);

  const missing = chosen?.command.options.filter((o) => o.required && !values[o.name]?.trim()).map((o) => o.name) ?? [];

  /** Runs the picked command; false when something's missing or it's already on its way. */
  async function send(): Promise<boolean> {
    if (!chosen || running || missing.length) return false;
    const args = chosen.command.options
      .filter((o) => values[o.name]?.trim())
      .map((o) => create(CommandArgumentSchema, { name: o.name, value: values[o.name]!.trim() }));
    setRunning(true);
    try {
      await run(runCommand(instanceKey, serverId, channelId, chosen.agentId, chosen.command.name, args));
      cancel();
      return true;
    } catch (err) {
      toast((err as Error).message);
      return false;
    } finally {
      setRunning(false);
    }
  }

  return {
    open,
    loading,
    empty: !loading && choices.length === 0,
    options,
    active,
    setActive,
    pick,
    chosen,
    values,
    setValue: (name: string, value: string) => setValues((v) => ({ ...v, [name]: value })),
    missing,
    running,
    cancel,
    send,
    /** Handles the list's keys while it's open; true when it took the key. */
    onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
      if (!open) return false;
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(true);
        return true;
      }
      if (!options.length) return false;
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const by = e.key === "ArrowDown" ? 1 : -1;
        setActive((n) => (n + by + options.length) % options.length);
        return true;
      }
      if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") {
        e.preventDefault();
        pick(options[active] ?? options[0]!);
        return true;
      }
      return false;
    },
  };
}

/** The list over the composer while you type "/". */
export function CommandPicker({ picker }: { picker: CommandPickerState }) {
  const { t } = useI18n();
  return (
    <AnimatePresence>
      {picker.open && (
        <motion.div
          initial={{ opacity: 0, y: 8, scale: 0.97 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: 6, scale: 0.98 }}
          transition={SPRING}
          className="absolute right-0 bottom-full left-0 z-20 mb-2 origin-bottom overflow-hidden rounded-2xl border bg-popover p-1.5 shadow-xl"
        >
          <p className="flex items-center gap-1 px-2 pt-0.5 pb-1 text-[0.65rem] font-extrabold tracking-wide text-muted-foreground uppercase">
            <SlashIcon className="size-3" /> {t("chattools.commands.title")}
          </p>
          {picker.loading ? (
            <p className="flex items-center gap-2 px-2 py-2 text-sm text-muted-foreground">
              <LoaderCircleIcon className="size-4 animate-spin" /> {t("chattools.commands.looking")}
            </p>
          ) : picker.empty ? (
            <p className="px-2 py-2 text-sm text-muted-foreground">{t("chattools.commands.none")}</p>
          ) : (
            <ul role="listbox" aria-label={t("chattools.commands.title")}>
              {picker.options.map((choice, n) => {
                const on = n === picker.active;
                return (
                  <li key={choice.key} role="option" aria-selected={on}>
                    <button
                      type="button"
                      onMouseDown={(e) => e.preventDefault()}
                      onMouseEnter={() => picker.setActive(n)}
                      onClick={() => picker.pick(choice)}
                      className="relative flex w-full items-center gap-2 rounded-xl px-2 py-1.5 text-left text-sm"
                    >
                      {on && <motion.span layoutId="command-active" transition={SPRING} className="absolute inset-0 rounded-xl bg-primary/12" />}
                      <UserAvatar user={choice.agent} className="relative size-6" />
                      <span className="relative shrink-0 font-bold">/{choice.command.name}</span>
                      <span className="relative min-w-0 flex-1 truncate text-muted-foreground">{choice.command.description}</span>
                      <span className="relative hidden shrink-0 items-center gap-1 text-xs text-muted-foreground sm:flex">
                        <BotIcon className="size-3" /> {displayName(choice.agent)}
                      </span>
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </motion.div>
      )}
    </AnimatePresence>
  );
}

/**
 * In place of the box once you've picked a command: its options as fields.
 * Enter in a field runs it, Escape goes back to the box.
 */
export function CommandForm({
  instanceKey,
  serverId,
  picker,
  onDone,
}: {
  instanceKey: string;
  serverId: string;
  picker: CommandPickerState;
  onDone: () => void;
}) {
  const chosen = picker.chosen!;
  const first = useRef<HTMLInputElement & HTMLSelectElement>(null);
  const form = useRef<HTMLDivElement>(null);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? NO_MEMBERS);
  const channels = useFuwa((s) => s.instances[instanceKey]?.channels[serverId] ?? NO_CHANNELS);
  const roles = useRoles(instanceKey, serverId);
  const { t } = useI18n();

  useEffect(() => {
    // With nothing to fill in, the form itself takes Enter and Escape.
    requestAnimationFrame(() => (first.current ?? form.current)?.focus());
  }, [chosen.key]);

  function onKeyDown(e: KeyboardEvent<HTMLElement>) {
    if (e.nativeEvent.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      picker.cancel();
      onDone();
    } else if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      void picker.send().then((ran) => ran && onDone());
    }
  }

  const field = "h-8 min-w-0 rounded-lg border bg-background px-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring max-sm:w-full max-sm:max-w-none";

  return (
    <motion.div
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: 6 }}
      transition={SPRING}
      ref={form}
      tabIndex={-1}
      onKeyDown={onKeyDown}
      className="flex min-w-0 flex-1 flex-col gap-2 py-1 outline-none"
    >
      <div className="flex min-w-0 items-center gap-2">
        <span className="shrink-0 rounded-lg bg-primary/12 px-2 py-0.5 text-sm font-extrabold text-primary">/{chosen.command.name}</span>
        <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {chosen.command.description} · {displayName(chosen.agent)}
        </span>
        <button
          type="button"
          aria-label={t("chattools.commands.back")}
          title={t("chattools.commands.backHint")}
          onClick={() => {
            picker.cancel();
            onDone();
          }}
          className="grid size-7 shrink-0 place-items-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
        >
          <XIcon className="size-4" />
        </button>
      </div>
      {chosen.command.options.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {chosen.command.options.map((option, n) => {
            const value = picker.values[option.name] ?? "";
            const set = (v: string) => picker.setValue(option.name, v);
            const ref = n === 0 ? first : undefined;
            const label = option.required ? option.name : t("chattools.commands.optional", { name: option.name });
            let input;
            switch (option.type) {
              case CommandOptionType.INTEGER:
                input = <input ref={ref} type="number" step={1} inputMode="numeric" value={value} onChange={(e) => set(e.target.value)} className={cn(field, "w-28")} />;
                break;
              case CommandOptionType.BOOLEAN:
                input = (
                  <select ref={ref} value={value} onChange={(e) => set(e.target.value)} className={field}>
                    <option value="">{t("chattools.commands.pickOne")}</option>
                    <option value="true">{t("chattools.commands.yes")}</option>
                    <option value="false">{t("chattools.commands.no")}</option>
                  </select>
                );
                break;
              case CommandOptionType.USER:
                input = (
                  <select ref={ref} value={value} onChange={(e) => set(e.target.value)} className={cn(field, "max-w-48")}>
                    <option value="">{t("chattools.commands.pickSomeone")}</option>
                    {members
                      .filter((m) => m.user)
                      .map((m) => (
                        <option key={m.user!.id} value={m.user!.id}>
                          {memberName(m)}
                        </option>
                      ))}
                  </select>
                );
                break;
              case CommandOptionType.CHANNEL:
                input = (
                  <select ref={ref} value={value} onChange={(e) => set(e.target.value)} className={cn(field, "max-w-48")}>
                    <option value="">{t("chattools.commands.pickChannel")}</option>
                    {channels
                      .filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.VOICE)
                      .map((c) => (
                        <option key={c.id} value={c.id}>
                          #{c.name}
                        </option>
                      ))}
                  </select>
                );
                break;
              case CommandOptionType.ROLE:
                input = (
                  <select ref={ref} value={value} onChange={(e) => set(e.target.value)} className={cn(field, "max-w-48")}>
                    <option value="">{t("chattools.commands.pickRole")}</option>
                    {roles.map((r) => (
                      <option key={r.id} value={r.id}>
                        {r.id === serverId ? "@everyone" : r.name}
                      </option>
                    ))}
                  </select>
                );
                break;
              default:
                input = option.choices.length ? (
                  <select ref={ref} value={value} onChange={(e) => set(e.target.value)} className={cn(field, "max-w-48")}>
                    <option value="">{t("chattools.commands.pickOne")}</option>
                    {option.choices.map((c) => (
                      <option key={c} value={c}>
                        {c}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input ref={ref} type="text" maxLength={1000} value={value} onChange={(e) => set(e.target.value)} className={cn(field, "w-48 max-sm:w-full")} />
                );
            }
            return (
              <label key={option.name} title={option.description} className="flex min-w-0 flex-col gap-0.5 max-sm:w-full">
                <span className={cn("text-[0.7rem] font-bold text-muted-foreground", picker.missing.includes(option.name) && "text-foreground")}>{label}</span>
                {input}
              </label>
            );
          })}
        </div>
      )}
    </motion.div>
  );
}

/** Over an agent's answer: who used which command or button. */
export function UsedCommand({ message, instanceKey, serverId }: { message: Message; instanceKey: string; serverId: string }) {
  const used = message.interaction;
  const member = useFuwa((s) => (used ? s.instances[instanceKey]?.members[serverId]?.find((m) => m.user?.id === used.userId) : undefined));
  const { t } = useI18n();
  if (!used) return null;
  const name = <b className="text-foreground/80">{member ? memberName(member) : t("common.someone")}</b>;
  return (
    <p className="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground">
      <UserAvatar user={member?.user} className="size-4" />
      <span className="truncate">
        {used.kind === InteractionKind.BUTTON ? (
          <T k="chattools.commands.pressed" values={{ name }} />
        ) : (
          <T k="chattools.commands.used" values={{ name, command: <b className="text-primary">/{used.command}</b> }} />
        )}
      </span>
    </p>
  );
}

const STYLES: Record<ButtonStyle, string> = {
  [ButtonStyle.UNSPECIFIED]: "bg-muted text-foreground hover:bg-muted/70",
  [ButtonStyle.SECONDARY]: "bg-muted text-foreground hover:bg-muted/70",
  [ButtonStyle.PRIMARY]: "bg-primary text-primary-foreground hover:bg-primary/90",
  [ButtonStyle.SUCCESS]: "bg-emerald-600 text-white hover:bg-emerald-600/90",
  [ButtonStyle.DANGER]: "bg-destructive text-white hover:bg-destructive/90",
  [ButtonStyle.LINK]: "bg-muted text-foreground hover:bg-muted/70",
};

/** An agent's buttons under its message. */
export function MessageButtons({ message }: { message: Message }) {
  const place = useContext(PollPlace);
  if (!message.components.length) return null;
  return (
    <div className="mt-1.5 flex flex-col gap-1.5">
      {message.components.map((row, r) => (
        <div key={r} className="flex flex-wrap gap-1.5">
          {row.buttons.map((button, b) => (
            <MessageButton key={button.customId || `${r}-${b}`} button={button} message={message} canPress={!!place?.canVote} place={place} />
          ))}
        </div>
      ))}
    </div>
  );
}

function MessageButton({
  button,
  message,
  canPress,
  place,
}: {
  button: Button;
  message: Message;
  canPress: boolean;
  place: { instanceKey: string; serverId: string } | null;
}) {
  const [pressing, setPressing] = useState(false);
  const [pressed, setPressed] = useState(false);
  const streamer = usePrefs((p) => hidesPersonal(p));
  const look = cn(
    "relative inline-flex h-8 max-w-full items-center gap-1.5 rounded-lg px-3 text-sm font-bold transition-colors disabled:cursor-not-allowed disabled:opacity-50",
    STYLES[button.style] ?? STYLES[ButtonStyle.SECONDARY],
  );
  if (button.style === ButtonStyle.LINK)
    return (
      <motion.a
        href={button.url}
        target="_blank"
        rel="noopener noreferrer"
        title={streamer ? undefined : button.url}
        whileTap={{ scale: 0.95 }}
        className={look}
      >
        <span className="truncate">{button.label}</span>
        <ExternalLinkIcon className="size-3.5 shrink-0" />
      </motion.a>
    );
  return (
    <motion.button
      type="button"
      disabled={button.disabled || !canPress || pressing || !place}
      whileTap={{ scale: 0.95 }}
      animate={pressed ? { scale: [1, 1.06, 1] } : undefined}
      transition={SPRING}
      onClick={() => {
        if (!place) return;
        setPressing(true);
        run(pressButton(place.instanceKey, place.serverId, message.id, button.customId))
          .then(() => {
            setPressed(true);
            setTimeout(() => setPressed(false), 600);
          })
          .catch((err: Error) => toast(err.message))
          .finally(() => setPressing(false));
      }}
      className={look}
    >
      <span className="truncate">{button.label}</span>
      {pressing && <LoaderCircleIcon className="size-3.5 shrink-0 animate-spin" />}
    </motion.button>
  );
}
