import { ChevronLeftIcon, HashIcon, SnailIcon, UsersIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef } from "react";
import { Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { focusChannel } from "@/fuwa/actions";
import { useAccess } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { CHANNEL_ICON } from "@/components/ChannelSidebar";
import { Composer } from "@/components/chat/Composer";
import { MemberList } from "@/components/chat/MemberList";
import { MessageList, type MessageListHandle } from "@/components/chat/MessageList";
import { NotificationBell } from "@/components/chat/NotificationBell";
import { SharedPill } from "@/components/chat/Shared";
import { SearchBar } from "@/components/search/SearchBar";
import { SearchPanel } from "@/components/search/SearchPanel";
import { useSearch } from "@/fuwa/search";
import { CopyId } from "@/components/CopyId";
import { ConnDot, connectionLabel } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { SPRING, SwapText } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { formatDuration, shortDuration } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { setTitle } from "@/lib/notify";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

export function ChannelView({ instanceKey, serverId, channel }: { instanceKey: string; serverId: string; channel: Channel }) {
  const access = useAccess(instanceKey, serverId);
  const unslowed = hasIn(access, channel.id, Permission.MANAGE_MESSAGES) || hasIn(access, channel.id, Permission.MANAGE_CHANNELS);
  const list = useRef<MessageListHandle>(null);
  const { compact, setNavOpen, membersOpen, setMembersOpen } = useLayout();
  const docked = useMediaQuery("(min-width: 1024px)");
  const wide = useMediaQuery("(min-width: 640px)");
  const Icon = CHANNEL_ICON[channel.type] ?? HashIcon;

  useEffect(() => {
    focusChannel(instanceKey, channel.id);
    return () => focusChannel(null, null);
  }, [instanceKey, channel.id]);

  const serverName = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId)?.name);
  useEffect(() => {
    setTitle(`#${channel.name} · ${serverName ?? "fuwa"}`);
  }, [channel.name, serverName]);
  useEffect(() => () => setTitle("fuwa"), []);

  const connection = useFuwa((s) => s.instances[instanceKey]?.connection ?? "connecting");
  // Search results take the members' place while they're open.
  const searching = useSearch((s) => s.open && s.instanceKey === instanceKey && s.serverId === serverId);

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
          <SharedPill channel={channel} />
          {channel.topic && (
            <>
              <span className="hidden h-5 w-px bg-border sm:block" />
              <InlineMarkdown className="hidden min-w-0 truncate text-sm text-muted-foreground sm:block">{channel.topic}</InlineMarkdown>
            </>
          )}
          <CopyId id={channel.id} what="channel ID" />
          <AnimatePresence initial={false}>
            {channel.slowmodeSeconds > 0 && (
              <motion.span
                key="slowmode"
                initial={{ opacity: 0, scale: 0.6 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.6 }}
                transition={SPRING}
                title={`Slow mode: one message every ${formatDuration(channel.slowmodeSeconds)}${unslowed ? " (not for you)" : ""}`}
                className="flex shrink-0 items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-xs font-bold text-muted-foreground tabular-nums"
              >
                <SnailIcon className="size-3.5" />
                <span className="hidden sm:inline">{shortDuration(channel.slowmodeSeconds)}</span>
              </motion.span>
            )}
          </AnimatePresence>
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
          <SearchBar instanceKey={instanceKey} serverId={serverId} />
          <NotificationBell instanceKey={instanceKey} serverId={serverId} channel={channel} />
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
        <MessageList key={channel.id} ref={list} instanceKey={instanceKey} serverId={serverId} channel={channel} />
        <Composer
          instanceKey={instanceKey}
          serverId={serverId}
          channel={channel}
          placeholder={`Message #${channel.name}`}
          onEditLast={() => list.current?.editLast()}
        />
      </div>
      <AnimatePresence initial={false}>
        {searching &&
          (docked ? (
            <motion.aside
              key="search"
              initial={{ x: 32, opacity: 0 }}
              animate={{ x: 0, opacity: 1 }}
              exit={{ x: 32, opacity: 0 }}
              transition={{ type: "spring", stiffness: 400, damping: 40 }}
              className="surface-side h-full w-[24rem] shrink-0 overflow-hidden border-l"
            >
              <SearchPanel instanceKey={instanceKey} serverId={serverId} />
            </motion.aside>
          ) : (
            <motion.aside
              key="search-sheet"
              initial={{ x: "100%" }}
              animate={{ x: 0 }}
              exit={{ x: "100%" }}
              transition={{ type: "spring", stiffness: 420, damping: 40 }}
              className={cn(
                "surface-side absolute inset-y-0 right-0 z-30 h-full",
                wide ? "w-[24rem] max-w-[85vw] border-l shadow-2xl" : "inset-x-0",
              )}
            >
              <SearchPanel instanceKey={instanceKey} serverId={serverId} sheet={!wide} />
            </motion.aside>
          ))}
        {membersOpen &&
          !searching &&
          (docked ? (
            // The panel takes its width at once and slides in on the compositor: growing its width every frame would
            // lay the whole message list out again each frame.
            <motion.aside
              key="members"
              initial={{ x: 32, opacity: 0 }}
              animate={{ x: 0, opacity: 1 }}
              exit={{ x: 32, opacity: 0 }}
              transition={{ type: "spring", stiffness: 400, damping: 40 }}
              className="surface-side h-full w-60 shrink-0 overflow-hidden border-l"
            >
              <MemberList instanceKey={instanceKey} serverId={serverId} />
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
