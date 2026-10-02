import { motion } from "motion/react";
import { createContext, use, useMemo, type ReactNode } from "react";
import type { Emoji, Member, Role, User } from "@/gen/fuwa/v1/types_pb";
import { ProfilePopover } from "@/components/ProfilePopover";
import { memberName } from "@/lib/format";
import { colorOf, cssColor } from "@/lib/permissions";
import { usePrefs } from "@/lib/prefs";
import { cn } from "@/lib/utils";

/**
 * Mentions in messages: `@username`, `@everyone`, `@here` and roles written
 * `<@&role id>` (the composer writes those for you). Pinging is decided by
 * the server; this only draws them as chips, colored like the role and
 * brighter when they're about you. A server's own emoji (`<:name:id>`) go
 * through here too, drawn as their pictures.
 */

/** What a server's messages need to draw names and mentions. */
export type ServerLook = {
  instanceKey?: string;
  ownerId: string;
  /** Highest first. */
  roles: Role[];
  members: Member[];
  /** The server's own emoji. */
  emojis?: Emoji[];
  /** Your id and roles, to light up mentions of you. */
  me?: { id: string; username: string; roleIds: string[] };
};

const EMPTY_LOOK: ServerLook = { ownerId: "", roles: [], members: [] };
const LookContext = createContext<ServerLook>(EMPTY_LOOK);

export function ServerLookProvider({ value, children }: { value: ServerLook; children: ReactNode }) {
  return <LookContext value={value}>{children}</LookContext>;
}

export const useServerLook = () => use(LookContext);

/** A member's name color, from their highest colored role (when there's a server around). */
export function useRoleColor(member: Member | undefined): number | undefined {
  const { roles } = useServerLook();
  return useMemo(() => colorOf(roles, member), [roles, member]);
}

// ───────────────────────── Finding them in the text ─────────────────────────

const ROLE = String.raw`<@&([0-9A-Za-z]{26})>`;
const EVERYONE = String.raw`(?<![\w@<])@(everyone|here)\b`;
const USER = String.raw`(?<![\w@<.])@([a-z0-9][a-z0-9_.]{0,30}[a-z0-9_]|[a-z0-9])(?![\w])`;
const EMOJI = String.raw`<(a?):([A-Za-z0-9_]{2,32}):([0-9A-Za-z]{10,32})>`;
const PATTERN = new RegExp(`${ROLE}|${EVERYONE}|${USER}|${EMOJI}`, "gi");

type MdNode = { type: string; value?: string; children?: MdNode[]; data?: Record<string, unknown> };

const mention = (kind: string, target: string, text: string): MdNode => ({
  type: "mention",
  data: { hName: "fuwa-mention", hProperties: { dataKind: kind, dataTarget: target } },
  children: [{ type: "text", value: text }],
});

function split(value: string): MdNode[] | null {
  const out: MdNode[] = [];
  let last = 0;
  for (const m of value.matchAll(PATTERN)) {
    const at = m.index ?? 0;
    if (at > last) out.push({ type: "text", value: value.slice(last, at) });
    if (m[6]) out.push(mention("emoji", m[6].toUpperCase(), `:${m[5]}:`));
    else if (m[1]) out.push(mention("role", m[1].toUpperCase(), m[0]));
    else if (m[2]) out.push(mention("everyone", m[2].toLowerCase(), m[0]));
    else out.push(mention("user", m[3]!.toLowerCase(), m[0]));
    last = at + m[0].length;
  }
  if (!out.length) return null;
  if (last < value.length) out.push({ type: "text", value: value.slice(last) });
  return out;
}

/** Leaves code and links alone. */
const SKIP = new Set(["code", "inlineCode", "link", "linkReference", "html"]);

function walk(node: MdNode) {
  if (!node.children) return;
  const next: MdNode[] = [];
  for (const child of node.children) {
    if (child.type === "text" && child.value) {
      const parts = split(child.value);
      if (parts) {
        next.push(...parts);
        continue;
      }
    } else if (!SKIP.has(child.type)) walk(child);
    next.push(child);
  }
  node.children = next;
}

/** The remark plugin that turns mentions into `fuwa-mention` elements. */
export function remarkMentions() {
  return (tree: MdNode) => walk(tree);
}

// ───────────────────────── Drawing them ─────────────────────────

type MentionProps = { children?: ReactNode; "data-kind"?: string; "data-target"?: string };

const chip =
  "mention inline-flex items-baseline rounded-md px-1 font-bold transition-[background-color,box-shadow] duration-200";

/** One mention, as a chip. Unknown people stay plain text; deleted roles say so. */
export function Mention(props: MentionProps) {
  const kind = props["data-kind"];
  const target = props["data-target"] ?? "";
  const look = useServerLook();
  const mode = usePrefs((p) => p.roleColors);
  if (kind === "emoji") {
    const emoji = look.emojis?.find((e) => e.id === target);
    if (!emoji) return <span className="text-muted-foreground">{props.children}</span>;
    return (
      <motion.img
        src={emoji.url}
        alt={`:${emoji.name}:`}
        title={`:${emoji.name}:`}
        draggable={false}
        whileHover={{ scale: 1.35, rotate: -6 }}
        transition={{ type: "spring", stiffness: 600, damping: 12 }}
        className="emoji inline-block object-contain"
      />
    );
  }
  if (kind === "role") {
    const role = look.roles.find((r) => r.id === target);
    if (!role) return <span className={cn(chip, "bg-muted text-muted-foreground")}>@deleted-role</span>;
    const mine = !!look.me?.roleIds.includes(role.id);
    const color = role.color !== undefined && mode !== "off" ? cssColor(role.color) : null;
    return (
      <motion.span
        whileHover={{ y: -1 }}
        className={cn(chip, !color && "bg-primary/15 text-primary", mine && "ring-1 ring-current/40")}
        style={color ? { color, backgroundColor: `color-mix(in srgb, ${color} ${mine ? 24 : 15}%, transparent)` } : undefined}
        title={`Role: ${role.name}`}
      >
        @{role.name}
      </motion.span>
    );
  }
  if (kind === "everyone")
    return <span className={cn(chip, "bg-primary/15 text-primary")}>@{target}</span>;
  const member = look.members.find((m) => m.user?.username === target);
  if (!member?.user) return <>{props.children}</>;
  return <UserMention member={member} user={member.user} me={look.me?.id === member.user.id} instanceKey={look.instanceKey} />;
}

function UserMention({ member, user, me, instanceKey }: { member: Member; user: User; me: boolean; instanceKey?: string }) {
  const label = (
    <span className={cn(chip, me ? "bg-primary/25 text-primary ring-1 ring-primary/40" : "bg-primary/12 text-primary hover:bg-primary/20")}>
      @{memberName(member)}
    </span>
  );
  if (!instanceKey) return label;
  return (
    <ProfilePopover instanceKey={instanceKey} user={user} member={member}>
      <button type="button" className="inline align-baseline">
        {label}
      </button>
    </ProfilePopover>
  );
}

/** A role, as the mention picker and settings show it. */
export function RoleDot({ role, className }: { role: Pick<Role, "color">; className?: string }) {
  return (
    <span
      aria-hidden
      className={cn("inline-block size-3 shrink-0 rounded-full", role.color === undefined && "bg-muted-foreground/50", className)}
      style={role.color !== undefined ? { background: cssColor(role.color) } : undefined}
    />
  );
}
