import { BadgeCheckIcon, BotOffIcon, ChevronLeftIcon, ImageOffIcon, LockKeyholeIcon, SearchXIcon, ShieldCheckIcon, ShieldOffIcon, UserPlusIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Permission, type Channel, type Member, type User } from "@/gen/fuwa/v1/types_pb";
import type { Item } from "@/e2ee/vault";
import { focusChannel } from "@/fuwa/actions";
import { markDmRead, prepareSecureChannel } from "@/fuwa/dms";
import { useAccess } from "@/fuwa/hooks";
import { useFuwa, type DmMember, type DmState } from "@/fuwa/store";
import { NotificationBell } from "@/components/chat/NotificationBell";
import { EncryptedComposer, EncryptedMessages, Starting, Unavailable } from "@/components/dm/DmView";
import { Padlock } from "@/components/dm/Padlock";
import { ConnDot, connectionLabel, UserAvatar } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { SPRING, SwapText } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { displayName, memberName } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { setTitle } from "@/lib/notify";
import { cn } from "@/lib/utils";

const NO_MEMBERS: DmMember[] = [];
const NO_SERVER_MEMBERS: Member[] = [];

/** What a secure channel can't do, said once: the server can't read it, so nothing that needs to can work. */
const CANT = [
  { icon: ShieldOffIcon, text: "AutoMod can't check messages here" },
  { icon: SearchXIcon, text: "Search can't find them" },
  { icon: BotOffIcon, text: "Bots, agents and webhooks can't post or read" },
  { icon: ImageOffIcon, text: "Links stay links: no previews or inline pictures" },
  { icon: UserPlusIcon, text: "People who join later only see what's sent after they join" },
];

/**
 * A secure channel: a server's channel whose messages are end-to-end
 * encrypted between the devices of the people who can see it. It reads
 * and writes through this browser's encryption (`@/e2ee/engine`), like a
 * direct message, and never through the server's messages.
 */
export function SecureChannelView({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const status = useFuwa((s) => s.instances[instanceKey]?.dms.status ?? "off");
  const problem = useFuwa((s) => s.instances[instanceKey]?.dms.problem ?? null);
  const me = useFuwa((s) => s.instances[instanceKey]?.me ?? undefined);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? NO_SERVER_MEMBERS);
  const connection = useFuwa((s) => s.instances[instanceKey]?.connection ?? "connecting");
  const serverName = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.name);
  const access = useAccess(instanceKey, serverId);
  const { compact, setNavOpen } = useLayout();
  const [info, setInfo] = useState(false);
  const id = channel.id;

  useEffect(() => {
    focusChannel(instanceKey, id);
    void markDmRead(instanceKey, id);
    return () => focusChannel(null, null);
  }, [instanceKey, id]);
  useEffect(() => {
    setTitle(`#${channel.name} · ${serverName ?? "fuwa"}`);
  }, [channel.name, serverName]);
  useEffect(() => () => setTitle("fuwa"), []);

  // Once encryption is running here: catch up, and bring in everyone who can see the channel.
  useEffect(() => {
    if (status === "ready") void prepareSecureChannel(instanceKey, serverId, id).catch(() => {});
  }, [status, instanceKey, serverId, id]);

  const byId = useMemo(() => new Map(members.map((m) => [m.user?.id ?? "", m])), [members]);
  const userOf = useCallback((userId: string) => byId.get(userId)?.user ?? users?.[userId], [byId, users]);
  const memberOf = useCallback((userId: string) => byId.get(userId), [byId]);
  const describe = useCallback((item: Item) => (me ? channelLine(item, (u) => nameIn(byId, users, u), me) : ""), [byId, users, me]);
  const canSend = hasIn(access, id, Permission.SEND_MESSAGES);

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-2 sm:px-4">
        {compact && (
          <button
            type="button"
            aria-label="Channels"
            onClick={() => setNavOpen(true)}
            className="grid size-9 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted"
          >
            <ChevronLeftIcon className="size-5" />
          </button>
        )}
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span
            key={id}
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -10 }}
            transition={SPRING}
            className="flex min-w-0 shrink items-center gap-2"
          >
            <ShieldCheckIcon className="size-5 shrink-0 text-emerald-600 dark:text-emerald-400" />
            <h1 className="truncate font-extrabold">
              <SwapText className="truncate align-bottom">{channel.name}</SwapText>
            </h1>
          </motion.span>
        </AnimatePresence>
        {channel.topic && (
          <>
            <span className="hidden h-5 w-px bg-border sm:block" />
            <InlineMarkdown className="hidden min-w-0 truncate text-sm text-muted-foreground sm:block">{channel.topic}</InlineMarkdown>
          </>
        )}
        <span className="flex-1" />
        <AnimatePresence>
          {connection !== "live" && (
            <motion.span
              initial={{ opacity: 0, scale: 0.9 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.9 }}
              className="flex items-center gap-1.5 rounded-full bg-muted px-2.5 py-1 text-xs font-bold text-muted-foreground"
            >
              <ConnDot state={connection} /> {connectionLabel(connection)}
            </motion.span>
          )}
        </AnimatePresence>
        <NotificationBell instanceKey={instanceKey} serverId={serverId} channel={channel} />
        <motion.button
          type="button"
          onClick={() => setInfo(true)}
          whileTap={{ scale: 0.92 }}
          initial={{ opacity: 0, scale: 0.8 }}
          animate={{ opacity: 1, scale: 1 }}
          transition={SPRING}
          title="See who can read this channel"
          className="group relative flex shrink-0 items-center gap-1.5 overflow-hidden rounded-full bg-emerald-500/12 px-2.5 py-1 text-xs font-bold text-emerald-700 transition-colors hover:bg-emerald-500/20 dark:text-emerald-300"
        >
          <span aria-hidden className="shine pointer-events-none absolute inset-0" />
          <LockKeyholeIcon className="size-3.5 transition-transform duration-300 group-hover:-rotate-12 group-hover:scale-110" />
          <span className="hidden sm:inline">End-to-end encrypted</span>
        </motion.button>
      </header>
      {me && status === "ready" ? (
        <>
          <EncryptedMessages
            instanceKey={instanceKey}
            id={id}
            me={me}
            userOf={userOf}
            memberOf={memberOf}
            describe={describe}
            beginning={<SecureBeginning channel={channel} />}
            canModerate={hasIn(access, id, Permission.MANAGE_MESSAGES)}
            deleteQuestion="Delete for everyone?"
            joiningText="Unlocking the channel on this device…"
          />
          <EncryptedComposer
            instanceKey={instanceKey}
            id={id}
            placeholder={`Message #${channel.name}`}
            promise="Only people in this channel can read this"
            locked={canSend ? "" : "You don't have permission to send messages in this channel."}
          />
          <SecureChannelDialog open={info} onOpenChange={setInfo} instanceKey={instanceKey} channel={channel} byId={byId} />
        </>
      ) : status === "unsupported" || status === "failed" ? (
        <Unavailable text={problem ?? "Encrypted messages aren't available here."} />
      ) : (
        <Starting />
      )}
    </div>
  );
}

