/**
 * What this browser keeps for encrypted direct messages and secure channels,
 * in IndexedDB: each signed-in account's device (its private keys and every
 * group's state), how far it has read each conversation or channel (both are
 * a "conversation" here, by id), and the messages it opened. A message can only be opened once (keys are thrown away as they're
 * used, which is what keeps old messages safe), so what it said lives here
 * and nowhere else. Signing out wipes it.
 *
 * Everything for one account on one instance shares a vault key,
 * "<instance>|<account id>". With a message backup on, this device's
 * recovery key is kept here too, with the lines the backup hasn't taken yet.
 */

const DB = "fuwa-e2ee";
const VERSION = 2;

/** One account's device on one instance. */
export type StoredDevice = {
  vault: string;
  /** SHA-256 of the session token it registered with: a new sign-in is a new device. */
  session: string;
  /** `Device.save()`. */
  state: Uint8Array;
  /** Goes up with every save, so another tab knows its copy is old. */
  version: number;
};

/** Where this device stands in one conversation. */
export type Note = {
  vault: string;
  conversation: string;
  /** The last record it read (or skipped). */
  cursor: number;
  /** The last record you saw. */
  read: number;
  /** The safety number you checked with the other person, if you did. */
  verified: string;
  /** Threads in a secure channel you followed (true) or unfollowed (false) by hand, by their message's record. */
  follows?: Record<number, boolean>;
  /** The last reply you saw in each thread, by the thread's message's record. */
  threadRead?: Record<number, number>;
};

/** A device being added to or leaving a conversation, by whose it is. */
export type DeviceRef = { userId: string; deviceId: string };

/** One line of a conversation as this device saw it. */
/** A SignedContent's parts, and the signature key of the device that signed it. */
export type Signed = { payload: Uint8Array; signature: Uint8Array; key: Uint8Array };

export type Item = {
  vault: string;
  conversation: string;
  /** The record it came in: its place in the conversation. */
  seq: number;
  /** Unix ms, as the instance stamped the record. */
  at: number;
  senderId: string;
  deviceId: string;
  /**
   * text: a message. devices: devices joined or left. joined: this device
   * came in here (what came before, it can't read). unreadable: a record it
   * couldn't open. reset: someone started a secure channel's encryption over.
   * setting: someone turned a secure channel's history sharing on ("on") or
   * off ("off"), in content. voice: a voice message (`voice`), with a
   * line about it in content for previews and notifications. thread: someone
   * locked ("locked") or unlocked ("unlocked") the thread under the message
   * `thread` names.
   */
  kind: "text" | "devices" | "joined" | "unreadable" | "reset" | "setting" | "voice" | "thread";
  content: string;
  replyTo: number;
  /** Unix ms of the last edit, or 0. */
  editedAt: number;
  deleted: boolean;
  added: DeviceRef[];
  removed: DeviceRef[];
  /** A secure channel message as its sender's device signed it, to pass on to devices added later. */
  signed?: Signed;
  /** Its latest edit, signed the same way. */
  editSigned?: Signed;
  /** Who passed it on to this device, when it came as shared history rather than as it was sent. */
  sharedBy?: string;
  /** In a secure channel: the record of the message this replies under, as a thread (0 or absent: none). */
  thread?: number;
  /** A thread reply its author also sent to the channel. */
  inChannel?: boolean;
  /** A voice message: what fetching, opening and showing it takes. */
  voice?: Voice;
  /** Files a text carries (its content is then their caption, maybe empty). */
  files?: FileRef[];
};

/** A file inside an encrypted message, sealed in chunks: what fetching, opening and showing it takes. */
export type FileRef = {
  mediaId: string;
  key: Uint8Array;
  sha256: Uint8Array;
  /** The stored (sealed) bytes. */
  size: number;
  chunkBytes: number;
  /** Its name, cleaned. */
  name: string;
  /** What the sender said it is: shown as a hint, never trusted for previews. */
  type: string;
  width: number;
  height: number;
  /** Its size before padding, as the sender said (0: unknown). */
  fileSize: number;
};

/** A voice message's sealed file and what it sounds like, from inside the encrypted message. */
export type Voice = {
  mediaId: string;
  key: Uint8Array;
  sha256: Uint8Array;
  size: number;
  durationMs: number;
  waveform: Uint8Array;
};

/** What a line says, for previews and notifications: its text, or what files it carries. */
export function lineText(i: Pick<Item, "content" | "files">): string {
  if (i.content || !i.files?.length) return i.content;
  return i.files.length === 1 ? `File: ${i.files[0]!.name}` : `${i.files.length} files`;
}

/** Something someone said: text or a voice message. */
export const isMessage = (i: Pick<Item, "kind">) => i.kind === "text" || i.kind === "voice";

/** This device's hold on the account's message backup: the recovery key, which never leaves the browser. */
export type StoredBackup = { vault: string; key: Uint8Array; check: Uint8Array };

/** A line written since the backup last took it. */
type Unbacked = { vault: string; conversation: string; seq: number };

