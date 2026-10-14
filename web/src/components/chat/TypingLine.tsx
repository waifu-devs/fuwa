import { AnimatePresence, m as motion, useReducedMotion } from "motion/react";
import { useCallback, useMemo } from "react";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { useFuwa } from "@/fuwa/store";
import { useTypingIn, type TypingTarget } from "@/fuwa/typing";
import { displayName, memberName } from "@/lib/format";
import { blockedIds } from "@/lib/friends";
import { SPRING } from "@/lib/motion";
import { usePrefs } from "@/lib/prefs";
import { typingLabel } from "@/lib/typing";
import { type Key, T } from "@/i18n/react";

const NO_MEMBERS: Member[] = [];
const NO_USERS: User[] = [];
/** The placeholders each line names people with, in order. */
const SLOTS = { one: ["name"], two: ["first", "second"], three: ["first", "second", "third"], several: [] } as const;
const KEYS: Record<"one" | "two" | "three" | "several", Key> = {
  one: "chat.typing.one",
  two: "chat.typing.two",
  three: "chat.typing.three",
  several: "chat.typing.several",
};

/**
 * "Ana is typing" just above a composer: overlaid on the bottom of the
 * messages, so it never pushes anything around. `nameOf` names someone as
 * the place shows them, or says null to leave them out.
 */
export function TypingLine({ instanceKey, target, nameOf }: { instanceKey: string; target: TypingTarget; nameOf: (id: string) => string | null }) {
  const people = useTypingIn(instanceKey, target);
  const names = Object.keys(people)
    .map(nameOf)
    .filter((n): n is string => n !== null);
  const label = typingLabel(names);
  return (
    <div className="pointer-events-none relative h-0">
      <AnimatePresence initial={false}>
        {label && (
          <motion.div
            key="typing"
            layout
            role="status"
            initial={{ opacity: 0, y: 6, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 6, scale: 0.96 }}
            transition={SPRING}
            className="absolute bottom-1 left-2 z-10 flex max-w-[calc(100%-1rem)] origin-bottom-left items-center gap-1.5 rounded-full bg-background/85 px-2 py-0.5 text-xs text-muted-foreground backdrop-blur-sm"
          >
            <Dots />
            <AnimatePresence mode="popLayout" initial={false}>
              <motion.span
                key={`${label.kind}:${label.names.join("\n")}`}
                initial={{ opacity: 0, y: "0.5em" }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: "-0.5em" }}
                transition={SPRING}
                className="min-w-0 truncate"
              >
                <T
                  k={KEYS[label.kind]}
                  values={Object.fromEntries(label.names.map((name, n) => [SLOTS[label.kind][n], <b className="font-bold text-foreground">{name}</b>]))}
                />
              </motion.span>
            </AnimatePresence>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Three dots that bob one after another; still when motion is turned down. */
function Dots() {
  const system = useReducedMotion();
  const setting = usePrefs((p) => p.reduceMotion);
  const quiet = setting === "always" || (setting === "system" && !!system);
  return (
    <span aria-hidden className="flex shrink-0 items-center gap-0.5">
      {[0, 1, 2].map((n) =>
        quiet ? (
          <span key={n} className="size-1 rounded-full bg-current opacity-70" />
        ) : (
          <motion.span
            key={n}
            className="size-1 rounded-full bg-current"
            animate={{ y: [0, -2.5, 0], opacity: [0.45, 1, 0.45] }}
            transition={{ duration: 0.9, repeat: Infinity, ease: "easeInOut", delay: n * 0.15 }}
          />
        ),
      )}
    </span>
  );
}

/** Names in a server's channel: as the member list shows them. */
export function useServerNames(instanceKey: string, serverId: string) {
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? NO_MEMBERS);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const byId = useMemo(() => new Map(members.map((m) => [m.user?.id ?? "", m])), [members]);
  return useCallback(
    (id: string) => {
      const member = byId.get(id);
      return member ? memberName(member) : displayName(users?.[id]);
    },
    [byId, users],
  );
}

/** Names in a conversation: its people, leaving out anyone you blocked. */
export function useConversationNames(instanceKey: string, conversationId: string) {
  const people = useFuwa((s) => s.instances[instanceKey]?.dms.conversations.find((c) => c.id === conversationId)?.users ?? NO_USERS);
  const friends = useFuwa((s) => s.instances[instanceKey]?.friends.list);
  const blocked = useMemo(() => blockedIds(friends ?? []), [friends]);
  return useCallback((id: string) => (blocked.has(id) ? null : displayName(people.find((u) => u.id === id))), [people, blocked]);
}