function nameIn(byId: Map<string, Member>, users: Record<string, User> | undefined, userId: string) {
  const member = byId.get(userId);
  return member ? memberName(member) : displayName(users?.[userId]);
}

/** What changed about the channel's devices, in words, from the commit itself (not from the server). */
export function channelLine(item: Item, nameOf: (userId: string) => string, me: User): string {
  const name = (id: string) => (id === me.id ? "you" : nameOf(id));
  const whose = (id: string) => (id === me.id ? "your" : `${nameOf(id)}'s`);
  const capital = (text: string) => `${text[0]?.toUpperCase() ?? ""}${text.slice(1)}`;
  if (item.kind === "joined") return "This device joined the channel. Messages from before it can't be read here.";
  if (item.kind === "unreadable") return `A message from ${name(item.senderId)} couldn't be opened on this device.`;
  const devices = (list: Item["added"]) =>
    [...new Set(list.map((d) => d.userId))].map((userId) => {
      const n = list.filter((d) => d.userId === userId).length;
      return `${whose(userId)} ${n === 1 ? "device" : `${n} devices`}`;
    });
  const added = devices(item.added);
  const removed = devices(item.removed);
  const by = name(item.senderId);
  const alone = item.added.length === 1 && item.added[0].userId === item.senderId && !item.removed.length;
  if (item.seq === 1) {
    return added.length ? `${capital(by)} started this secure channel and added ${list(added)}.` : `${capital(by)} started this secure channel.`;
  }
  if (alone) return `${capital(by)} came in on a new device.`;
  const parts = [added.length ? `added ${list(added)}` : "", removed.length ? `removed ${list(removed)}` : ""].filter(Boolean);
  return parts.length ? `${capital(by)} ${parts.join(", and ")}.` : `${capital(by)} refreshed the channel's keys.`;
}

const list = (parts: string[]) => (parts.length < 2 ? parts.join("") : `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}`);

function SecureBeginning({ channel }: { channel: Channel }) {
  return (
    <motion.div initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={{ ...SPRING, delay: 0.05 }} className="px-4 pt-10 pb-4">
      <span className="relative inline-grid size-16 place-items-center rounded-2xl bg-emerald-500/15 text-emerald-600 dark:text-emerald-400">
        <ShieldCheckIcon className="size-8" />
        <motion.span
          initial={{ scale: 0, rotate: -30 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.45 }}
          className="absolute -right-1.5 -bottom-1.5 grid size-7 place-items-center rounded-full bg-emerald-500 text-white ring-4 ring-card"
        >
          <LockKeyholeIcon className="size-3.5" />
        </motion.span>
      </span>
      <h2 className="mt-3 text-2xl font-extrabold sm:text-3xl">Welcome to #{channel.name}</h2>
      <p className="mt-3 flex max-w-xl items-start gap-2 rounded-2xl bg-emerald-500/10 px-3 py-2.5 text-sm text-emerald-900 dark:text-emerald-100">
        <LockKeyholeIcon className="mt-0.5 size-4 shrink-0 text-emerald-600 dark:text-emerald-400" />
        <span>
          This is a <b>secure channel</b>. Messages here are end-to-end encrypted: only the people in this channel can read them, on their own
          devices. Not this fuwa server, and not whoever runs it. That means AutoMod, search, link previews, bots and agents don't work here, and
          people who join later only see messages sent after they join.
        </span>
      </p>
    </motion.div>
  );
}

