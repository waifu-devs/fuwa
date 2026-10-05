import type { Channel, WelcomeScreen } from "@/gen/fuwa/v1/types_pb";

/** A suggested channel as a card: its emoji or kind, its name and why to go there. */
export type Suggested = { channelId: string; description: string; emoji: string; channel: Channel };

/** The welcome screen's channels that exist and the caller can see, with their channel. */
export function suggestedChannels(screen: Pick<WelcomeScreen, "channels">, channels: Channel[]): Suggested[] {
  return screen.channels.flatMap((w) => {
    const channel = channels.find((c) => c.id === w.channelId);
    return channel ? [{ channelId: w.channelId, description: w.description, emoji: w.emoji, channel }] : [];
  });
}
