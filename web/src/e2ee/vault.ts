/**
 * What this browser keeps for encrypted direct messages and secure channels,
 * in IndexedDB: each signed-in account's device (its private keys and every
 * group's state), how far it has read each conversation or channel (both are
 * a "conversation" here, by id), and the messages it opened. A message can only be opened once (keys are thrown away as they're
 * used, which is what keeps old messages safe), so what it said lives here
 * and nowhere else. Signing out wipes it.
 *
 * Everything for one account on one instance shares a vault key,
 * "<instance>|<account id>".
 */

const DB = "fuwa-e2ee";
const VERSION = 1;

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
   * off ("off"), in content.
   */
  kind: "text" | "devices" | "joined" | "unreadable" | "reset" | "setting";
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
  /** Shared history whose signing device isn't one of its sender's any more, so it can't be checked against them. */
  unchecked?: boolean;
};

/** What this device sent, by the SHA-256 of its ciphertext: it can't open its own messages. */
type Sent = { vault: string; hash: string; plaintext: Uint8Array; at: number };

let opening: Promise<IDBDatabase> | null = null;

function open(): Promise<IDBDatabase> {
  opening ??= new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open(DB, VERSION);
    request.onupgradeneeded = () => {
      const db = request.result;
      db.createObjectStore("devices", { keyPath: "vault" });
      db.createObjectStore("notes", { keyPath: ["vault", "conversation"] });
      db.createObjectStore("items", { keyPath: ["vault", "conversation", "seq"] });
      db.createObjectStore("sent", { keyPath: ["vault", "hash"] });
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

export async function write(vault: string, batch: Batch): Promise<void> {
  const db = await open();
  const tx = db.transaction(["devices", "notes", "items", "sent"], "readwrite");
  if (batch.device) tx.objectStore("devices").put(batch.device);
  for (const note of batch.notes ?? []) tx.objectStore("notes").put(note);
  for (const item of batch.items ?? []) tx.objectStore("items").put(item);
  const now = Date.now();
  for (const { hash, plaintext } of batch.sent ?? []) tx.objectStore("sent").put({ vault, hash, plaintext, at: now } satisfies Sent);
  for (const hash of batch.forgetSent ?? []) tx.objectStore("sent").delete([vault, hash]);
  await done(tx);
}

/** Forgets what was kept for one conversation or secure channel: how far it was read, and what it said. */
export async function forget(vault: string, conversation: string): Promise<void> {
  const db = await open();
  const tx = db.transaction(["notes", "items"], "readwrite");
  tx.objectStore("notes").delete([vault, conversation]);
  tx.objectStore("items").delete(inConversation(vault, conversation));
  await done(tx);
}

/** Forgets everything kept for vaults whose key starts with `prefix`: one account, or every account on an instance. */
export async function wipe(prefix: string): Promise<void> {
  const db = await open();
  const tx = db.transaction(["devices", "notes", "items", "sent"], "readwrite");
  const range = IDBKeyRange.bound(prefix, `${prefix}￿`);
  tx.objectStore("devices").delete(range);
  for (const store of ["notes", "items", "sent"] as const) {
    tx.objectStore(store).delete(IDBKeyRange.bound([prefix], [`${prefix}￿`, []]));
  }
  await done(tx);
}
