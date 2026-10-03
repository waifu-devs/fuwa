/**
 * Which entries of shared history a device takes. An entry's signature has
 * already been checked against the key it carries; this decides whether
 * that key, and where the sharer placed the entry, can be trusted.
 */

/** One shared entry, as its signed payload says, placed by the sharer at `seq`. */
export type Candidate = {
  seq: number;
  senderId: string;
  /** The device whose key signed it. */
  deviceId: string;
  /** "edit" entries name the message they change, inside what was signed. */
  kind: "text" | "edit";
  target?: number;
};

/** What the channel's log says about a record, read from its header: nothing decrypted. */
export type Logged = {
  kind: "message" | "settings" | "other";
  senderId: string;
  deviceId: string;
  deleted: boolean;
  /** For a settings record: whether it turned sharing on. */
  on?: boolean;
};

export type Context = {
  /** The sequence this device joined at: only what came before it is taken. */
  joined: number;
  /** Who is in the channel. */
  allowed: string[];
  /** `${userId}/${deviceId}` for every device registered to its owner now. */
  devices: Set<string>;
  /** The log between the earliest entry and `joined`. */
  log: Map<number, Logged>;
};

/**
 * Indexes of the entries to take, in order:
 * - signed by a device registered to the person named as sender (anyone can
 *   sign with a key of their own, so a key that isn't theirs proves nothing);
 * - the sender is in the channel;
 * - before this device joined, and after sharing was last turned on, so what
 *   was written while it was off stays with those who were there;
 * - the log has a message at that place, from that sender (and, for new text,
 *   that very device), not deleted, so a sharer can't move, repeat or bring
 *   back messages;
 * - each message at most once, and an edit only of a message taken here.
 */
export function accept(entries: Candidate[], ctx: Context): number[] {
  let since = 0;
  let on = true;
  for (const [seq, r] of ctx.log) {
    if (r.kind === "settings" && seq < ctx.joined && seq > since) {
      since = seq;
      on = r.on ?? false;
    }
  }
  if (!on) return [];
  const taken = new Set<number>();
  const kept: number[] = [];
  entries.forEach((e, i) => {
    if (e.seq <= since || e.seq >= ctx.joined) return;
    if (!ctx.allowed.includes(e.senderId) || !ctx.devices.has(`${e.senderId}/${e.deviceId}`)) return;
    const r = ctx.log.get(e.seq);
    if (!r || r.kind !== "message" || r.deleted || r.senderId !== e.senderId) return;
    if (e.kind === "text") {
      if (r.deviceId !== e.deviceId || taken.has(e.seq)) return;
      taken.add(e.seq);
    } else if (e.target !== e.seq || !taken.has(e.seq)) {
      return;
    }
    kept.push(i);
  });
  return kept;
}
