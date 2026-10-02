import { AtSignIcon, ShieldIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { Permission, type Channel, type Member, type Role } from "@/gen/fuwa/v1/types_pb";
import { useAccess, useRoles } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { RoleDot } from "@/components/chat/mentions";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { memberName } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { cn } from "@/lib/utils";

/**
 * The list that opens when you type @ in the composer: people, roles you can
 * ping, and @everyone and @here when you may. Arrows move, Enter or Tab
 * picks, Escape closes. A role goes in as @Name and is sent as `<@&id>`.
 */

type Option =
  | { kind: "member"; key: string; member: Member; insert: string }
  | { kind: "role"; key: string; role: Role; insert: string }
  | { kind: "everyone"; key: string; name: "everyone" | "here"; insert: string };

const MAX = 8;
const NO_MEMBERS: Member[] = [];

export type MentionPickerState = ReturnType<typeof useMentionPicker>;

export function useMentionPicker(
  instanceKey: string,
  serverId: string,
  channel: Channel,
  box: RefObject<HTMLTextAreaElement | null>,
  text: string,
  setText: (text: string) => void,
) {
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? NO_MEMBERS);
  const roles = useRoles(instanceKey, serverId);
  const access = useAccess(instanceKey, serverId);
  const everyone = hasIn(access, channel.id, Permission.MENTION_EVERYONE);
  const [caret, setCaret] = useState(0);
  const [active, setActive] = useState(0);
  const [dismissed, setDismissed] = useState<number | null>(null);
  /** Roles picked by name, sent as their tokens. */
  const picked = useRef(new Map<string, string>());
  /** Where the caret goes once a pick is in the box. */
  const landing = useRef<number | null>(null);

  // Before the next key press, so fast typing after a pick goes after it.
  useLayoutEffect(() => {
    const at = landing.current;
    const el = box.current;
    if (at === null || !el) return;
    landing.current = null;
    el.focus();
    el.setSelectionRange(at, at);
  }, [text, box]);

  useEffect(() => {
    setCaret(box.current?.selectionStart ?? text.length);
  }, [text, box]);

  const token = useMemo(() => {
    const before = text.slice(0, caret);
    const m = /(?:^|\s)@([^\s@]{0,32})$/.exec(before);
    if (!m) return null;
    return { start: caret - m[1]!.length - 1, query: m[1]!.toLowerCase() };
  }, [text, caret]);

  const options = useMemo<Option[]>(() => {
    if (!token || dismissed === token.start) return [];
    const q = token.query;
    const hit = (...names: (string | undefined)[]) => names.some((n) => n?.toLowerCase().includes(q));
    const people: Option[] = members
      .filter((m) => m.user && hit(m.user.username, m.user.displayName, m.nickname))
      .sort((a, b) => Number(!a.user!.username.startsWith(q)) - Number(!b.user!.username.startsWith(q)))
      .slice(0, MAX)
      .map((m) => ({ kind: "member", key: `u-${m.user!.id}`, member: m, insert: `@${m.user!.username}` }));
    const pingable: Option[] = roles
      .filter((r) => r.id !== serverId && (r.mentionable || everyone) && hit(r.name))
      .map((r) => ({ kind: "role", key: `r-${r.id}`, role: r, insert: `@${r.name}` }));
    const loud: Option[] = everyone
      ? (["everyone", "here"] as const).filter((n) => n.startsWith(q)).map((n) => ({ kind: "everyone", key: n, name: n, insert: `@${n}` }))
      : [];
    return [...people.slice(0, MAX - Math.min(pingable.length + loud.length, 4)), ...pingable, ...loud].slice(0, MAX);
  }, [token, dismissed, members, roles, serverId, everyone]);

  useEffect(() => setActive(0), [token?.start, token?.query]);

  const pick = useCallback(
    (option: Option) => {
      if (!token) return;
      if (option.kind === "role") picked.current.set(option.role.name, option.role.id);
      const insert = `${option.insert} `;
      landing.current = token.start + insert.length;
      setText(text.slice(0, token.start) + insert + text.slice(caret));
    },
    [token, text, caret, setText],
  );

  const open = options.length > 0;

  return {
    open,
    options,
    active,
    pick,
    setActive,
    onSelect: () => setCaret(box.current?.selectionStart ?? 0),
    /** Handles the keys the list uses while it's open; true when it took the key. */
    onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
      if (!open) return false;
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
      if (e.key === "Escape") {
        e.preventDefault();
        setDismissed(token?.start ?? null);
        return true;
      }
      return false;
    },
    /** The text to send: roles picked by name become their tokens. */
    encode(content: string) {
      let out = content;
      const names = [...picked.current.keys()].sort((a, b) => b.length - a.length);
      for (const name of names) {
        const escaped = name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
        out = out.replace(new RegExp(`(^|\\s)@${escaped}(?=$|[^\\p{L}\\p{N}_])`, "gu"), `$1<@&${picked.current.get(name)}>`);
      }
      picked.current.clear();
      return out;
    },
  };
}

export function MentionPicker({ picker }: { picker: MentionPickerState }) {
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
            <AtSignIcon className="size-3" /> Mention
          </p>
          <ul role="listbox" aria-label="Mentions">
            {picker.options.map((option, n) => {
              const on = n === picker.active;
              return (
                <li key={option.key} role="option" aria-selected={on}>
                  <button
                    type="button"
                    // Keep the caret in the box.
                    onMouseDown={(e) => e.preventDefault()}
                    onMouseEnter={() => picker.setActive(n)}
                    onClick={() => picker.pick(option)}
                    className="relative flex w-full items-center gap-2 rounded-xl px-2 py-1.5 text-left text-sm"
                  >
                    {on && <motion.span layoutId="mention-active" transition={SPRING} className="absolute inset-0 rounded-xl bg-primary/12" />}
                    <OptionBody option={option} />
                  </button>
                </li>
              );
            })}
          </ul>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function OptionBody({ option }: { option: Option }) {
  if (option.kind === "member")
    return (
      <>
        <UserAvatar user={option.member.user} className="relative size-6" />
        <span className="relative truncate font-bold">{memberName(option.member)}</span>
        <span className="relative ml-auto truncate text-xs text-muted-foreground">@{option.member.user?.username}</span>
      </>
    );
  if (option.kind === "role")
    return (
      <>
        <span className="relative grid size-6 place-items-center">
          <RoleDot role={option.role} />
        </span>
        <span className="relative truncate font-bold">@{option.role.name}</span>
        <span className={cn("relative ml-auto flex items-center gap-1 text-xs text-muted-foreground")}>
          <ShieldIcon className="size-3" /> Role
        </span>
      </>
    );
  return (
    <>
      <span className="relative grid size-6 place-items-center rounded-full bg-primary/15 text-primary">
        <AtSignIcon className="size-3.5" />
      </span>
      <span className="relative font-bold">@{option.name}</span>
      <span className="relative ml-auto text-xs text-muted-foreground">
Everyone who can see this channel
      </span>
    </>
  );
}
