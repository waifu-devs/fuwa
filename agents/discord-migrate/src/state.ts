// What a migration has done so far, per Discord server and fuwa server, so a
// run that stopped (cancelled, a restart, an outage) picks up where it left
// off instead of copying things twice. One JSON file per pair under DATA_DIR.
import { mkdir, readFile, rename, writeFile } from "node:fs/promises";
import { join } from "node:path";

export interface ChannelHistory {
  /** The last Discord message copied (or passed over). */
  lastId?: string;
  /** The day of the last divider posted, so a resumed run doesn't post it again. */
  lastDay?: string;
  messages: number;
  done?: boolean;
  /** Its threads (or a forum's posts) by Discord id, and the fuwa message each one is under. */
  threads?: Record<string, string>;
}

export interface JobState {
  guildId: string;
  serverId: string;
  server?: boolean;
  /** Discord id to fuwa id. */
  roles: Record<string, string>;
  rolesDone?: boolean;
  channels: Record<string, string>;
  channelsDone?: boolean;
  emojis: Record<string, { id: string; name: string }>;
  stickers: Record<string, { id: string; name: string }>;
  emojisDone?: boolean;
  /** Discord sound id to fuwa sound name. */
  sounds: Record<string, string>;
  soundsDone?: boolean;
  /** By Discord channel id, threads' too. */
  history: Record<string, ChannelHistory>;
  invite?: string;
}

export function emptyState(guildId: string, serverId: string): JobState {
  return { guildId, serverId, roles: {}, channels: {}, emojis: {}, stickers: {}, sounds: {}, history: {} };
}

export interface StateStore {
  load(guildId: string, serverId: string): Promise<JobState>;
  save(state: JobState): Promise<void>;
}

/** Keeps each migration's state in a file of its own. */
export class FileStore implements StateStore {
  readonly #dir: string;
  // Saves of one file run one after another, so an older one never lands last.
  readonly #queue = new Map<string, Promise<void>>();

  constructor(dir: string) {
    this.#dir = dir;
  }

  #path(guildId: string, serverId: string): string {
    return join(this.#dir, `${guildId.replace(/\W/g, "")}-${serverId.replace(/\W/g, "")}.json`);
  }

  async load(guildId: string, serverId: string): Promise<JobState> {
    try {
      const saved = JSON.parse(await readFile(this.#path(guildId, serverId), "utf8")) as JobState;
      return { ...emptyState(guildId, serverId), ...saved };
    } catch (err) {
      if ((err as NodeJS.ErrnoException).code === "ENOENT") return emptyState(guildId, serverId);
      throw err;
    }
  }

  save(state: JobState): Promise<void> {
    const path = this.#path(state.guildId, state.serverId);
    const text = JSON.stringify(state);
    const next = (this.#queue.get(path) ?? Promise.resolve()).then(async () => {
      await mkdir(this.#dir, { recursive: true });
      await writeFile(`${path}.tmp`, text);
      await rename(`${path}.tmp`, path);
    });
    this.#queue.set(
      path,
      next.catch(() => {}),
    );
    return next;
  }
}

/** Keeps state in memory only: for tests, or when nothing should be written. */
export class MemoryStore implements StateStore {
  readonly saved = new Map<string, string>();

  async load(guildId: string, serverId: string): Promise<JobState> {
    const text = this.saved.get(`${guildId}-${serverId}`);
    return text ? (JSON.parse(text) as JobState) : emptyState(guildId, serverId);
  }

  async save(state: JobState): Promise<void> {
    this.saved.set(`${state.guildId}-${state.serverId}`, JSON.stringify(state));
  }
}
