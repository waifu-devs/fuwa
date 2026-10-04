import { createContext, useCallback, useContext, useEffect, useState, useSyncExternalStore, type RefObject } from "react";
import type { MessageListHandle } from "@/components/chat/MessageList";
import { loadFollowed, loadMessages, run } from "@/fuwa/actions";
import { store } from "@/fuwa/store";
import { toast } from "@/lib/ui";
import type { ThreadSummary } from "@/gen/fuwa/v1/types_pb";
import { toDate } from "@/lib/format";

/** Opens the thread under a message; given by the channel on screen. Stable for the channel's life. */
const ThreadOpener = createContext<((threadId: string) => void) | null>(null);
export const ThreadOpenerProvider = ThreadOpener.Provider;
export const useThreadOpener = () => useContext(ThreadOpener);

// ── A thread asked for from elsewhere (a notification), opened once its channel is on screen.

let wanted: { channelId: string; threadId: string } | null = null;
const wantedListeners = new Set<() => void>();

export function requestThread(channelId: string, threadId: string) {
  wanted = { channelId, threadId };
  for (const l of wantedListeners) l();
}

/** The thread asked for in this channel, if any, and a way to say it's been opened. */
export function useWantedThread(channelId: string): [string | null, () => void] {
  const id = useSyncExternalStore(
    (l) => {
      wantedListeners.add(l);
      return () => wantedListeners.delete(l);
    },
    () => (wanted?.channelId === channelId ? wanted.threadId : null),
  );
  return [id, takeWanted];
}

function takeWanted() {
  wanted = null;
  for (const l of wantedListeners) l();
}

/** Whether nobody has replied for longer than the server keeps threads open. */
export function isArchived(thread: ThreadSummary | undefined, hours: number, now = Date.now()) {
  return !!thread && hours > 0 && now - toDate(thread.lastReplyAt).getTime() > hours * 3_600_000;
}

/** What sits beside a channel for threads: one open thread, or the channel's list of them. */
export type ThreadPanelState = { kind: "thread"; id: string } | { kind: "threads" } | null;

/**
 * The thread panel's state for the channel on screen: closed again when the
 * channel changes, and opened on a thread asked for from a notification.
 */
export function useThreadPanel(
  instanceKey: string,
  serverId: string,
  channel: { id: string; shared?: unknown },
  list: RefObject<MessageListHandle | null>,
  docked: boolean,
) {
  const channelId = channel.id;
  const [panel, setPanel] = useState<ThreadPanelState>(null);
  const [shownFor, setShownFor] = useState(channelId);
  if (shownFor !== channelId) {
    setShownFor(channelId);
    setPanel(null);
  }
  const openThread = useCallback((id: string) => setPanel({ kind: "thread", id }), []);
  const closePanel = useCallback(() => setPanel(null), []);
  const [wantedId, taken] = useWantedThread(channelId);
  if (wantedId && !(panel?.kind === "thread" && panel.id === wantedId)) setPanel({ kind: "thread", id: wantedId });
  useEffect(() => {
    if (wantedId) taken();
  }, [wantedId, taken]);
  // The threads you follow here, so their replies reach you.
  useEffect(() => {
    if (!channel.shared) run(loadFollowed(instanceKey, serverId)).catch(() => {});
  }, [instanceKey, serverId, channel.shared]);
  /** Shows a thread's message in the channel; on a narrow screen the thread steps aside first. */
  const jump = (id: string) => {
    if (!docked) setPanel(null);
    void jumpToParent(list, instanceKey, serverId, channelId, id);
  };
  return { panel, setPanel, openThread, closePanel, jump };
}

/** Finds the message a thread is under in the channel, reading back through it a few pages if it has to. */
export async function jumpToParent(
  list: RefObject<MessageListHandle | null>,
  instanceKey: string,
  serverId: string,
  channelId: string,
  id: string,
) {
  for (let pages = 0; pages < 10; pages++) {
    await new Promise((r) => requestAnimationFrame(r));
    if (list.current?.jumpTo(id)) return;
    const loaded = store.get().instances[instanceKey]?.messages[channelId];
    if (!loaded?.hasMore) break;
    await run(loadMessages(instanceKey, serverId, channelId, true)).catch(() => {});
  }
  toast("That message is too far back to jump to");
}