/** What this device sent, by the SHA-256 of its ciphertext: it can't open its own messages. */
type Sent = { vault: string; hash: string; plaintext: Uint8Array; at: number };

let opening: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  opening ??= new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DB, VERSION);
    request.onupgradeneeded = (e) => {
      const db = request.result;
      if (e.oldVersion < 1) {
        db.createObjectStore("devices", { keyPath: "vault" });
        db.createObjectStore("notes", { keyPath: ["vault", "conversation"] });
        db.createObjectStore("items", { keyPath: ["vault", "conversation", "seq"] });
        db.createObjectStore("sent", { keyPath: ["vault", "hash"] });
      }
      if (e.oldVersion < 2) {
        db.createObjectStore("backup", { keyPath: "vault" });
        db.createObjectStore("unbacked", { keyPath: ["vault", "conversation", "seq"] });
      }
    };
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("IndexedDB wouldn't open"));
    request.onblocked = () => reject(new Error("another tab is upgrading this browser's encrypted storage"));
  }).catch((err) => {
    opening = null;
    throw err;
  });
  return opening;
}

const done = (tx: IDBTransaction) =>
  new Promise<void>((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onerror = () => reject(tx.error ?? new Error("IndexedDB write failed"));
    tx.onabort = () => reject(tx.error ?? new Error("IndexedDB write was aborted"));
  });

const result = <T>(request: IDBRequest<T>) =>
  new Promise<T>((resolve, reject) => {
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error ?? new Error("IndexedDB read failed"));
  });

/** Every key from `[vault]` to `[vault, …]` in a store keyed by arrays starting with the vault. */
const inVault = (vault: string) => IDBKeyRange.bound([vault], [vault, []]);
const inConversation = (vault: string, conversation: string) =>
  IDBKeyRange.bound([vault, conversation], [vault, conversation, []]);

export async function loadDevice(vault: string): Promise<StoredDevice | undefined> {
  const db = await open();
  return result(db.transaction("devices").objectStore("devices").get(vault) as IDBRequest<StoredDevice | undefined>);
}

export async function loadNotes(vault: string): Promise<Note[]> {
  const db = await open();
  return result(db.transaction("notes").objectStore("notes").getAll(inVault(vault)) as IDBRequest<Note[]>);
}

export async function loadNote(vault: string, conversation: string): Promise<Note> {
  const db = await open();
  const note = await result(
    db.transaction("notes").objectStore("notes").get([vault, conversation]) as IDBRequest<Note | undefined>,
  );
  return note ?? { vault, conversation, cursor: 0, read: 0, verified: "" };
}

export async function loadItems(vault: string, conversation: string): Promise<Item[]> {
  const db = await open();
  return result(db.transaction("items").objectStore("items").getAll(inConversation(vault, conversation)) as IDBRequest<Item[]>);
}

export async function loadSent(vault: string, hash: string): Promise<Uint8Array | undefined> {
  const db = await open();
  const sent = await result(db.transaction("sent").objectStore("sent").get([vault, hash]) as IDBRequest<Sent | undefined>);
  return sent?.plaintext;
}

/** Changes to write together, so the device's state never gets ahead of what it noted. */
export type Batch = {
  device?: StoredDevice;
  notes?: Note[];
  items?: Item[];
  sent?: { hash: string; plaintext: Uint8Array }[];
  forgetSent?: string[];
};

/** Vaults whose new lines go to the message backup, and what to tell when some are written. */
const backingUp = new Map<string, () => void>();

/** Notes a vault's new lines for the message backup, calling `written` after each write that has some (or stops, with null). */
export function setBackingUp(vault: string, written: (() => void) | null) {
  if (written) backingUp.set(vault, written);
  else backingUp.delete(vault);
}

export async function write(vault: string, batch: Batch): Promise<void> {
  const db = await open();
  const tx = db.transaction(["devices", "notes", "items", "sent", "unbacked"], "readwrite");
  if (batch.device) tx.objectStore("devices").put(batch.device);
  for (const note of batch.notes ?? []) tx.objectStore("notes").put(note);
  for (const item of batch.items ?? []) {
    tx.objectStore("items").put(item);
    if (backingUp.has(vault)) {
      tx.objectStore("unbacked").put({ vault, conversation: item.conversation, seq: item.seq } satisfies Unbacked);
    }
  }
  const now = Date.now();
  for (const { hash, plaintext } of batch.sent ?? []) tx.objectStore("sent").put({ vault, hash, plaintext, at: now } satisfies Sent);
  for (const hash of batch.forgetSent ?? []) tx.objectStore("sent").delete([vault, hash]);
  await done(tx);
  if (batch.items?.length) backingUp.get(vault)?.();
}

