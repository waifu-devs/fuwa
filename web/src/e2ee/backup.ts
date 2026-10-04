import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { timestampMs } from "@bufbuild/protobuf/wkt";
import { Code } from "@connectrpc/connect";
import type { Api } from "@/fuwa/client";
import { toFuwaError } from "@/fuwa/errors";
import { updateDms, type BackupState } from "@/fuwa/store";
import { reportError, reportTiming } from "@/lib/reports";
import {
  BackupDeviceSchema,
  BackupItemKind,
  BackupItemSchema,
  BackupPartSchema,
  SignedFormSchema,
  type Backup,
  type BackupItem,
  type SignedForm,
} from "@/gen/fuwa/v1/dm_pb";
import { deriveKeys, formatRecoveryKey, newer, newRecoveryKey, open, paddingFor, parseRecoveryKey, sameBytes, seal, type BackupKeys } from "./backupkey";
import * as vault from "./vault";

/**
 * The account's message backup, as this device takes part in it: every line
 * it reads in direct messages and secure channels goes, a few seconds later,
 * into an encrypted part on the instance; a new device with the recovery key
 * reads them all back. See docs/e2ee.md.
 */

const CALL = { timeoutMs: 30_000 };
/** How long after a new line the backup takes it, so a burst goes in one part. */
const SETTLE_MS = 4_000;
/** How long to wait after a failure before trying again. */
const RETRY_MS = 30_000;
/** The most plaintext in one part, leaving room for padding under the instance's 256 KiB. */
const PART_BYTES = 200 * 1024;
/** Lines read at once while making parts. */
const BATCH = 400;

const KINDS: Partial<Record<vault.Item["kind"], BackupItemKind>> = {
  text: BackupItemKind.TEXT,
  devices: BackupItemKind.DEVICES,
  reset: BackupItemKind.RESET,
  setting: BackupItemKind.SETTING,
  thread: BackupItemKind.THREAD,
};
const FROM_KIND: Record<number, vault.Item["kind"]> = {
  [BackupItemKind.TEXT]: "text",
  [BackupItemKind.DEVICES]: "devices",
  [BackupItemKind.RESET]: "reset",
  [BackupItemKind.SETTING]: "setting",
  [BackupItemKind.THREAD]: "thread",
};

const signedForm = (s: vault.Signed | undefined) =>
  s ? create(SignedFormSchema, { payload: s.payload, signature: s.signature, signatureKey: s.key }) : undefined;
const fromSigned = (s: SignedForm | undefined): vault.Signed | undefined =>
  s && s.payload.length ? { payload: s.payload, signature: s.signature, key: s.signatureKey } : undefined;

function toBackup(i: vault.Item): BackupItem | null {
  const kind = KINDS[i.kind];
  if (kind === undefined) return null;
  return create(BackupItemSchema, {
    conversationId: i.conversation,
    sequence: BigInt(i.seq),
    atMs: BigInt(i.at),
    senderId: i.senderId,
    deviceId: i.deviceId,
    kind,
    content: i.content,
    replyToSequence: BigInt(i.replyTo),
    editedAtMs: BigInt(i.editedAt),
    deleted: i.deleted,
    added: i.added.map((d) => create(BackupDeviceSchema, d)),
    removed: i.removed.map((d) => create(BackupDeviceSchema, d)),
    signed: signedForm(i.signed),
    editSigned: signedForm(i.editSigned),
    sharedBy: i.sharedBy ?? "",
    threadSequence: BigInt(i.thread ?? 0),
    inChannel: !!i.inChannel,
    locked: i.kind === "thread" && i.content === "locked",
  });
}

function fromBackup(vaultKey: string, b: BackupItem): vault.Item | null {
  const kind = FROM_KIND[b.kind];
  const seq = Number(b.sequence);
  const thread = Number(b.threadSequence);
  if (!kind || !b.conversationId || !(seq > 0)) return null;
  if (kind === "thread" && !(thread > 0)) return null;
  return {
    vault: vaultKey,
    conversation: b.conversationId,
    seq,
    at: Number(b.atMs),
    senderId: b.senderId,
    deviceId: b.deviceId,
    kind,
    content: b.deleted ? "" : b.content,
    replyTo: Number(b.replyToSequence),
    editedAt: Number(b.editedAtMs),
    deleted: b.deleted,
    added: b.added.map((d) => ({ userId: d.userId, deviceId: d.deviceId })),
    removed: b.removed.map((d) => ({ userId: d.userId, deviceId: d.deviceId })),
    signed: fromSigned(b.signed),
    editSigned: fromSigned(b.editSigned),
    sharedBy: b.sharedBy || undefined,
    ...(thread > 0 ? { thread, inChannel: b.inChannel } : {}),
    ...(kind === "thread" ? { content: b.locked ? "locked" : "unlocked" } : {}),
  };
}

