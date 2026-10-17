// Linking a Discord server to a fuwa server: /migrate in Discord gives a
// short code, and running /import with it in the fuwa server proves the same
// person (or team) manages both. Codes live only in memory, work once, and
// run out; a fuwa server gets a few wrong tries before it has to wait.
import { randomInt } from "node:crypto";

/** Letters and digits nobody mixes up when typing them over. */
const ALPHABET = "ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LENGTH = 8;
export const CODE_MS = 30 * 60_000;
const TRIES = 5;
const TRIES_MS = 10 * 60_000;

/** What the person who ran /migrate asked for. */
export interface Pending {
  guildId: string;
  /** The Discord channel /migrate was run in, for progress there. */
  channelId: string;
  userId: string;
  messagesPerChannel: number;
  /** A Discord channel to post the invite in at the end. */
  announceChannelId?: string;
}

export function newCode(): string {
  let code = "";
  for (let i = 0; i < CODE_LENGTH; i++) code += ALPHABET[randomInt(ALPHABET.length)];
  return `${code.slice(0, 4)}-${code.slice(4)}`;
}

/** A code as typed: any case, with or without the dash or spaces. */
export function normalize(code: string): string {
  const bare = code.toUpperCase().replace(/[^A-Z0-9]/g, "");
  return bare.length === CODE_LENGTH ? `${bare.slice(0, 4)}-${bare.slice(4)}` : bare;
}

export type Redeemed = { ok: true; pending: Pending } | { ok: false; why: "wrong" | "slow down" };

export class Links {
  readonly #codes = new Map<string, { pending: Pending; expires: number }>();
  readonly #misses = new Map<string, number[]>();
  readonly #now: () => number;

  constructor(now = () => Date.now()) {
    this.#now = now;
  }

  /** A new code for a guild; any older one for it stops working. */
  create(pending: Pending): string {
    this.#sweep();
    for (const [code, entry] of this.#codes) if (entry.pending.guildId === pending.guildId) this.#codes.delete(code);
    let code = newCode();
    while (this.#codes.has(code)) code = newCode();
    this.#codes.set(code, { pending, expires: this.#now() + CODE_MS });
    return code;
  }

  /** Uses a code from a fuwa server (`from`), once. */
  redeem(code: string, from: string): Redeemed {
    this.#sweep();
    const now = this.#now();
    const misses = (this.#misses.get(from) ?? []).filter((t) => now - t < TRIES_MS);
    if (misses.length >= TRIES) return { ok: false, why: "slow down" };
    const key = normalize(code);
    const entry = this.#codes.get(key);
    if (!entry) {
      this.#misses.set(from, [...misses, now]);
      return { ok: false, why: "wrong" };
    }
    this.#codes.delete(key);
    return { ok: true, pending: entry.pending };
  }

  #sweep(): void {
    const now = this.#now();
    for (const [code, entry] of this.#codes) if (entry.expires <= now) this.#codes.delete(code);
    for (const [from, times] of this.#misses) if (times.every((t) => now - t >= TRIES_MS)) this.#misses.delete(from);
  }
}
