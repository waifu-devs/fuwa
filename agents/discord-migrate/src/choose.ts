// What someone running /migrate start chooses to copy: the parts of the
// server, the channels to leave out, and how much history, which by default
// shrinks as the server has more channels. Pure.
import { PARTS, type Part } from "./migrate.ts";

/** Each part as the picker names it. */
export const PART_NAMES: Record<Part, string> = {
  server: "Name, description, icon and banner",
  roles: "Roles and their permissions",
  emoji: "Emoji and stickers",
  sounds: "Soundboard sounds",
  channels: "Categories and channels",
  invite: "An invite for your members",
  history: "Message history",
};

/** The fewest messages a channel gets by default, however big the server. */
export const MIN_PER_CHANNEL = 100;

/**
 * How many of each channel's latest messages to copy unless told: `budget`
 * shared out over the server's text channels, from MIN_PER_CHANNEL up to `most`.
 */
export function perChannel(channels: number, budget: number, most: number): number {
  if (most <= 0) return 0;
  const share = Math.floor(budget / Math.max(1, channels));
  return Math.min(most, Math.max(Math.min(MIN_PER_CHANNEL, most), share));
}

export interface Choice {
  parts: Part[];
  /** Discord channels (and categories) to leave out. */
  leaveOut: string[];
  messagesPerChannel: number;
  /** Only messages from the last this many days; all of the latest ones when 0. */
  days: number;
}

/** What's wrong with a choice, in words for the person making it; empty when nothing is. */
export function problems(choice: Choice): string[] {
  const has = (p: Part) => choice.parts.includes(p);
  const out: string[] = [];
  if (!choice.parts.length) out.push("Pick at least one thing to copy.");
  if (has("history") && !has("channels")) out.push("Message history needs the channels too, to go in.");
  return out;
}

const count = (n: number) => n.toLocaleString("en-US");

/** The choice in a sentence or two: what gets copied, and how much history. */
export function choiceText(choice: Choice): string {
  const parts = PARTS.filter((p) => choice.parts.includes(p));
  const named = parts.filter((p) => p !== "history").map((p) => PART_NAMES[p].toLowerCase());
  const lines = [named.length ? `It copies: ${named.join("; ")}.` : "It copies no part of the server's setup."];
  if (choice.parts.includes("history") && choice.messagesPerChannel > 0) {
    const window = choice.days ? `, from the last ${choice.days === 1 ? "day" : `${count(choice.days)} days`}` : "";
    lines.push(
      `History: up to the latest ${count(choice.messagesPerChannel)} messages of each text channel${window}, ` +
        "with their threads and forum posts.",
    );
  } else {
    lines.push("No message history.");
  }
  if (choice.leaveOut.length) {
    lines.push(
      `Left out: ${choice.leaveOut.map((id) => `<#${id}>`).join(", ")} (a category leaves out the channels in it).`,
    );
  }
  return lines.join("\n");
}
