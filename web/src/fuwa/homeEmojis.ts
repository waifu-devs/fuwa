import { useEffect, useSyncExternalStore } from "react";
import type { Emoji } from "@/gen/fuwa/v1/types_pb";
import { engine } from "./sync";

/*
 * The custom emoji of a channel's home server, for the picker in a channel
 * this server shows from another (ListEmojis with the channel). Asked once
 * a visit per channel; the home's people's changes show on the next visit.
 */

const kept = new Map<string, Emoji[]>();
const asking = new Set<string>();
const listeners = new Set<() => void>();
const subscribe = (l: () => void) => (listeners.add(l), () => listeners.delete(l));
const NONE: Emoji[] = [];

function ask(key: string, serverId: string, channelId: string) {
  const id = `${key}|${channelId}`;
  if (kept.has(id) || asking.has(id)) return;
  asking.add(id);
  engine(key)
    .api.emojis.listEmojis({ serverId, channelId })
    .then(
      (r) => kept.set(id, r.emojis),
      // Its home's emoji just aren't offered; the next visit asks again.
      () => {},
    )
    .finally(() => {
      asking.delete(id);
      for (const l of listeners) l();
    });
}

/** The home's emoji, or none while they load or when `channelId` is empty. */
export function useHomeEmojis(key: string, serverId: string, channelId: string): Emoji[] {
  useEffect(() => {
    if (channelId) ask(key, serverId, channelId);
  }, [key, serverId, channelId]);
  return useSyncExternalStore(subscribe, () => (channelId ? (kept.get(`${key}|${channelId}`) ?? NONE) : NONE));
}