/** A part's plaintext, padded to an exact multiple of PAD_TO. */
function encodePart(items: BackupItem[]): Uint8Array {
  const bare = toBinary(BackupPartSchema, create(BackupPartSchema, { items }));
  return toBinary(BackupPartSchema, create(BackupPartSchema, { items, padding: new Uint8Array(paddingFor(bare.length)) }));
}

export class BackupError extends Error {}

export class BackupSync {
  private keys: BackupKeys | null = null;
  /** The place the next part takes, as last heard. */
  private next = 1n;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private flushing: Promise<void> | null = null;
  private stopped = false;

  constructor(
    private readonly key: string,
    private readonly api: Api,
    private readonly vaultKey: string,
    private readonly accountId: string,
    /** Runs `fn` while no other tab works on this vault's backup. */
    private readonly exclusive: <T>(fn: () => Promise<T>) => Promise<T>,
    /** Shows restored lines. */
    private readonly refresh: (conversations: string[]) => Promise<void>,
  ) {}

  private show(patch: Partial<BackupState>) {
    updateDms(this.key, (d) => ({ ...d, backup: { ...d.backup, ...patch } }));
  }

  private showBackup(b: Backup | undefined, status: BackupState["status"], problem: string | null = null) {
    if (b) this.next = b.nextSequence;
    this.show({
      status,
      problem,
      size: Number(b?.size ?? 0n),
      maxSize: Number(b?.maxSize ?? 0n),
      updatedAt: b?.updatedAt ? timestampMs(b.updatedAt) : 0,
    });
  }

  /** Works out where this device stands: in the backup, or not. */
  async start() {
    let backup: Backup | undefined;
    try {
      backup = (await this.api.dms.getBackup({}, CALL)).backup;
    } catch (err) {
      // An instance from before backups has none to offer.
      if (toFuwaError(err).code === Code.Unimplemented) return this.show({ status: "unsupported" });
      throw err;
    }
    const stored = await vault.loadBackup(this.vaultKey);
    if (!backup) {
      if (stored) await vault.dropBackup(this.vaultKey);
      return this.showBackup(undefined, "off");
    }
    if (!stored || !sameBytes(stored.check, backup.keyCheck)) {
      if (stored) await vault.dropBackup(this.vaultKey);
      return this.showBackup(backup, "locked");
    }
    this.keys = await deriveKeys(stored.key);
    this.enable();
    this.showBackup(backup, "on");
    this.soon(0);
  }

  stop() {
    this.stopped = true;
    if (this.timer) clearTimeout(this.timer);
    vault.setBackingUp(this.vaultKey, null);
  }

  private enable() {
    vault.setBackingUp(this.vaultKey, () => this.noteChanges());
  }

  private disable() {
    this.keys = null;
    vault.setBackingUp(this.vaultKey, null);
    if (this.timer) clearTimeout(this.timer);
    this.timer = null;
  }

  /** New lines were written: take them shortly. */
  noteChanges() {
    if (this.keys) this.soon(SETTLE_MS);
  }

