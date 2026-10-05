import { HashIcon, MegaphoneIcon, ShieldCheckIcon, Volume2Icon } from "lucide-react";
import { ChannelType, type Channel } from "@/gen/fuwa/v1/types_pb";

export const CHANNEL_ICON: Partial<Record<ChannelType, typeof HashIcon>> = {
  [ChannelType.ANNOUNCEMENT]: MegaphoneIcon,
  [ChannelType.VOICE]: Volume2Icon,
  [ChannelType.SECURE]: ShieldCheckIcon,
};

export type Group = { category: Channel | null; channels: Channel[] };

/** Channels you can open, in the order the sidebar lists them. */
export const openableChannels = (channels: Channel[]) =>
  groupChannels(channels)
    .flatMap((g) => g.channels)
    .filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT || c.type === ChannelType.SECURE);

export function groupChannels(channels: Channel[]): Group[] {
  const categories = channels.filter((c) => c.type === ChannelType.CATEGORY);
  const known = new Set(categories.map((c) => c.id));
  const loose = channels.filter((c) => c.type !== ChannelType.CATEGORY && !known.has(c.parentId));
  return [
    { category: null, channels: loose },
    ...categories.map((category) => ({
      category,
      channels: channels.filter((c) => c.parentId === category.id && c.type !== ChannelType.CATEGORY),
    })),
  ];
}