/** Forgets what was kept for one conversation or secure channel: how far it was read, and what it said. */
export async function forget(vault: string, conversation: string): Promise<void> {
  const db = await open();
  const tx = db.transaction(["notes", "items", "unbacked"], "readwrite");
  tx.objectStore("notes").delete([vault, conversation]);
  tx.objectStore("items").delete(inConversation(vault, conversation));
  tx.objectStore("unbacked").delete(inConversation(vault, conversation));
  await done(tx);
}

// ───────────────────────── Message backup ─────────────────────────

export async function loadBackup(vault: string): Promise<StoredBackup | undefined> {
  const db = await open();
  return result(db.transaction("backup").objectStore("backup").get(vault) as IDBRequest<StoredBackup | undefined>);
}

/** Keeps this device's recovery key, and notes every line it has so far for the backup. */
export async function keepBackup(backup: StoredBackup, everything: boolean): Promise<void> {
  const db = await open();
  const tx = db.transaction(["backup", "items", "unbacked"], "readwrite");
  tx.objectStore("backup").put(backup);
  if (everything) {
    const keys = await result(tx.objectStore("items").getAllKeys(inVault(backup.vault)));
    for (const key of keys as [string, string, number][]) {
      tx.objectStore("unbacked").put({ vault: backup.vault, conversation: key[1], seq: key[2] } satisfies Unbacked);
    }
  }
  await done(tx);
}

/** Forgets this device's recovery key and what was waiting for the backup. */
export async function dropBackup(vault: string): Promise<void> {
  const db = await open();
  const tx = db.transaction(["backup", "unbacked"], "readwrite");
  tx.objectStore("backup").delete(vault);
  tx.objectStore("unbacked").delete(inVault(vault));
  await done(tx);
}

/** Up to `limit` lines waiting for the backup, with what they say now (gone ones as null). */
export async function loadUnbacked(vault: string, limit: number): Promise<{ key: [string, string, number]; item: Item | null }[]> {
  const db = await open();
  const tx = db.transaction(["unbacked", "items"]);
  const keys = (await result(tx.objectStore("unbacked").getAllKeys(inVault(vault), limit))) as [string, string, number][];
  return Promise.all(
    keys.map(async (key) => ({ key, item: ((await result(tx.objectStore("items").get(key))) as Item | undefined) ?? null })),
  );
}

/** Notes lines as taken by the backup. */
export async function markBacked(keys: [string, string, number][]): Promise<void> {
  const db = await open();
  const tx = db.transaction("unbacked", "readwrite");
  for (const key of keys) tx.objectStore("unbacked").delete(key);
  await done(tx);
}

/**
 * Keeps lines from the backup and counts them as read (another device read
 * them), changing nothing else about where this device stands, in one go.
 */
export async function writeRestored(vault: string, items: Item[]): Promise<void> {
  const db = await open();
  const tx = db.transaction(["notes", "items"], "readwrite");
  const latest = new Map<string, number>();
  for (const item of items) {
    tx.objectStore("items").put(item);
    latest.set(item.conversation, Math.max(latest.get(item.conversation) ?? 0, item.seq));
  }
  for (const [conversation, seq] of latest) {
    const request = tx.objectStore("notes").get([vault, conversation]) as IDBRequest<Note | undefined>;
    request.onsuccess = () => {
      const note = request.result;
      if (!note) tx.objectStore("notes").put({ vault, conversation, cursor: 0, read: seq, verified: "" } satisfies Note);
      else if (note.read < seq) tx.objectStore("notes").put({ ...note, read: seq });
    };
  }
  await done(tx);
}

/** Items for `keys`, as this device has them now. */
export async function loadItemsAt(keys: [string, string, number][]): Promise<(Item | undefined)[]> {
  const db = await open();
  const tx = db.transaction("items");
  return Promise.all(keys.map((key) => result(tx.objectStore("items").get(key)) as Promise<Item | undefined>));
}

/** Forgets everything kept for exactly one vault: one account on one instance, and no other account whose id starts the same. */
export async function wipeVault(vaultKey: string): Promise<void> {
  const db = await open();
  const tx = db.transaction(["devices", "notes", "items", "sent", "backup", "unbacked"], "readwrite");
  tx.objectStore("devices").delete(vaultKey);
  tx.objectStore("backup").delete(vaultKey);
  for (const store of ["notes", "items", "sent", "unbacked"] as const) {
    tx.objectStore(store).delete(IDBKeyRange.bound([vaultKey], [vaultKey, []]));
  }
  await done(tx);
}

/** Forgets everything kept for vaults whose key starts with `prefix`: every account on an instance. */
export async function wipe(prefix: string): Promise<void> {
  const db = await open();
  const tx = db.transaction(["devices", "notes", "items", "sent", "backup", "unbacked"], "readwrite");
  const range = IDBKeyRange.bound(prefix, `${prefix}￿`);
  tx.objectStore("devices").delete(range);
  tx.objectStore("backup").delete(range);
  for (const store of ["notes", "items", "sent", "unbacked"] as const) {
    tx.objectStore(store).delete(IDBKeyRange.bound([prefix], [`${prefix}￿`, []]));
  }
  await done(tx);
}