  private soon(ms: number) {
    if (this.stopped || this.timer) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.flush();
    }, ms);
  }

  /** Puts every line waiting for the backup into parts. */
  flush(): Promise<void> {
    this.flushing ??= this.exclusive(() => this.flushNow()).finally(() => {
      this.flushing = null;
    });
    return this.flushing;
  }

  private async flushNow() {
    while (this.keys && !this.stopped) {
      const keys = this.keys;
      const waiting = await vault.loadUnbacked(this.vaultKey, BATCH);
      if (!waiting.length) return;
      const items: BackupItem[] = [];
      const taken: [string, string, number][] = [];
      let size = 0;
      for (const { key, item } of waiting) {
        const b = item && toBackup(item);
        const bytes = b ? toBinary(BackupItemSchema, b).length + 4 : 0;
        if (items.length && size + bytes > PART_BYTES) break;
        taken.push(key);
        if (b) {
          items.push(b);
          size += bytes;
        }
      }
      try {
        if (items.length) {
          const plaintext = encodePart(items);
          for (let attempt = 0; ; attempt++) {
            const sequence = this.next;
            const data = await seal(keys, this.accountId, sequence, plaintext);
            try {
              const { backup } = await this.api.dms.addBackupPart({ keyCheck: keys.check, data, sequence }, CALL);
              this.showBackup(backup, "on");
              break;
            } catch (err) {
              // Another device took that place: seal it again for the next one.
              if (attempt >= 3 || toFuwaError(err).code !== Code.AlreadyExists) throw err;
              const { backup } = await this.api.dms.getBackup({}, CALL);
              if (!backup) throw err;
              this.next = backup.nextSequence;
            }
          }
        }
        await vault.markBacked(taken);
      } catch (err) {
        const e = toFuwaError(err);
        if (e.code === Code.FailedPrecondition) {
          // Started over (or turned off) on another device: this one's key is no good now.
          this.disable();
          await vault.dropBackup(this.vaultKey);
          await this.start().catch(() => {});
          return;
        }
        if (e.code === Code.ResourceExhausted) {
          reportError("e2ee.backup_full", "backup");
          this.show({ status: "full", problem: e.message });
          return;
        }
        reportError("e2ee.backup_failed", "backup");
        this.show({ problem: e.message });
        this.soon(RETRY_MS);
        return;
      }
    }
  }

  /**
   * Starts a backup with a new recovery key, holding everything this device
   * has kept so far. `replace` deletes one the account already has. The key,
   * as the person should write it down.
   */
  async create(replace: boolean): Promise<string> {
    const key = newRecoveryKey();
    const keys = await deriveKeys(key);
    const { backup } = await this.api.dms.startBackup({ keyCheck: keys.check, replace }, CALL);
    await vault.keepBackup({ vault: this.vaultKey, key, check: keys.check }, true);
    this.keys = keys;
    this.enable();
    this.showBackup(backup, "on");
    void this.flush();
    return formatRecoveryKey(key);
  }

  /**
   * Reads the backup back with its recovery key, keeping every line this
   * device doesn't have a newer copy of, and from then on adds to it. What
   * this device had already goes into the backup too.
   */
  async restore(text: string) {
    const key = await parseRecoveryKey(text);
    if (!key) throw new BackupError("That isn't a recovery key. Check for a typo: it's 56 letters and digits.");
    const keys = await deriveKeys(key);
    const { backup } = await this.api.dms.getBackup({}, CALL);
    if (!backup) throw new BackupError("Your account has no message backup any more.");
    if (!sameBytes(backup.keyCheck, keys.check)) throw new BackupError("That's not this backup's recovery key.");
    this.show({ status: "restoring", restored: 0, total: Number(backup.parts), problem: null });
    const began = performance.now();
    const touched = new Set<string>();
    let after = 0n;
    let done = 0;
    let unreadable = 0;
    let missing = 0;
    try {
      await vault.keepBackup({ vault: this.vaultKey, key, check: keys.check }, true);
      for (;;) {
        const { parts, hasMore } = await this.api.dms.listBackupParts({ afterSequence: after, limit: 50 }, CALL);
        for (const part of parts) {
          // Parts are numbered one after another: a gap is a part the instance didn't give back.
          if (part.sequence > after + 1n) missing += Number(part.sequence - after - 1n);
          after = part.sequence;
          done++;
          const plaintext = await open(keys, this.accountId, part.sequence, part.data);
          if (!plaintext) {
            unreadable++;
            continue;
          }
          let items: vault.Item[];
          try {
            items = fromBinary(BackupPartSchema, plaintext)
              .items.map((b) => fromBackup(this.vaultKey, b))
              .filter((i): i is vault.Item => i !== null);
          } catch {
            unreadable++;
            continue;
          }
          await this.merge(items, touched);
        }
        this.show({ restored: done });
        if (!hasMore || !parts.length) break;
      }
      // And any after the last one it gave, by its own count.
      if (backup.nextSequence > after + 1n) missing += Number(backup.nextSequence - after - 1n);
    } catch (err) {
      // Left as it was: the key is asked for again.
      await vault.dropBackup(this.vaultKey).catch(() => {});
      this.showBackup(backup, "locked");
      throw err;
    }
    reportTiming("e2ee.backup_restore", performance.now() - began);
    if (unreadable) reportError("e2ee.backup_unreadable", "backup");
    if (missing) reportError("e2ee.backup_missing", "backup");
    this.keys = keys;
    this.enable();
    const lost = unreadable + missing;
    this.showBackup(
      backup,
      "on",
      lost
        ? `${lost} part${lost === 1 ? "" : "s"} of the backup ${missing ? "didn't come back from the instance or " : ""}couldn't be read, so some messages, edits or deletions may be missing here.`
        : null,
    );
    await this.refresh([...touched]);
    void this.flush();
  }

  /** Keeps the lines this device has no newer copy of; what came back was read already, so it counts as read. */
  private async merge(items: vault.Item[], touched: Set<string>) {
    const have = await vault.loadItemsAt(items.map((i) => [i.vault, i.conversation, i.seq]));
    const take = items.filter((i, n) => newer(have[n], i));
    if (!take.length) return;
    for (const i of take) touched.add(i.conversation);
    await vault.writeRestored(this.vaultKey, take);
  }

  /** Deletes the account's backup, for every device. */
  async remove() {
    await this.api.dms.deleteBackup({}, CALL);
    this.disable();
    await vault.dropBackup(this.vaultKey);
    this.showBackup(undefined, "off");
  }
}
