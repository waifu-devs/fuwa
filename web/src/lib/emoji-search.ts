/**
 * The emoji pickers' decisions, apart from React and the store so node's
 * test runner reads this file as is (see emoji-catalog.ts for the rest).
 *
 * In the composer server emoji are written `:name:`. When two servers have
 * the same name the later one is `:name~2:` (then `~3`), so each name finds
 * one.
 */

import type { Emoji, Server } from "@/gen/fuwa/v1/types_pb";
import type { I18n } from "../i18n/i18n.ts";
import { EMOJI_TOKEN, emojiToken } from "./emoji.ts";

/** One standard emoji: how it's written (its first name), other names, words to find it by, and its five skin tones. */
export type StandardEmoji = { char: string; names: string[]; words: string; skins?: string[] };
export type StandardGroup = { id: string; name: string; emojis: StandardEmoji[] };

/** A standard emoji in a skin tone: 0 is the default yellow, 1–5 light to dark. */
export const toned = (e: StandardEmoji, tone: number) => (tone > 0 && e.skins ? (e.skins[tone - 1] ?? e.char) : e.char);

export type ServerRef = Pick<Server, "id" | "name" | "iconUrl">;
/** A server's emoji with the name the composer writes it by. */
export type CustomEmoji = { emoji: Emoji; alias: string; server: ServerRef; here: boolean };
export type CustomSection = { server: ServerRef; here: boolean; emojis: CustomEmoji[] };

export type Catalog = {
  /** This server's first (when it has any), then the others in the order they were joined. */
  sections: CustomSection[];
  /** By lowercase alias. */
  byAlias: Map<string, CustomEmoji>;
  /** By id. */
  byId: Map<string, CustomEmoji>;
};

const NO_EMOJI: Emoji[] = [];

/** The catalog for writing in `serverId`, from what the instance has loaded. */
export function catalogOf(
  servers: Server[],
  emojis: Record<string, Emoji[]>,
  serverId: string,
): Catalog {
  const here = servers.find((s) => s.id === serverId);
  const order = [...(here ? [here] : []), ...servers.filter((s) => s.id !== serverId)];
  const byAlias = new Map<string, CustomEmoji>();
  const byId = new Map<string, CustomEmoji>();
  const sections: CustomSection[] = [];
  for (const server of order) {
    const list = [...(emojis[server.id] ?? NO_EMOJI)].sort((a, b) => a.name.localeCompare(b.name));
    if (!list.length) continue;
    const isHere = server.id === serverId;
    const section: CustomSection = { server: { id: server.id, name: server.name, iconUrl: server.iconUrl }, here: isHere, emojis: [] };
    for (const emoji of list) {
      let alias = emoji.name;
      for (let n = 2; byAlias.has(alias.toLowerCase()); n++) alias = `${emoji.name}~${n}`;
      const entry: CustomEmoji = { emoji, alias, server: section.server, here: isHere };
      byAlias.set(alias.toLowerCase(), entry);
      byId.set(emoji.id, entry);
      section.emojis.push(entry);
    }
    sections.push(section);
  }
  return { sections, byAlias, byId };
}

/** A catalog of only these emoji, for places that offer one server's own (the welcome screen). */
export function ownCatalog(t: I18n["t"], server: ServerRef | undefined, emojis: Emoji[] | undefined): Catalog {
  const ref = server ?? { id: "", name: t("system.emoji.thisServer"), iconUrl: "" };
  return catalogOf([ref as Server], { [ref.id]: emojis ?? NO_EMOJI }, ref.id);
}

/** `:name:` (or `:name~2:`) written for a server emoji, but not inside an emoji's own token. */
const SHORTCODE = /(?<!<a?):([A-Za-z0-9_]{2,32}(?:~\d{1,3})?):/g;

/** The text with each `:name:` of a catalog emoji made into its token. */
export function encodeEmoji(content: string, catalog: Catalog) {
  if (!catalog.byAlias.size) return content;
  return content.replace(SHORTCODE, (whole, alias: string) => {
    const entry = catalog.byAlias.get(alias.toLowerCase());
    return entry ? emojiToken(entry.emoji) : whole;
  });
}

/**
 * Other servers' emoji that `content` writes as tokens, to send along with
 * it. The instance checks each one; any it won't keep show as their names.
 */
export function outsideEmojis(emojis: Record<string, Emoji[]> | undefined, serverId: string, content: string): Emoji[] {
  if (!emojis || !content.includes("<")) return [];
  const out: Emoji[] = [];
  const seen = new Set<string>();
  for (const m of content.matchAll(EMOJI_TOKEN)) {
    const id = m[3]!;
    if (seen.has(id)) continue;
    seen.add(id);
    for (const [owner, list] of Object.entries(emojis)) {
      if (owner === serverId) continue;
      const emoji = list.find((e) => e.id === id);
      if (emoji) {
        out.push(emoji);
        break;
      }
    }
  }
  return out;
}

// ───────────────────────── Finding them ─────────────────────────

/** One emoji to pick, from a server or the standard set. */
export type Choice =
  | { kind: "custom"; key: string; custom: CustomEmoji }
  | { kind: "standard"; key: string; emoji: StandardEmoji };

export const customChoice = (custom: CustomEmoji): Choice => ({ kind: "custom", key: `c:${custom.emoji.id}`, custom });
export const standardChoice = (emoji: StandardEmoji): Choice => ({ kind: "standard", key: `u:${emoji.char}`, emoji });

/** How a choice is written after a colon. */
export const choiceName = (c: Choice) => (c.kind === "custom" ? c.custom.alias : c.emoji.names[0]!);

/**
 * Emoji whose names (or, for standard ones, words) match `query`: whole
 * names first, then names that start with it, then ones that have it;
 * within each, this server's, then other servers', then standard ones.
 */
export function searchCatalog(query: string, catalog: Catalog, groups: StandardGroup[] | null, limit = Infinity): Choice[] {
  const q = query.trim().toLowerCase().replace(/^:|:$/g, "");
  if (!q) return [];
  const ranked: { choice: Choice; rank: number; order: number }[] = [];
  const nameRank = (name: string) => (name === q ? 0 : name.startsWith(q) ? 1 : name.includes(q) ? 2 : -1);
  let order = 0;
  for (const section of catalog.sections) {
    for (const custom of section.emojis) {
      const best = [custom.alias, custom.emoji.name].map((n) => nameRank(n.toLowerCase())).filter((x) => x >= 0);
      if (best.length) ranked.push({ choice: customChoice(custom), rank: Math.min(...best) * 3 + (custom.here ? 0 : 1), order: order++ });
    }
  }
  for (const group of groups ?? []) {
    for (const emoji of group.emojis) {
      const best = emoji.names.map(nameRank).filter((x) => x >= 0);
      if (best.length) ranked.push({ choice: standardChoice(emoji), rank: Math.min(...best) * 3 + 2, order: order++ });
      else if (q.length >= 2 && emoji.words.split(" ").some((w) => w.startsWith(q)))
        ranked.push({ choice: standardChoice(emoji), rank: 9, order: order++ });
    }
  }
  ranked.sort((a, b) => a.rank - b.rank || a.order - b.order);
  return ranked.slice(0, limit).map((r) => r.choice);
}
