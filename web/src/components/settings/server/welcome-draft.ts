import { create } from "@bufbuild/protobuf";
import { WelcomeChannelSchema, WelcomeScreenSchema, type WelcomeScreen } from "@/gen/fuwa/v1/types_pb";

/** A suggested channel being edited, with a key that survives reordering. */
export type Row = { key: string; channelId: string; description: string; emoji: string };

/** The welcome screen as it's being edited. */
export type WelcomeDraft = { enabled: boolean; description: string; list: Row[] };

export const welcomeDraft = (screen: WelcomeScreen): WelcomeDraft => ({
  enabled: screen.enabled,
  description: screen.description,
  list: screen.channels.map((c, n) => ({ key: `${n}-${c.channelId}`, channelId: c.channelId, description: c.description, emoji: c.emoji })),
});

const same = (a: Row[], b: Row[]) =>
  a.length === b.length && a.every((r, n) => r.channelId === b[n]!.channelId && r.description === b[n]!.description && r.emoji === b[n]!.emoji);

/** How many of the welcome screen's settings differ from what's saved. */
export const welcomeChanges = (draft: WelcomeDraft, saved: WelcomeScreen) => {
  const was = welcomeDraft(saved);
  return [draft.enabled !== was.enabled, draft.description !== was.description, !same(draft.list, was.list)].filter(Boolean).length;
};

/** The draft as the server takes it. */
export const welcomeScreen = (draft: WelcomeDraft): WelcomeScreen =>
  create(WelcomeScreenSchema, {
    enabled: draft.enabled,
    description: draft.description.trim(),
    channels: draft.list
      .filter((r) => r.channelId)
      .map((r) => create(WelcomeChannelSchema, { channelId: r.channelId, description: r.description.trim(), emoji: r.emoji })),
  });
