import { ChevronLeftIcon, HashIcon, UsersIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef } from "react";
import { MemberRole, type Channel } from "@/gen/fuwa/v1/types_pb";
import { focusChannel } from "@/fuwa/actions";
import { useInstance } from "@/fuwa/hooks";
import { CHANNEL_ICON, useMyRole } from "@/components/ChannelSidebar";
import { Composer } from "@/components/chat/Composer";
import { MemberList } from "@/components/chat/MemberList";
import { MessageList, type MessageListHandle } from "@/components/chat/MessageList";
import { CopyId } from "@/components/CopyId";
import { ConnDot, connectionLabel } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { SPRING, SwapText } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { setTitle } from "@/lib/notify";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

export function ChannelView({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const inst = useInstance(instanceKey);
  const role = useMyRole(instanceKey, serverId);
  const manager = role >= MemberRole.ADMIN || !!inst?.admin;
  const list = useRef<MessageListHandle>(null);
  const { compact, setNavOpen, membersOpen, setMembersOpen } = useLayout();
  const docked = useMediaQuery("(min-width: 1024px)");
  const Icon = CHANNEL_ICON[channel.type] ?? HashIcon;

  useEffect(() => {
    focusChannel(instanceKey, channel.id);
    return () => focusChannel(null, null);
  }, [instanceKey, channel.id]);

  const serverName = inst?.servers.find((s) => s.id === serverId)?.name;
  useEffect(() => {
    setTitle(`#${channel.name} · ${serverName ?? "fuwa"}`);
  }, [channel.name, serverName]);
  useEffect(() => () => setTitle("fuwa"), []);

  const connection = inst?.connection ?? "connecting";

  return (
    <div className="flex h-full min-h-0">
      <div className="flex min-w-0 flex-1 flex-col">
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
              key={channel.id}
              initial={{ opacity: 0, y: 10 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -10 }}
              transition={SPRING}
              className="flex min-w-0 shrink items-center gap-2"
            >
              <Icon className="size-5 shrink-0 text-muted-foreground" />
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
          <CopyId id={channel.id} what="channel ID" />
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
          <motion.button
            type="button"
            aria-label={membersOpen ? "Hide members" : "Show members"}
            aria-pressed={membersOpen}
            onClick={() => setMembersOpen(!membersOpen)}
            whileTap={{ scale: 0.85 }}
            className={cn(
              "grid size-9 place-items-center rounded-full transition-colors hover:bg-muted",
              membersOpen ? "bg-primary/10 text-primary" : "text-muted-foreground",
            )}
          >
            <motion.span initial={false} animate={{ rotate: membersOpen ? 0 : -12, scale: membersOpen ? 1.08 : 1 }} transition={SPRING}>
              <UsersIcon className="size-5" />
            </motion.span>
          </motion.button>
        </header>
        <MessageList key={channel.id} ref={list} instanceKey={instanceKey} serverId={serverId} channel={channel} manager={manager} />
        <Composer
          instanceKey={instanceKey}
          serverId={serverId}
          channelId={channel.id}
          placeholder={`Message #${channel.name}`}
          onEditLast={() => list.current?.editLast()}
        />
      </div>
      <AnimatePresence initial={false}>
        {membersOpen &&
          (docked ? (
            <motion.aside
              key="members"
              initial={{ width: 0, opacity: 0 }}
              animate={{ width: 240, opacity: 1 }}
              exit={{ width: 0, opacity: 0 }}
              transition={{ type: "spring", stiffness: 400, damping: 40 }}
              className="surface-side h-full shrink-0 overflow-hidden border-l"
            >
              <div className="h-full w-60">
                <MemberList instanceKey={instanceKey} serverId={serverId} />
              </div>
            </motion.aside>
          ) : (
            <motion.div key="members-sheet" className="absolute inset-0 z-30 flex justify-end" initial="closed" animate="open" exit="closed">
              <motion.button
                type="button"
                aria-label="Close members"
                className="absolute inset-0 bg-black/40"
                variants={{ open: { opacity: 1 }, closed: { opacity: 0 } }}
                onClick={() => setMembersOpen(false)}
              />
              <motion.aside
                className="surface-side relative h-full w-72 max-w-[85vw] border-l shadow-2xl"
                variants={{ open: { x: 0 }, closed: { x: "100%" } }}
                transition={{ type: "spring", stiffness: 420, damping: 40 }}
              >
                <MemberList instanceKey={instanceKey} serverId={serverId} />
              </motion.aside>
            </motion.div>
          ))}
      </AnimatePresence>
    </div>
  );
}
