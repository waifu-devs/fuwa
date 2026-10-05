import { m as motion } from "motion/react";
import { createContext, use, useMemo, type ReactNode } from "react";
import type { Emoji, Member, Role, User } from "@/gen/fuwa/v1/types_pb";
import { EmojiImage } from "@/components/EmojiImage";
import { ProfilePopover } from "@/components/ProfilePopover";
import { useI18n } from "@/i18n/react";
import { memberName } from "@/lib/format";
import { colorOf, cssColor } from "@/lib/permissions";
import { usePrefs } from "@/lib/prefs";
import { cn } from "@/lib/utils";

/**
 * Mentions in messages: `@username`, `@everyone`, `@here` and roles written
 * `<@&role id>` (the composer writes those for you). Pinging is decided by
 * the server; this only draws them as chips, colored like the role and
 * brighter when they're about you. Server emoji (`<:name:id>`) go through
 * here too, drawn as their pictures: the server's own, ones from other
 * servers the message brought along (`Message.emojis`), or ones from your
 * other servers while it's still on its way.
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
  /** Your other servers' emoji, by id. */
  otherEmojis?: Map<string, Emoji>;
  /** Your id and roles, to light up mentions of you. */
  me?: { id: string; username: string; roleIds: string[] };
};

const EMPTY_LOOK: ServerLook = { ownerId: "", roles: [], members: [] };
const LookContext = createContext<ServerLook>(EMPTY_LOOK);

export function ServerLookProvider({ value, children }: { value: ServerLook; children: ReactNode }) {
  return <LookContext value={value}>{children}</LookContext>;
}

export const useServerLook = () => use(LookContext);

const NO_EMOJI: Emoji[] = [];
const MessageEmojiContext = createContext<Emoji[]>(NO_EMOJI);

/** The emoji from other servers one message brought along. */
export function MessageEmojis({ value, children }: { value: Emoji[] | undefined; children: ReactNode }) {
  if (!value?.length) return children;
  return <MessageEmojiContext value={value}>{children}</MessageEmojiContext>;
}

/** A member's name color, from their highest colored role (when there's a server around). */
export function useRoleColor(member: Member | undefined): number | undefined {
  const { roles } = useServerLook();
  return useMemo(() => colorOf(roles, member), [roles, member]);
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
  if (kind === "emoji") return <EmojiMention target={target}>{props.children}</EmojiMention>;
  if (kind === "role") return <RoleMention target={target} />;
  if (kind === "everyone")
    return <span className={cn(chip, "bg-primary/15 text-primary")}>@{target}</span>;
  const member = look.members.find((m) => m.user?.username === target);
  if (!member?.user) return <>{props.children}</>;
  return <UserMention member={member} user={member.user} me={look.me?.id === member.user.id} instanceKey={look.instanceKey} />;
}

/** A server emoji, as its picture; one nobody here has stays as its text. */
function EmojiMention({ target, children }: { target: string; children?: ReactNode }) {
  const look = useServerLook();
  const carried = use(MessageEmojiContext);
  const emoji =
    look.emojis?.find((e) => e.id === target) ?? carried.find((e) => e.id === target) ?? look.otherEmojis?.get(target);
  if (!emoji) return <span className="text-muted-foreground">{children}</span>;
  return <EmojiImage emoji={emoji} className="emoji" />;
}

function RoleMention({ target }: { target: string }) {
  const look = useServerLook();
  const mode = usePrefs((p) => p.roleColors);
  const { t } = useI18n();
  const role = look.roles.find((r) => r.id === target);
  if (!role) return <span className={cn(chip, "bg-muted text-muted-foreground")}>{t("chat.mentions.deletedRole")}</span>;
  const mine = !!look.me?.roleIds.includes(role.id);
  const color = role.color !== undefined && mode !== "off" ? cssColor(role.color) : null;
  return (
    <motion.span
      whileHover={{ y: -1 }}
      className={cn(chip, !color && "bg-primary/15 text-primary", mine && "ring-1 ring-current/40")}
      style={color ? { color, backgroundColor: `color-mix(in srgb, ${color} ${mine ? 24 : 15}%, transparent)` } : undefined}
      title={t("chat.mentions.role", { name: role.name })}
    >
      @{role.name}
    </motion.span>
  );
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