/** Whether you checked someone's safety number in a direct message, and every device of theirs in this channel was in it. */
function verifiedPeople(dms: DmState, meId: string, channelMembers: DmMember[]): Set<string> {
  const hex = (key: Uint8Array) => Array.from(key, (b) => b.toString(16).padStart(2, "0")).join("");
  const out = new Set<string>();
  for (const userId of new Set(channelMembers.map((m) => m.userId))) {
    if (userId === meId) continue;
    const c = dms.conversations.find((x) => x.users.some((u) => u.id === userId) && x.users.some((u) => u.id === meId));
    if (!c || !dms.safety[c.id] || dms.verified[c.id] !== dms.safety[c.id]) continue;
    const checked = new Set((dms.members[c.id] ?? []).filter((m) => m.userId === userId).map((m) => hex(m.signatureKey)));
    if (channelMembers.filter((m) => m.userId === userId).every((m) => checked.has(hex(m.signatureKey)))) out.add(userId);
  }
  return out;
}

/** How a secure channel is kept private: what the server can't do, and every person (and how many devices) that can read it. */
function SecureChannelDialog({
  open,
  onOpenChange,
  instanceKey,
  channel,
  byId,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  channel: Channel;
  byId: Map<string, Member>;
}) {
  const me = useFuwa((s) => s.instances[instanceKey]?.me);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const dms = useFuwa((s) => s.instances[instanceKey]?.dms);
  const members = dms?.members[channel.id] ?? NO_MEMBERS;
  const people = useMemo(() => {
    const counts = new Map<string, number>();
    for (const m of members) counts.set(m.userId, (counts.get(m.userId) ?? 0) + 1);
    return [...counts].sort(([a], [b]) => nameIn(byId, users, a).localeCompare(nameIn(byId, users, b)));
  }, [members, byId, users]);
  const verified = useMemo(() => (dms && me ? verifiedPeople(dms, me.id, members) : new Set<string>()), [dms, me, members]);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <div className="mb-4 flex justify-center">
          <Padlock delay={0.1} />
        </div>
        <DialogHeader
          title={`#${channel.name} is end-to-end encrypted`}
          description="Messages are locked on the sender's device and only open on the devices below. This fuwa server keeps and passes along what it can't read."
        />
        <ul className="grid gap-1.5 rounded-2xl border bg-muted/40 p-3 text-sm">
          {CANT.map(({ icon: Icon, text }, n) => (
            <motion.li
              key={text}
              initial={{ opacity: 0, x: -8 }}
              animate={{ opacity: 1, x: 0 }}
              transition={{ ...SPRING, delay: 0.1 + n * 0.04 }}
              className="flex items-center gap-2.5 text-muted-foreground"
            >
              <Icon className="size-4 shrink-0" /> {text}
            </motion.li>
          ))}
        </ul>
        <p className="mt-5 mb-2 text-sm font-extrabold">
          Who can read it <span className="font-bold text-muted-foreground">· {people.length}</span>
        </p>
        <ul className="scroll-thin -mx-1 max-h-64 overflow-y-auto px-1">
          {people.map(([userId, count], n) => {
            const user = byId.get(userId)?.user ?? users?.[userId];
            return (
              <motion.li
                key={userId}
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ ...SPRING, delay: Math.min(n, 10) * 0.03 }}
                className="flex items-center gap-3 rounded-xl px-2 py-1.5 hover:bg-muted/60"
              >
                <UserAvatar user={user} className="size-8 text-xs" />
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5 truncate font-bold">
                    {userId === me?.id ? "You" : nameIn(byId, users, userId)}
                    {verified.has(userId) && (
                      <BadgeCheckIcon className="size-4 text-emerald-500" aria-label="Verified in your direct messages" />
                    )}
                  </span>
                  <span className="block text-xs text-muted-foreground">
                    {count === 1 ? "1 device" : `${count} devices`}
                    {verified.has(userId) && " · verified in your direct messages"}
                  </span>
                </span>
              </motion.li>
            );
          })}
        </ul>
        <p className={cn("mt-4 text-xs text-muted-foreground")}>
          Who's in it follows the channel's permissions. Compare safety numbers in a direct message to verify someone's devices.
        </p>
      </DialogContent>
    </Dialog>
  );
}
