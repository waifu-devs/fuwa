import { createContext } from "react";
import type { Emoji } from "@/gen/fuwa/v1/types_pb";

/** Where the polls in a message list are, and what you may do with them there. */
export type PollPlaceValue = {
  instanceKey: string;
  serverId: string;
  channelId: string;
  /** You can vote here: not waiting to agree to the rules, nor reading a channel from another server's home that this instance can't vote in. */
  canVote: boolean;
  /** You can end other people's polls here (Manage Messages). */
  moderator: boolean;
  emojis: Emoji[] | undefined;
};

export const PollPlace = createContext<PollPlaceValue | null>(null);
