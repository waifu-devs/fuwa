import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { timestampMs } from "@bufbuild/protobuf/wkt";
import { Code } from "@connectrpc/connect";
import type { Api } from "@/fuwa/client";
import { toFuwaError } from "@/fuwa/errors";
import { byId, memberById } from "@/fuwa/lists";
import { store, updateDms, updateInstance, without, type DmMember, type FuwaState } from "@/fuwa/store";
import { onDirectMessage, onSecureMessage } from "@/lib/notify";
import { onDmPinEvent } from "@/fuwa/pins";
import { onDmTypingEvent } from "@/fuwa/typing";
import {
  ConversationRecordKind,
  DirectMessageContentSchema,
  DirectMessageEditSchema,
  DirectMessageReactionSchema,
  DirectMessageTextSchema,
  SharedEntrySchema,
  SharedHistorySchema,
  SignedContentSchema,
  SignedPayloadSchema,
  ThreadChangeSchema,
  WatchResponseSchema,
  type Conversation,
  type ConversationRecord,
  type DirectMessageContent,
  type DirectMessageEvent,
  type DirectMessageReaction,
  type SharedHistory,
  type WatchResponse,
} from "@/gen/fuwa/v1/dm_pb";
import type { DmCall } from "@/gen/fuwa/v1/call_pb";
import type { Device as DeviceInfo } from "@/gen/fuwa/v1/dm_pb";
import { SealedKind } from "@/gen/fuwa/v1/dm_pb";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import { SecureRecordKind } from "@/gen/fuwa/v1/secure_pb";
import { Permission, type Event, type User } from "@/gen/fuwa/v1/types_pb";
import type { Timestamp } from "@bufbuild/protobuf/wkt";
import { accessOf, hasIn } from "@/lib/permissions";
import { i18n, type Key } from "@/i18n/i18n";
import type { CarriedEmoji } from "@/lib/emoji-search";
import { reportError } from "@/lib/reports";
import { isOneEmoji, tallyDm } from "@/lib/reactions";
import { BackupSync } from "./backup";
import * as history from "./history";
import * as win from "./lines";
import * as threads from "./threads";
import * as vault from "./vault";
import { customField, emojisField, instanceOrigins, toEmojis } from "./emojis";
import { filesOf, toSealedFiles } from "./files";
import { toVoiceMessage, voiceLength, voiceOf } from "./voice";
import { loadE2ee, type Commit, type Device, type E2ee, type Processed, type WasmMember } from "./wasm";

/**
 * Encrypted direct messages for one signed-in account on one instance: this
 * browser's device, kept in the vault, following every conversation.
 *
 * Each conversation is an MLS group of every device of both people. The
 * instance keeps its records in one order and takes one only for the
 * group's current epoch, so every device reads the same history: commits
 * (devices joining or leaving) and messages. This device reads them in order
 * and keeps what they said. Before sending, it makes sure the group holds
 * exactly the devices both people are signed in on, adding new ones with the
 * key packages they published and dropping signed-out ones. A device that
 * wasn't added (a new sign-in) joins by itself from the group's public state.
 *
 * Tabs share the vault. Anything that changes the device runs under a Web
 * Lock, after reloading the device if another tab saved it since, and tells
 * the other tabs what changed.
 */

/** Key packages a new device publishes, and when to publish more. */
const KEY_PACKAGES = 30;
const LOW_ON_KEY_PACKAGES = 10;
/** Records read at a time while catching up. */
const PAGE = 200;
/** The server sends a heartbeat every 25 seconds; this long without anything means the stream is gone. */
const SILENCE_MS = 70_000;
const CALL = { timeoutMs: 20_000 };
/** Reserving an upload can wait on the instance's daily counts. */
const UPLOAD = { timeoutMs: 20_000 };
/** The longest message, in characters, as the composer allows. */
export const MAX_DM = 4000;

/** What a conversation's group exports its call secret under. */
const CALL_LABEL = "fuwa call v1";

/** The most people or devices one call asks the instance about. */
const LOOKUPS = 100;
/** The most shared history one device passes on at once: the newest messages that fit in one encrypted message. */
const HISTORY_BYTES = 56_000;
const HISTORY_ENTRIES = 500;
/** Pages of the log a device reads to check shared history against: enough for what one share carries. */
const LOG_PAGES = 10;
/** How long a device waits, at most, before taking people who lost access out of a secure channel. */
const ACCESS_SETTLE_MS = 2500;
/** How much longer a device that joined a secure channel after it started waits, so one with more of its history goes first. */
const LATE_SETTLE_MS = 3500;

/**
 * Why a secure channel can't be read or written: its group can't be followed
 * any more (a change to it no device could read), until someone with Manage
 * Channels starts its encryption over. It's the catalog key, which also marks
 * the channel in `dms.blocked`; whatever shows it translates it.
 */
export const SECURE_BROKEN: Key = "system.e2ee.secureBroken";

/** One entry in a group's log: a direct message's ConversationRecord, or a secure channel's SecureRecord. */
type Rec = {
  sequence: bigint;
  kind: number;
  senderId: string;
  senderDeviceId: string;
  data: Uint8Array;
  createdAt?: Timestamp;
};

/**
 * One MLS group this device takes part in, and how to reach it: a direct
 * message conversation (DirectMessageService) or a secure channel in a
 * community server (SecureChannelService). Both keep their records the same
 * way, so everything else here is the same for both.
 */
type Room = {
  id: string;
  channel: { serverId: string } | null;
  /** Who may show up in the group's history. */
  allowed: string[];
  /** Who belongs in the group now, asked fresh. */
  belong(): Promise<string[]>;
  /** Refuses to write when the group can't be made right yet. */
  check(devices: DeviceInfo[], belong: string[]): void;
  records(after: number): Promise<{ records: Rec[]; hasMore: boolean }>;
  welcome(): Promise<{ sequence: bigint; data: Uint8Array } | undefined>;
  groupInfo(): Promise<{ epoch: bigint; groupInfo: Uint8Array }>;
  commit(commit: Commit, welcome: boolean): Promise<Rec | undefined>;
  /** Sends an encrypted message, with the sealed files it carries. */
  message(ciphertext: Uint8Array, mediaIds?: string[]): Promise<void>;
  /** Reserves an upload for a sealed file of `size` stored bytes, to send here. */
  upload(size: number): Promise<{ mediaId: string; uploadUrl: string }>;
  /** Whether earlier messages are passed on to devices added later (secure channels only), asked fresh. */
  shares(): Promise<boolean>;
  /** Passes earlier messages on, right after this device's commit that added devices. */
  history(ciphertext: Uint8Array): Promise<void>;
  remove(seq: number): Promise<void>;
  /** A message someone else sent, just opened; `answersMe` when it replies to one of this account's. */
  notify(item: vault.Item, answersMe: boolean): void;
};

/** A secure channel this device follows. */
type SecureChannel = { serverId: string; memberIds: string[]; shareHistory: boolean };

const unique = (ids: Iterable<string>) => [...new Set([...ids].filter(Boolean))];
const chunks = <T,>(list: T[], size: number) =>
  Array.from({ length: Math.ceil(list.length / size) }, (_, n) => list.slice(n * size, n * size + size));

/** Something a person can be told about why sending didn't work. */
export class DmError extends Error {
  override name = "DmError";
}

const ms = (record: Rec) => (record.createdAt ? timestampMs(record.createdAt) : Date.now());
const isPrecondition = (err: unknown) => toFuwaError(err).code === Code.FailedPrecondition;
const lockName = (vaultKey: string) => `fuwa-e2ee:${vaultKey}`;

/** One tab at a time, across tabs where the browser can (Web Locks need https), or within this one. */
const tabQueues = new Map<string, Promise<unknown>>();
function exclusive<T>(name: string, fn: () => Promise<T>): Promise<T> {
  if (typeof navigator !== "undefined" && navigator.locks) return navigator.locks.request(name, fn) as Promise<T>;
  const run = (tabQueues.get(name) ?? Promise.resolve()).then(fn, fn);
  tabQueues.set(
    name,
    run.catch(() => {}),
  );
  return run;
}

function item(vaultKey: string, conversation: string, fields: Partial<vault.Item> & Pick<vault.Item, "seq" | "kind">): vault.Item {
  return {
    vault: vaultKey,
    conversation,
    at: Date.now(),
    senderId: "",
    deviceId: "",
    content: "",
    replyTo: 0,
    editedAt: 0,
    deleted: false,
    added: [],
    removed: [],
    ...fields,
  };
}

const ref = (m: WasmMember): vault.DeviceRef => ({ userId: m.userId, deviceId: m.deviceId });

/** The plaintext of a message: what only the conversation's devices see. */
export type Content =
  | { text: string; replyTo?: number; thread?: number; inChannel?: boolean; files?: vault.FileRef[]; emojis?: CarriedEmoji[] }
  | { edit: number; text: string; emojis?: CarriedEmoji[] }
  | { lock: number; locked: boolean }
  | { voice: vault.Voice; replyTo?: number }
  | { react: number; emoji: string; custom?: CarriedEmoji; removed: boolean };

function contentOf(content: Content): DirectMessageContent {
  const body: DirectMessageContent["body"] =
    "voice" in content
      ? { case: "voice", value: toVoiceMessage(content.voice, content.replyTo) }
      : "react" in content
        ? {
            case: "reaction",
            value: create(DirectMessageReactionSchema, {
              sequence: BigInt(content.react),
              emoji: content.custom ? "" : content.emoji,
              removed: content.removed,
              custom: content.custom ? toEmojis([content.custom])[0] : undefined,
            }),
          }
        : "edit" in content
        ? { case: "edit", value: create(DirectMessageEditSchema, { sequence: BigInt(content.edit), content: content.text, emojis: toEmojis(content.emojis) }) }
        : "lock" in content
          ? { case: "thread", value: create(ThreadChangeSchema, { parentSequence: BigInt(content.lock), locked: content.locked }) }
          : {
              case: "text",
              value: create(DirectMessageTextSchema, {
                content: content.text,
                replyToSequence: BigInt(content.replyTo ?? 0),
                threadSequence: BigInt(content.thread ?? 0),
                inChannel: !!content.inChannel,
                files: toSealedFiles(content.files),
                emojis: toEmojis(content.emojis),
              }),
            };
  return create(DirectMessageContentSchema, { body });
}

/** The files a text carries, checked; nothing for a text without any. */
function filesField(list: Parameters<typeof filesOf>[0]): Pick<vault.Item, "files"> {
  const files = filesOf(list);
  return files.length ? { files } : {};
}

/** A thread reply's place, from what its sender wrote; nothing for a line that isn't one. */
function threadFields(text: { threadSequence: bigint; inChannel: boolean }): Pick<vault.Item, "thread" | "inChannel"> {
  const thread = Number(text.threadSequence);
  return thread > 0 ? { thread, inChannel: text.inChannel } : {};
}

/** A reaction as kept: one standard emoji, or a server emoji that can be drawn here; null for anything else. */
function reactionOf(r: DirectMessageReaction, origins: ReadonlySet<string>): vault.Item["reaction"] | null {
  const target = Number(r.sequence);
  const custom = customField(r.custom, origins);
  if (custom) return { target, emoji: "", removed: r.removed, custom };
  return isOneEmoji(r.emoji) ? { target, emoji: r.emoji, removed: r.removed } : null;
}

/**
 * Who has Manage Messages in a secure channel, by this device's view of the
 * server's roles and the channel's overwrites: whose lock on a thread counts.
 */
export function secureModerates(key: string, serverId: string, channelId: string): (userId: string) => boolean {
  const i = store.get().instances[key];
  const server = i?.servers.find((x) => x.id === serverId);
  if (!i || !server) return () => false;
  const members = new Map((i.members[serverId] ?? []).map((m) => [m.user?.id ?? "", m]));
  const seen = new Map<string, boolean>();
  return (userId) => {
    let yes = seen.get(userId);
    if (yes === undefined) {
      const member = members.get(userId);
      const access = accessOf(serverId, server.ownerId, i.roles[serverId] ?? [], i.channels[serverId] ?? [], userId, member?.roleIds ?? []);
      seen.set(userId, (yes = !!member && hasIn(access, channelId, Permission.MANAGE_MESSAGES)));
    }
    return yes;
  };
}

export { voiceLength };

const encode = (content: Content): Uint8Array => toBinary(DirectMessageContentSchema, contentOf(content));

/** What a signed message says, once its signature checks out against the device that signed it. */
type Opened = { payload: ReturnType<typeof readPayload>; signed: vault.Signed };

function readPayload(bytes: Uint8Array) {
  return fromBinary(SignedPayloadSchema, bytes);
}

export class DmEngine {
  private readonly lock: string;
  private readonly controller = new AbortController();
  private readonly tabs: BroadcastChannel | null;
  private conversations = new Map<string, Conversation>();
  /** Secure channels in servers, by channel id: followed once opened, or once a record arrives. */
  private secure = new Map<string, SecureChannel>();
  /** Servers whose secure channels wait to be brought in step with a permission change. */
  private settling = new Map<string, ReturnType<typeof setTimeout> | null>();
  /** Catch-ups waiting their turn, so a burst of records reads each conversation once. */
  private queued = new Set<string>();
  private work: Promise<void> = Promise.resolve();
  /** The live connection's responses, taken one after another (`deliver`). */
  private delivered: Promise<void> = Promise.resolve();
  /** The account's message backup, as this device takes part in it. */
  readonly backup: BackupSync;
  /** Conversations opened lately, the one on screen first: these keep a window of lines in the store (lines.ts). */
  private recent: string[] = [];
  /** Their windows, as last shown. */
  private windows = new Map<string, win.Window>();
  /** Lines written since a window was last shown, by conversation; "all" when it has to be read again. */
  private changes = new Map<string, Map<number, vault.Item> | "all">();
  /** Each conversation's refreshes and window changes, one after another. */
  private chains = new Map<string, Promise<void>>();
  /** A refresh waiting its turn, per conversation: more asks join it. */
  private waiting = new Map<string, Promise<void>>();
  /** Whether this device joined a secure channel after it started, once worked out. */
  private late = new Map<string, boolean>();
  private lastFocus: FuwaState["focus"] = null;
  private readonly unwatch: () => void;

  private constructor(
    readonly key: string,
    private readonly api: Api,
    readonly me: User,
    private readonly e2ee: E2ee,
    private readonly vaultKey: string,
    private readonly session: string,
    private device: Device,
    private version: number,
  ) {
    this.lock = lockName(vaultKey);
    this.backup = new BackupSync(
      key,
      api,
      vaultKey,
      me.id,
      (fn) => exclusive(`${this.lock}:backup`, fn),
      async (ids) => {
        for (const id of ids) this.changedAll(id);
        await Promise.all(ids.map((id) => this.refresh(id).catch(() => {})));
        for (const id of ids) this.tell(id);
      },
    );
    this.tabs = typeof BroadcastChannel === "undefined" ? null : new BroadcastChannel("fuwa-e2ee");
    if (this.tabs) {
      this.tabs.onmessage = (e: MessageEvent<{ vault: string; conversation?: string; list?: boolean }>) => {
        if (e.data?.vault !== this.vaultKey || this.stopped) return;
        if (e.data.list) void this.resync().catch(() => {});
        else if (e.data.conversation) {
          this.changedAll(e.data.conversation);
          void this.refresh(e.data.conversation).catch(() => {});
        }
      };
    }
    // Which conversation is on screen decides which keep their lines in the store.
    const unsubscribe = store.subscribe(() => this.onStore());
    this.unwatch = () => void unsubscribe();
  }

  get deviceId() {
    return this.device.deviceId;
  }

  get stopped() {
    return this.controller.signal.aborted;
  }

  /**
   * Loads (or makes) this account's device and registers it with the
   * instance. A new sign-in is a new session, so it gets a new device, and
   * whatever the old one kept here goes.
   */
  static async start(key: string, api: Api, me: User, token: string): Promise<DmEngine> {
    const e2ee = await loadE2ee();
    const vaultKey = `${key}|${me.id}`;
    const session = e2ee.sha256(new TextEncoder().encode(token));
    return exclusive(lockName(vaultKey), async () => {
      let stored = await vault.loadDevice(vaultKey);
      if (stored && stored.session !== session) {
        await vault.wipeVault(vaultKey);
        stored = undefined;
      }
      const device = stored ? e2ee.Device.restore(stored.state) : new e2ee.Device(me.id);
      const engine = new DmEngine(key, api, me, e2ee, vaultKey, session, device, stored?.version ?? 0);
      const packages = stored ? [] : (device.keyPackages(KEY_PACKAGES) as Uint8Array[]);
      const lastResort = stored ? new Uint8Array() : device.lastResortKeyPackage();
      // Kept before the instance hands any of them out.
      await vault.write(vaultKey, { device: engine.saved() });
      const registered = await api.dms.registerDevice(
        { signatureKey: device.signatureKey, keyPackages: packages, lastResortKeyPackage: lastResort },
        CALL,
      );
      if (registered.keyPackages < LOW_ON_KEY_PACKAGES) {
        const more = device.keyPackages(KEY_PACKAGES) as Uint8Array[];
        await vault.write(vaultKey, { device: engine.saved() });
        await api.dms.addKeyPackages({ keyPackages: more }, CALL);
      }
      return engine;
    });
  }

  stop() {
    if (this.stopped) return;
    this.controller.abort();
    this.unwatch();
    this.backup.stop();
    this.tabs?.close();
    for (const timer of this.settling.values()) if (timer) clearTimeout(timer);
    try {
      this.device.free();
    } catch {
      // Already freed.
    }
  }

  /** Follows the account's conversations until stopped, reconnecting with backoff. */
  async follow(onProblem: (problem: string | null) => void) {
    const signal = this.controller.signal;
    let delay = 400;
    while (!signal.aborted) {
      const attempt = new AbortController();
      const abort = () => attempt.abort();
      signal.addEventListener("abort", abort);
      let silence = setTimeout(abort, SILENCE_MS);
      try {
        for await (const res of this.api.dms.watch({}, { signal: attempt.signal })) {
          clearTimeout(silence);
          silence = setTimeout(abort, SILENCE_MS);
          if (res.ready) {
            delay = 400;
            onProblem(null);
          }
          await this.receive(res);
        }
      } catch (err) {
        if (signal.aborted) return;
        const e = toFuwaError(err);
        // Signing out is the instance's sync to notice; an instance without DMs has nothing to follow.
        if (e.signedOut) return;
        if (e.code === Code.Unimplemented) {
          onProblem(i18n().t("system.e2ee.noDms"));
          return;
        }
        onProblem(e.message);
      } finally {
        clearTimeout(silence);
        signal.removeEventListener("abort", abort);
      }
      await new Promise((resolve) => setTimeout(resolve, delay * (0.75 + Math.random() / 2)));
      delay = Math.min(delay * 2, 20_000);
    }
  }

  /**
   * One response from the direct-message feed: at `ready` (listening again)
   * the conversations are listed and caught up on, so nothing said while
   * away is missed; after that, each event.
   */
  async receive(res: WatchResponse) {
    if (res.ready) await this.resync();
    if (res.event) this.onEvent(res.event);
  }

  /**
   * The same, for responses that come over the instance's live connection,
   * which doesn't wait for them: each is taken in order, after the last.
   */
  deliver(res: WatchResponse) {
    this.delivered = this.delivered
      .then(() => (this.stopped ? undefined : this.receive(res)))
      .catch((err: unknown) => console.warn("fuwa: couldn't catch up on direct messages", err));
  }

  /** Lists the conversations again and catches up on each. */
  async resync() {
    const { conversations } = await this.api.dms.listConversations({}, CALL);
    this.setConversations(conversations);
    // Calls aren't replayed, so they're listed again too. An instance from before calls has none.
    this.api.calls
      .listDmCalls({}, CALL)
      .then(({ calls }) => updateDms(this.key, (d) => ({ ...d, calls: Object.fromEntries(calls.map((c) => [c.conversationId, c])) })))
      .catch(() => {});
    const notes = new Map((await vault.loadNotes(this.vaultKey)).map((n) => [n.conversation, n]));
    for (const c of conversations) {
      const note = notes.get(c.id);
      if (!note || Number(c.lastSequence) > note.cursor || !this.device.isMember(c.id)) this.queue(c.id);
      else void this.refresh(c.id).catch(() => {});
    }
  }

  private setConversations(list: Conversation[]) {
    for (const c of list) this.conversations.set(c.id, c);
    const sorted = [...this.conversations.values()].sort(
      (a, b) => Number((b.updatedAt?.seconds ?? 0n) - (a.updatedAt?.seconds ?? 0n)) || (a.id < b.id ? 1 : -1),
    );
    updateDms(this.key, (d) => ({ ...d, conversations: sorted }));
  }

  /** A call starting, changing or ending (when nobody's left in it). */
  setCall(call: DmCall) {
    updateDms(this.key, (d) => {
      const { [call.conversationId]: _, ...rest } = d.calls;
      return { ...d, calls: call.participants.length ? { ...rest, [call.conversationId]: call } : rest };
    });
  }

  /**
   * The secret a call in this conversation encrypts its sound with: exported
   * from the conversation's group, so only its devices can work it out, and
   * new with every epoch (a device joining or leaving). `epoch` is one a
   * frame said it was sent in; when it's newer than this device's, the
   * device catches up on the conversation first.
   */
  async callSecret(id: string, epoch?: number): Promise<{ epoch: number; secret: Uint8Array }> {
    const read = () =>
      exclusive(this.lock, async () => {
        await this.fresh();
        return this.device.exportSecret(id, CALL_LABEL, 32) as { epoch: number; secret: Uint8Array };
      });
    let current = await read();
    if (epoch !== undefined && epoch > current.epoch) {
      await this.queue(id);
      current = await read();
    }
    return current;
  }

  /** A conversation someone just opened, as the instance answered. */
  add(conversation: Conversation) {
    this.setConversations([conversation]);
    this.queue(conversation.id);
  }

  private onEvent(event: DirectMessageEvent) {
    onDmTypingEvent(this.key, event);
    const p = event.payload;
    switch (p.case) {
      case "conversationOpened":
        this.add(p.value);
        break;
      case "recordAdded": {
        const c = this.conversations.get(p.value.conversationId);
        if (c) {
          this.setConversations([{ ...c, lastSequence: p.value.sequence, updatedAt: p.value.createdAt, epoch: c.epoch }]);
        }
        this.queue(p.value.conversationId);
        break;
      }
      case "recordDeleted":
        onDmPinEvent(this.key, event);
        void this.forgetDeleted(p.value.conversationId, Number(p.value.sequence)).catch(() => {});
        break;
      case "callUpdated":
        this.setCall(p.value);
        break;
      case "pinUpdated":
        onDmPinEvent(this.key, event);
        break;
    }
  }

  /** Catches up on a conversation, after whatever's already waiting. */
  queue(conversation: string): Promise<void> {
    if (this.queued.has(conversation)) return this.work;
    this.queued.add(conversation);
    this.work = this.work.then(async () => {
      this.queued.delete(conversation);
      if (this.stopped) return;
      try {
        await exclusive(this.lock, async () => {
          await this.fresh();
          await this.catchUp(conversation);
        });
        this.setBroken(conversation, false);
      } catch (err) {
        if (err instanceof DmError && err.message === SECURE_BROKEN) this.setBroken(conversation, true);
        else console.warn("fuwa: couldn't catch up on a conversation", err);
      }
      await this.refresh(conversation).catch(() => {});
    });
    return this.work;
  }

  /** Shows (or clears) that a secure channel's encryption can't be followed. */
  private setBroken(id: string, broken: boolean) {
    updateDms(this.key, (d) => {
      const now = d.blocked[id] ?? "";
      if (broken ? now === SECURE_BROKEN : now !== SECURE_BROKEN) return d;
      return { ...d, blocked: { ...d.blocked, [id]: broken ? SECURE_BROKEN : "" } };
    });
  }

  // ───────────────────────── Under the lock ─────────────────────────

  /** Picks up the device another tab saved since this one last looked. */
  private async fresh() {
    const stored = await vault.loadDevice(this.vaultKey);
    if (!stored || stored.session !== this.session) {
      this.stop();
      throw new DmError(i18n().t("system.e2ee.signedOut"));
    }
    if (stored.version !== this.version) {
      this.device.free();
      this.device = this.e2ee.Device.restore(stored.state);
      this.version = stored.version;
    }
  }

  /** The device as it is now, to write to the vault. */
  private saved(): vault.StoredDevice {
    this.version += 1;
    return { vault: this.vaultKey, session: this.session, state: this.device.save(), version: this.version };
  }

  /** How to reach a conversation or secure channel's group, if it's one this device knows. */
  private room(id: string): Room | undefined {
    const c = this.conversations.get(id);
    if (c) return this.conversationRoom(c);
    const sc = this.secure.get(id);
    if (sc) return this.channelRoom(id, sc);
    return undefined;
  }

  private conversationRoom(c: Conversation): Room {
    const dms = this.api.dms;
    const id = c.id;
    const allowed = c.users.map((u) => u.id);
    return {
      id,
      channel: null,
      allowed,
      belong: async () => allowed,
      check: (devices) => {
        const partner = c.users.find((u) => u.id !== this.me.id);
        if (partner && !devices.some((d) => d.userId === partner.id)) {
          const name = partner.displayName || partner.username;
          throw new DmError(
            partner.username === "deleted" ? i18n().t("system.e2ee.accountDeleted") : i18n().t("system.e2ee.notSignedIn", { name }),
          );
        }
      },
      records: (after) => dms.listRecords({ conversationId: id, afterSequence: BigInt(after), limit: PAGE }, CALL),
      welcome: async () => (await dms.listWelcomes({}, CALL)).welcomes.find((w) => w.conversationId === id),
      groupInfo: () => dms.getGroupInfo({ conversationId: id }, CALL),
      commit: async (commit, welcome) =>
        (
          await dms.postCommit(
            {
              conversationId: id,
              commit: commit.commit,
              groupInfo: commit.groupInfo,
              welcome: welcome ? (commit.welcome ?? new Uint8Array()) : new Uint8Array(),
              welcomeDeviceIds: welcome && commit.welcome ? commit.added.map((m) => m.deviceId) : [],
            },
            CALL,
          )
        ).record,
      message: async (message, mediaIds = []) => {
        await dms.postMessage({ conversationId: id, message, mediaIds }, CALL);
      },
      upload: async (size) => {
        const r = await dms.createSealedUpload({ conversationId: id, size: BigInt(size), kind: SealedKind.FILE }, UPLOAD);
        return { mediaId: r.mediaId, uploadUrl: r.uploadUrl };
      },
      shares: async () => false,
      history: async () => {},
      remove: async (seq) => {
        await dms.deleteRecord({ conversationId: id, sequence: BigInt(seq) }, CALL);
      },
      notify: (i) => onDirectMessage(this.key, id, c.users.find((u) => u.id === i.senderId), vault.lineText(i), i.at),
    };
  }

  private channelRoom(id: string, sc: SecureChannel): Room {
    const secure = this.api.secure;
    const at = { serverId: sc.serverId, channelId: id };
    const members = store.get().instances[this.key]?.members[sc.serverId] ?? [];
    // Someone added earlier may have lost access since; the group's history still names them.
    const allowed = unique([this.me.id, ...sc.memberIds, ...members.map((m) => m.user?.id ?? "")]);
    return {
      id,
      channel: { serverId: sc.serverId },
      allowed,
      belong: async () => {
        const { memberIds, shareHistory } = await secure.getSecureChannel(at, CALL);
        sc.memberIds = memberIds;
        sc.shareHistory = shareHistory;
        this.showHistorySetting(id, shareHistory);
        return memberIds;
      },
      check: (_devices, belong) => {
        if (!belong.includes(this.me.id)) throw new DmError(i18n().t("system.e2ee.cantSeeChannel"));
      },
      records: (after) => secure.listSecureRecords({ ...at, afterSequence: BigInt(after), limit: PAGE }, CALL),
      welcome: async () => (await secure.listSecureWelcomes({ serverId: sc.serverId }, CALL)).welcomes.find((w) => w.channelId === id),
      groupInfo: () => secure.getSecureGroupInfo(at, CALL),
      commit: async (commit, welcome) =>
        (
          await secure.postSecureCommit(
            {
              ...at,
              commit: commit.commit,
              groupInfo: commit.groupInfo,
              welcome: welcome ? (commit.welcome ?? new Uint8Array()) : new Uint8Array(),
              welcomeDeviceIds: welcome && commit.welcome ? commit.added.map((m) => m.deviceId) : [],
            },
            CALL,
          )
        ).record,
      message: async (message, mediaIds = []) => {
        await secure.postSecureMessage({ ...at, message, mediaIds }, CALL);
      },
      // An attachment upload for the server, like any channel's: only ever ciphertext here.
      upload: async (size) => {
        const r = await this.api.media.createUpload(
          { purpose: MediaPurpose.ATTACHMENT, contentType: "application/octet-stream", size: BigInt(size), serverId: sc.serverId },
          UPLOAD,
        );
        return { mediaId: r.media?.id ?? "", uploadUrl: r.uploadUrl };
      },
      shares: async () => {
        const { shareHistory } = await secure.getSecureChannel(at, CALL);
        sc.shareHistory = shareHistory;
        this.showHistorySetting(id, shareHistory);
        return shareHistory;
      },
      history: async (message) => {
        await secure.postSecureHistory({ ...at, message }, CALL);
      },
      remove: async (seq) => {
        await secure.deleteSecureRecord({ ...at, sequence: BigInt(seq) }, CALL);
      },
      notify: (i, answersMe) => onSecureMessage(this.key, sc.serverId, id, i.senderId, vault.lineText(i), i.at, answersMe),
    };
  }

  /** Reads a conversation's records this device hasn't, joining it first if it isn't in. */
  private async catchUp(id: string, depth = 0): Promise<void> {
    const c = this.room(id);
    if (!c) return;
    let note = await vault.loadNote(this.vaultKey, id);
    if (!this.device.isMember(id)) {
      updateDms(this.key, (d) => ({ ...d, joining: { ...d.joining, [id]: true } }));
      try {
        const joined = await this.join(c, note);
        if (!joined) return;
        note = joined;
      } finally {
        updateDms(this.key, (d) => ({ ...d, joining: { ...d.joining, [id]: false } }));
      }
    }
    const known = new Map((await vault.loadItems(this.vaultKey, id)).map((i) => [i.seq, i]));
    for (;;) {
      const { records, hasMore } = await c.records(note.cursor);
      const changed = new Map<number, vault.Item>();
      const had = new Set(known.keys());
      const forgetSent: string[] = [];
      let rejoin = false;
      for (const record of records) {
        const outcome = await this.open(c, record, known, changed, forgetSent);
        note = { ...note, cursor: Number(record.sequence) };
        if (outcome === "reset" && c.channel) this.restart(c.channel.serverId, id);
        if (outcome === "rejoin" || outcome === "reset") {
          rejoin = true;
          break;
        }
      }
      if (c.channel) {
        // A thread goes with its message: replies under a deleted one are dropped here (and only here).
        for (const i of threads.orphaned(known.values(), known)) {
          known.set(i.seq, i);
          changed.set(i.seq, i);
        }
      }
      await vault.write(this.vaultKey, { device: this.saved(), notes: [note], items: [...changed.values()], forgetSent });
      this.changed(id, changed.values());
      this.tell(id);
      const lines = c.channel ? [...known.values()] : [];
      for (const i of changed.values()) {
        if (!vault.isMessage(i) || had.has(i.seq) || i.senderId === this.me.id || i.sharedBy || i.deleted) continue;
        // A reply kept to its thread only reaches you if you follow the thread.
        const parent = c.channel ? threads.threadOf(i, known) : 0;
        if (parent && !i.inChannel && !threads.following(note, parent, lines, this.me.id)) continue;
        // A reply to one of yours reaches you like a mention: only this device can tell what it answers.
        c.notify(i, i.replyTo > 0 && known.get(i.replyTo)?.senderId === this.me.id);
      }
      if (rejoin) {
        if (depth < 2) await this.catchUp(id, depth + 1);
        return;
      }
      if (!hasMore || records.length === 0) return;
    }
  }

  /**
   * Opens one record and notes what it said. "rejoin" if this device has to
   * join the group again; "reset" if the channel's encryption started over.
   */
  private async open(
    c: Room,
    record: Rec,
    known: Map<number, vault.Item>,
    changed: Map<number, vault.Item>,
    forgetSent: string[],
  ): Promise<"rejoin" | "reset" | void> {
    const seq = Number(record.sequence);
    const at = ms(record);
    const put = (i: vault.Item) => {
      known.set(i.seq, i);
      changed.set(i.seq, i);
    };
    if (c.channel && record.kind === SecureRecordKind.SETTINGS) {
      const sc = this.secure.get(c.id);
      const on = (record as Rec & { shareHistory?: boolean }).shareHistory ?? false;
      if (sc) sc.shareHistory = on;
      this.showHistorySetting(c.id, on);
      put(item(this.vaultKey, c.id, { seq, at, kind: "setting", senderId: record.senderId, content: on ? "on" : "off" }));
      return;
    }
    if (c.channel && record.kind === SecureRecordKind.RESET) {
      // The group before it is gone; the next commit starts a new one.
      this.device.forget(c.id);
      put(item(this.vaultKey, c.id, { seq, at, kind: "reset", senderId: record.senderId }));
      return "reset";
    }
    if (record.data.length === 0) {
      // Deleted before this device read it.
      const before = known.get(seq);
      if (before && !before.deleted) put(threads.emptied(before));
      return;
    }
    const own = record.senderDeviceId === this.device.deviceId;
    let out: Processed;
    try {
      out = this.device.process(c.id, record.data, own, c.allowed) as Processed;
    } catch (err) {
      // A commit this device can't follow leaves it out of the group: it joins again.
      if ((err as Error).name === "Behind" || record.kind === ConversationRecordKind.COMMIT) {
        reportError("e2ee.lost_group", c.channel ? "secure_channel" : "dm");
        console.warn("fuwa: lost track of a conversation's group; joining it again", err);
        this.device.forget(c.id);
        return "rejoin";
      }
      put(item(this.vaultKey, c.id, { seq, at, kind: "unreadable", senderId: record.senderId, deviceId: record.senderDeviceId }));
      return;
    }
    const history = c.channel && record.kind === SecureRecordKind.HISTORY;
    switch (out.kind) {
      case "message":
        if (history) await this.takeHistory(c, out.sender.userId, out.plaintext, known, put);
        else this.read(c, seq, at, out.sender, out.plaintext, known, put);
        return;
      case "own": {
        // What this device passed on, the others already have.
        if (history) return;
        const hash = this.e2ee.sha256(record.data);
        const plaintext = await vault.loadSent(this.vaultKey, hash);
        if (plaintext) {
          this.read(c, seq, at, { userId: this.me.id, deviceId: this.device.deviceId, signatureKey: this.device.signatureKey }, plaintext, known, put);
          forgetSent.push(hash);
        } else {
          put(item(this.vaultKey, c.id, { seq, at, kind: "unreadable", senderId: this.me.id, deviceId: this.device.deviceId }));
        }
        return;
      }
      case "commit":
        if (out.removedMe) {
          this.device.forget(c.id);
          return "rejoin";
        }
        if (out.added.length || out.removed.length) {
          put(
            item(this.vaultKey, c.id, {
              seq,
              at,
              kind: "devices",
              senderId: out.by?.userId ?? record.senderId,
              deviceId: out.by?.deviceId ?? record.senderDeviceId,
              added: out.added.map(ref),
              removed: out.removed.map(ref),
            }),
          );
        }
        return;
      case "stale":
        return;
    }
  }

  /**
   * What a message said: new text, or an edit of the sender's own earlier
   * message. In a secure channel it comes signed by the device that sent it,
   * which is kept so it can be passed on to devices added later.
   */
  private read(
    c: Room,
    seq: number,
    at: number,
    sender: WasmMember,
    plaintext: Uint8Array,
    known: Map<number, vault.Item>,
    put: (i: vault.Item) => void,
  ) {
    const senderId = sender.userId;
    const deviceId = sender.deviceId;
    const unreadable = () => put(item(this.vaultKey, c.id, { seq, at, kind: "unreadable", senderId, deviceId }));
    let content: DirectMessageContent;
    let signed: vault.Signed | undefined;
    try {
      content = fromBinary(DirectMessageContentSchema, plaintext);
      if (content.body.case === "signed") {
        const opened = this.openSigned(c.id, content.body.value.payload, content.body.value.signature, sender.signatureKey);
        if (!opened || opened.payload.senderId !== senderId || !opened.payload.content) return unreadable();
        content = opened.payload.content;
        signed = opened.signed;
      }
    } catch {
      return unreadable();
    }
    const body = content.body;
    if (body.case === "text") {
      const text = body.value.content.slice(0, MAX_DM);
      put(
        item(this.vaultKey, c.id, {
          seq,
          at,
          kind: "text",
          senderId,
          deviceId,
          content: text,
          replyTo: Number(body.value.replyToSequence),
          ...(c.channel ? threadFields(body.value) : {}),
          ...filesField(body.value.files),
          ...emojisField(body.value.emojis, text, instanceOrigins(this.key)),
          signed,
        }),
      );
    } else if (body.case === "thread" && c.channel && signed) {
      // Whether it counts (only from someone with Manage Messages) is worked out where threads are shown.
      put(
        item(this.vaultKey, c.id, {
          seq,
          at,
          kind: "thread",
          senderId,
          deviceId,
          thread: Number(body.value.parentSequence),
          content: body.value.locked ? "locked" : "unlocked",
          signed,
        }),
      );
    } else if (body.case === "voice" && !c.channel) {
      const voice = voiceOf(body.value);
      if (!voice) return unreadable();
      put(
        item(this.vaultKey, c.id, {
          seq,
          at,
          kind: "voice",
          senderId,
          deviceId,
          content: i18n().t("system.e2ee.voiceLine", { length: voiceLength(voice.durationMs) }),
          replyTo: Number(body.value.replyToSequence),
          voice,
        }),
      );
    } else if (body.case === "edit") {
      const target = known.get(Number(body.value.sequence));
      if (target?.kind === "text" && target.senderId === senderId && !target.deleted) {
        const text = body.value.content.slice(0, MAX_DM);
        put({ ...target, content: text, emojis: emojisField(body.value.emojis, text, instanceOrigins(this.key)).emojis, editedAt: at, editSigned: signed });
      }
    } else if (body.case === "reaction" && (!c.channel || signed)) {
      // Kept as a line of its own, never shown: each device tallies them (lib/reactions.ts), the latest per sender and emoji winning.
      const target = Number(body.value.sequence);
      const reaction = reactionOf(body.value, instanceOrigins(this.key));
      if (target > 0 && target < seq && reaction) put(item(this.vaultKey, c.id, { seq, at, kind: "reaction", senderId, deviceId, reaction, signed }));
    }
    // Anything else is from a newer app: there's nothing to show for it here.
  }

  /** A signed payload for this conversation, if `key` signed it. */
  private openSigned(conversation: string, payload: Uint8Array, signature: Uint8Array, key: Uint8Array): Opened | null {
    if (!this.e2ee.verify(key, payload, signature)) return null;
    const opened = readPayload(payload);
    if (opened.conversationId !== conversation) return null;
    return { payload: opened, signed: { payload, signature, key } };
  }

  /** What this device sends in a secure channel: the content, signed by this device. */
  private signedContent(id: string, content: Content): Uint8Array {
    const payload = toBinary(
      SignedPayloadSchema,
      create(SignedPayloadSchema, { conversationId: id, senderId: this.me.id, sentAtMs: BigInt(Date.now()), content: contentOf(content) }),
    );
    const signature = this.device.sign(payload);
    return toBinary(
      DirectMessageContentSchema,
      create(DirectMessageContentSchema, { body: { case: "signed", value: create(SignedContentSchema, { payload, signature }) } }),
    );
  }

  /**
   * Earlier messages someone's device passed on when it added this one. Each
   * is checked against the signature of the device that sent it, which must
   * be one of its sender's devices now, and against the channel's log (see
   * history.ts); only messages from before this device joined, and not
   * already here, are taken. The rest are left out.
   */
  private async takeHistory(c: Room, by: string, plaintext: Uint8Array, known: Map<number, vault.Item>, put: (i: vault.Item) => void) {
    let shared: SharedHistory;
    try {
      const content = fromBinary(DirectMessageContentSchema, plaintext);
      if (content.body.case !== "history") return;
      shared = content.body.value;
    } catch {
      return;
    }
    if (!(await c.shares())) return;
    const joined = Math.max(0, ...[...known.values()].filter((i) => i.kind === "joined" && i.senderId === this.me.id).map((i) => i.seq));
    if (!joined) return;
    const opened: { seq: number; opened: Opened; deviceId: string }[] = [];
    for (const entry of shared.entries.slice(0, HISTORY_ENTRIES)) {
      const seq = Number(entry.sequence);
      if (seq <= 0 || seq >= joined) continue;
      try {
        const o = this.openSigned(c.id, entry.payload, entry.signature, entry.signatureKey);
        const body = o?.payload.content?.body;
        if (o && (body?.case === "text" || body?.case === "edit" || body?.case === "thread" || body?.case === "reaction")) opened.push({ seq, opened: o, deviceId: this.e2ee.deviceId(entry.signatureKey) });
      } catch {
        // Not a signed payload: left out.
      }
    }
    if (!opened.length) return;
    const allowedIds = new Set(c.allowed);
    const senders = unique(opened.map((e) => e.opened.payload.senderId)).filter((id) => allowedIds.has(id));
    const devices = new Set<string>();
    for (const userIds of chunks(senders, LOOKUPS)) {
      for (const d of (await this.api.dms.listDevices({ userIds }, CALL)).devices) devices.add(`${d.userId}/${d.id}`);
    }
    const log = await this.logBetween(c, Math.min(...opened.map((e) => e.seq)) - 1, joined);
    const candidates: history.Candidate[] = opened.map(({ seq, opened: o, deviceId }) => {
      const body = o.payload.content!.body;
      return body.case === "edit"
        ? { seq, senderId: o.payload.senderId, deviceId, kind: "edit", target: Number(body.value.sequence) }
        : { seq, senderId: o.payload.senderId, deviceId, kind: "text" };
    });
    for (const i of history.accept(candidates, { joined, allowed: c.allowed, devices, log: log.headers })) {
      const { seq, opened: o, deviceId } = opened[i];
      const senderId = o.payload.senderId;
      const at = log.at.get(seq) ?? Number(o.payload.sentAtMs);
      const body = o.payload.content!.body;
      if (body.case === "text") {
        if (known.has(seq)) continue;
        const text = body.value.content.slice(0, MAX_DM);
        put(
          item(this.vaultKey, c.id, {
            seq,
            at,
            kind: "text",
            senderId,
            deviceId,
            content: text,
            replyTo: Number(body.value.replyToSequence),
            ...threadFields(body.value),
            ...filesField(body.value.files),
            ...emojisField(body.value.emojis, text, instanceOrigins(this.key)),
            signed: o.signed,
            sharedBy: by,
          }),
        );
      } else if (body.case === "thread") {
        if (known.has(seq)) continue;
        put(
          item(this.vaultKey, c.id, {
            seq,
            at,
            kind: "thread",
            senderId,
            deviceId,
            thread: Number(body.value.parentSequence),
            content: body.value.locked ? "locked" : "unlocked",
            signed: o.signed,
            sharedBy: by,
          }),
        );
      } else if (body.case === "reaction") {
        // Checked like a text: its own record, from that device. Tallied with the rest; one to a message this
        // device doesn't have counts for nothing.
        const target = Number(body.value.sequence);
        const reaction = reactionOf(body.value, instanceOrigins(this.key));
        if (known.has(seq) || !(target > 0 && target < seq) || !reaction) continue;
        put(
          item(this.vaultKey, c.id, {
            seq,
            at,
            kind: "reaction",
            senderId,
            deviceId,
            reaction,
            signed: o.signed,
            sharedBy: by,
          }),
        );
      } else if (body.case === "edit") {
        const target = known.get(seq);
        if (target?.sharedBy && target.kind === "text" && target.senderId === senderId && !target.deleted) {
          const text = body.value.content.slice(0, MAX_DM);
          put({ ...target, content: text, emojis: emojisField(body.value.emojis, text, instanceOrigins(this.key)).emojis, editedAt: Number(o.payload.sentAtMs), editSigned: o.signed });
        }
      }
    }
  }

  /** The headers of a channel's records after `after` and before `before`: who sent what kind, and whether it's gone. */
  private async logBetween(c: Room, after: number, before: number) {
    const headers = new Map<number, history.Logged>();
    const at = new Map<number, number>();
    for (let page = 0, cursor = after; page < LOG_PAGES && cursor < before - 1; page++) {
      const { records, hasMore } = await c.records(cursor);
      for (const r of records) {
        const seq = Number(r.sequence);
        if (seq >= before) return { headers, at };
        const kind = r.kind === SecureRecordKind.MESSAGE ? "message" : r.kind === SecureRecordKind.SETTINGS ? "settings" : "other";
        const on = (r as Rec & { shareHistory?: boolean }).shareHistory ?? false;
        headers.set(seq, { kind, senderId: r.senderId, deviceId: r.senderDeviceId, deleted: r.data.length === 0, on });
        at.set(seq, ms(r));
        cursor = seq;
      }
      if (!hasMore || !records.length) break;
    }
    return { headers, at };
  }

  /**
   * Passes the newest earlier messages this device has, as their senders
   * signed them, on to the devices its commit just added. Only while the
   * channel shares history; the server takes one right after the commit.
   */
  private async shareHistory(c: Room) {
    if (!c.channel || !(await c.shares())) return;
    const all = await vault.loadItems(this.vaultKey, c.id);
    // What was said while sharing was off stays with those who were there.
    const since = Math.max(0, ...all.filter((i) => i.kind === "setting").map((i) => i.seq));
    const items = all
      .filter((i) => (i.kind === "text" || i.kind === "thread" || i.kind === "reaction") && !i.deleted && i.signed && i.seq > since)
      .sort((a, b) => b.seq - a.seq);
    const entries: ReturnType<typeof create<typeof SharedEntrySchema>>[] = [];
    let size = 0;
    for (const i of items) {
      const parts = [i.signed!, ...(i.editSigned ? [i.editSigned] : [])].map((s) =>
        create(SharedEntrySchema, { sequence: BigInt(i.seq), payload: s.payload, signature: s.signature, signatureKey: s.key }),
      );
      const bytes = parts.reduce((n, p) => n + p.payload.length + p.signature.length + p.signatureKey.length + 16, 0);
      if (size + bytes > HISTORY_BYTES || entries.length + parts.length > HISTORY_ENTRIES) break;
      size += bytes;
      // Newest first here; turned around below, so an edit follows its message.
      entries.push(...parts.reverse());
    }
    if (!entries.length) return;
    entries.reverse();
    const plaintext = toBinary(
      DirectMessageContentSchema,
      create(DirectMessageContentSchema, { body: { case: "history", value: create(SharedHistorySchema, { entries }) } }),
    );
    const ciphertext = this.device.encrypt(c.id, plaintext);
    await vault.write(this.vaultKey, { device: this.saved() });
    await c.history(ciphertext);
  }

  /** Tells the screens whether a secure channel shares history. */
  private showHistorySetting(id: string, on: boolean) {
    updateDms(this.key, (d) => (d.secureHistory[id] === on ? d : { ...d, secureHistory: { ...d.secureHistory, [id]: on } }));
  }

  /**
   * Joins a conversation's group: from a welcome if someone added this
   * device, or else by itself from the group's public state. Null if nobody
   * started the group yet.
   */
  private async join(c: Room, note: vault.Note): Promise<vault.Note | null> {
    const allowed = c.allowed;
    const welcome = await c.welcome();
    if (welcome) {
      try {
        this.device.joinFromWelcome(c.id, welcome.data, allowed);
        const seq = Number(welcome.sequence);
        const next = { ...note, cursor: seq };
        const joined = item(this.vaultKey, c.id, { seq, kind: "joined", senderId: this.me.id, deviceId: this.device.deviceId });
        await vault.write(this.vaultKey, { device: this.saved(), notes: [next], items: [joined] });
        this.joinedAt(c.id, joined);
        return next;
      } catch (err) {
        // Used up or out of date: join by itself instead.
        console.warn("fuwa: couldn't join a conversation from its welcome", err);
      }
    }
    for (let attempt = 0; attempt < 3; attempt++) {
      const info = await c.groupInfo();
      if (info.epoch === 0n || info.groupInfo.length === 0) return null;
      let commit: Commit;
      try {
        commit = this.device.joinByItself(c.id, info.groupInfo, allowed) as Commit;
      } catch (err) {
        if (!c.channel) throw err;
        // Nothing to join from: the group the server holds can't be read.
        reportError("e2ee.secure_broken", "secure_channel");
        throw new DmError(SECURE_BROKEN);
      }
      await vault.write(this.vaultKey, { device: this.saved() });
      try {
        const record = await c.commit(commit, false);
        const seq = Number(record?.sequence ?? 0n);
        const next = { ...note, cursor: seq };
        const joined = item(this.vaultKey, c.id, { seq, at: record ? ms(record) : Date.now(), kind: "joined", senderId: this.me.id, deviceId: this.device.deviceId });
        await vault.write(this.vaultKey, { notes: [next], items: [joined] });
        this.joinedAt(c.id, joined);
        return next;
      } catch (err) {
        this.device.forget(c.id);
        await vault.write(this.vaultKey, { device: this.saved() });
        if (!isPrecondition(err)) throw err;
      }
    }
    throw new DmError(i18n().t("system.e2ee.cantJoin"));
  }

  /**
   * Makes the group hold exactly the devices both people are signed in on:
   * starts it if nobody has, adds new devices, drops ones whose sessions ended.
   */
  private async reconcile(c: Room, attempt = 0): Promise<void> {
    const belong = await c.belong();
    // One lookup at a time: every client runs this on each membership change, so a big channel mustn't fan out.
    const devices: DeviceInfo[] = [];
    for (const userIds of chunks(belong, LOOKUPS)) devices.push(...(await this.api.dms.listDevices({ userIds }, CALL)).devices);
    c.check(devices, belong);
    const allowed = unique([...c.allowed, ...belong]);
    const starting = !this.device.isMember(c.id);
    if (starting) this.device.createGroup(c.id);
    const members = this.device.members(c.id) as WasmMember[];
    const present = new Set(members.map((m) => m.deviceId));
    const expected = new Set(devices.map((d) => d.id));
    const adds = devices.filter((d) => !present.has(d.id)).map((d) => d.id);
    const removes = members.filter((m) => !expected.has(m.deviceId) && m.deviceId !== this.device.deviceId).map((m) => m.deviceId);
    const claimed = [];
    for (const deviceIds of chunks(adds, LOOKUPS)) claimed.push(...(await this.api.dms.claimKeyPackages({ deviceIds }, CALL)).keyPackages);
    // A secure channel starts its group even when nobody else is signed in yet, so its first writer isn't stuck.
    if (!claimed.length && !removes.length && !(starting && c.channel)) {
      if (starting) this.device.forget(c.id);
      if (starting) throw new DmError(i18n().t("system.e2ee.cantReachDevices"));
      return;
    }
    const commit = this.device.commit(
      c.id,
      claimed.map((k) => ({ deviceId: k.deviceId, keyPackage: k.keyPackage })),
      removes,
      allowed,
    ) as Commit;
    await vault.write(this.vaultKey, { device: this.saved() });
    try {
      await c.commit(commit, true);
    } catch (err) {
      if (this.device.epoch(c.id) === 0) this.device.forget(c.id);
      else this.device.discardPending(c.id);
      await vault.write(this.vaultKey, { device: this.saved() });
      if (!isPrecondition(err) || attempt >= 2) throw err;
      // Someone else's commit got there first.
      await this.catchUp(c.id);
      return this.reconcile(c, attempt + 1);
    }
    await this.catchUp(c.id);
    if (claimed.length && c.channel) {
      // Best effort: messages keep working whether or not this goes through.
      await this.shareHistory(c).catch((err: unknown) => {
        if (!isPrecondition(err)) reportError("e2ee.secure_share_history", "secure_channel");
      });
      await this.catchUp(c.id);
    }
  }

  // ───────────────────────── What people do ─────────────────────────

  /** Gets a conversation ready to write in. Throws a DmError when it can't be yet. */
  prepare(id: string): Promise<void> {
    return exclusive(this.lock, async () => {
      await this.fresh();
      await this.catchUp(id);
      const c = this.room(id);
      if (!c) throw new DmError(i18n().t("system.e2ee.notHere"));
      await this.reconcile(c);
    }).finally(() => this.refresh(id).catch(() => {}));
  }

  /** Encrypts and sends a message (or an edit) to everyone in the conversation. */
  send(id: string, content: Content): Promise<void> {
    return exclusive(this.lock, async () => {
      await this.fresh();
      await this.catchUp(id);
      const c = this.room(id);
      if (!c) throw new DmError(i18n().t("system.e2ee.notHere"));
      if (c.channel && "voice" in content) throw new DmError(i18n().t("system.e2ee.noVoiceInSecure"));
      await this.reconcile(c);
      const plaintext = c.channel ? this.signedContent(id, content) : encode(content);
      const mediaIds = "voice" in content ? [content.voice.mediaId] : "files" in content ? (content.files ?? []).map((f) => f.mediaId) : [];
      for (let attempt = 0; ; attempt++) {
        const ciphertext = this.device.encrypt(id, plaintext);
        const hash = this.e2ee.sha256(ciphertext);
        // Kept first: this device can't open what it sent, so this is how it knows what it said.
        await vault.write(this.vaultKey, { device: this.saved(), sent: [{ hash, plaintext }] });
        try {
          await c.message(ciphertext, mediaIds);
          break;
        } catch (err) {
          await vault.write(this.vaultKey, { forgetSent: [hash] });
          if (!isPrecondition(err) || attempt >= 3) throw err;
          await this.catchUp(id);
        }
      }
      await this.catchUp(id);
    }).finally(() => this.refresh(id).catch(() => {}));
  }

  /** Reserves an upload for a sealed file to send in a conversation or secure channel. */
  async reserveUpload(id: string, size: number): Promise<{ mediaId: string; uploadUrl: string }> {
    const c = this.room(id);
    if (!c) throw new DmError(i18n().t("system.e2ee.notHere"));
    return c.upload(size);
  }

  /** Deletes a message you sent: from the instance, and from every device's copy. */
  async remove(id: string, seq: number) {
    const c = this.room(id);
    if (!c) throw new DmError(i18n().t("system.e2ee.notHere"));
    await threads.deleteLine({ ...this.lines(id), remove: (s) => c.remove(s) }, seq, this.secure.has(id));
    await this.refresh(id);
  }

  private async forgetDeleted(id: string, seq: number) {
    await threads.forgetLine(this.lines(id), seq, this.secure.has(id));
    await this.refresh(id);
  }

  /** One conversation's lines on this device, read and written under the device's lock. */
  private lines(id: string) {
    return {
      locked: (fn: () => Promise<void>) => exclusive(this.lock, fn),
      load: () => vault.loadItems(this.vaultKey, id),
      write: async (items: vault.Item[]) => {
        await vault.write(this.vaultKey, { items });
        this.changed(id, items);
        this.tell(id);
      },
    };
  }

  /** Notes that you've seen everything in a conversation so far. */
  async markRead(id: string) {
    const last = store.get().instances[this.key]?.dms.items[id]?.at(-1)?.seq ?? 0;
    await exclusive(this.lock, async () => {
      const note = await vault.loadNote(this.vaultKey, id);
      if (note.read >= last) return;
      await vault.write(this.vaultKey, { notes: [{ ...note, read: last }] });
      this.tell(id);
    });
    if (this.secure.has(id)) updateInstance(this.key, (i) => (i.unread[id] ? { ...i, unread: { ...i.unread, [id]: 0 } } : i));
    else updateDms(this.key, (d) => (d.unread[id] ? { ...d, unread: { ...d.unread, [id]: 0 } } : d));
  }

  /** Remembers that you compared this safety number with the other person (or forgets it, with ""). */
  async verify(id: string, safety: string) {
    await exclusive(this.lock, async () => {
      const note = await vault.loadNote(this.vaultKey, id);
      await vault.write(this.vaultKey, { notes: [{ ...note, verified: safety }] });
      this.tell(id);
    });
    await this.refresh(id);
  }

  /** Follows a secure channel's thread by hand, or stops (kept on this device only). */
  async followThread(id: string, parent: number, on: boolean) {
    await exclusive(this.lock, async () => {
      const note = await vault.loadNote(this.vaultKey, id);
      await vault.write(this.vaultKey, { notes: [{ ...note, follows: { ...note.follows, [parent]: on } }] });
      this.tell(id);
    });
    await this.refresh(id);
  }

  /** Notes that you've seen a secure channel's thread up to `seq`. */
  async markThreadRead(id: string, parent: number, seq: number) {
    await exclusive(this.lock, async () => {
      const note = await vault.loadNote(this.vaultKey, id);
      if ((note.threadRead?.[parent] ?? 0) >= seq) return;
      await vault.write(this.vaultKey, { notes: [{ ...note, threadRead: { ...note.threadRead, [parent]: seq } }] });
      this.tell(id);
    });
    await this.refresh(id);
  }

  // ───────────────────────── Showing it ─────────────────────────

  private tell(conversation: string) {
    this.tabs?.postMessage({ vault: this.vaultKey, conversation });
  }

  /** Whether a conversation is the one on screen. */
  private focused(id: string) {
    const f = store.get().focus;
    return f?.instance === this.key && f.channel === id;
  }

  /** Whether a conversation keeps a window of lines in the store: on screen, or opened lately. */
  private wanted(id: string) {
    return this.recent.includes(id) || this.focused(id);
  }

  /** A conversation came on screen: it keeps its window, and the one opened longest ago lets go of its own. */
  private onStore() {
    const f = store.get().focus;
    if (f === this.lastFocus || this.stopped) return;
    this.lastFocus = f;
    if (f?.instance !== this.key) return;
    const id = f.channel;
    if (!this.conversations.has(id) && !this.secure.has(id)) return;
    // Not from inside the store's own update.
    queueMicrotask(() => {
      if (this.stopped || !this.focused(id)) return;
      const shown = this.windows.has(id);
      this.touch(id);
      if (!shown) void this.refresh(id).catch(() => {});
    });
  }

  private touch(id: string) {
    if (this.recent[0] === id) return;
    const { recent, dropped } = win.touchRecent(this.recent, id);
    this.recent = recent;
    for (const d of dropped) this.shrink(d);
  }

  /** A conversation left a while ago: back to its tail in the store, the rest only in the vault. */
  private shrink(id: string) {
    const w = this.windows.get(id);
    this.windows.delete(id);
    this.changes.delete(id);
    if (!w) return;
    const tail = win.tailOf(w.lines);
    updateDms(this.key, (d) => ({ ...d, items: { ...d.items, [id]: tail }, reactions: without(d.reactions, id), more: without(d.more, id) }));
  }

  /** Lines just written: shown conversations put them in their windows at the next refresh, instead of reading everything again. */
  private changed(id: string, items: Iterable<vault.Item>) {
    if (!this.wanted(id)) return;
    const had = this.changes.get(id);
    if (had === "all") return;
    const map = had ?? new Map<number, vault.Item>();
    for (const i of items) map.set(i.seq, i);
    this.changes.set(id, map);
  }

  /** Something else changed a conversation's lines (another tab, the backup): read it again. */
  private changedAll(id: string) {
    this.late.delete(id);
    if (this.wanted(id)) this.changes.set(id, "all");
  }

  /** This device just joined a group: from a welcome or by itself. */
  private joinedAt(id: string, joined: vault.Item) {
    if (joined.seq > 1) this.late.set(id, true);
    this.changed(id, [joined]);
  }

  /** Runs `fn` after whatever else is changing this conversation's window. */
  private serial(id: string, fn: () => Promise<void>): Promise<void> {
    const run = (this.chains.get(id) ?? Promise.resolve()).then(fn);
    const tail = run.catch(() => {});
    this.chains.set(id, tail);
    void tail.then(() => {
      if (this.chains.get(id) === tail) this.chains.delete(id);
    });
    return run;
  }

  /** Puts what this browser knows about a conversation (or secure channel) in the store. */
  refresh(id: string): Promise<void> {
    const waiting = this.waiting.get(id);
    if (waiting) return waiting;
    const run = this.serial(id, () => {
      this.waiting.delete(id);
      return this.show(id);
    });
    this.waiting.set(id, run);
    return run;
  }

  /** A window as the vault has it: a secure channel's whole history (its threads need it), a direct message's latest lines. */
  private async loadWindow(id: string, secure: boolean, from = 0): Promise<win.Window> {
    if (secure) return win.windowOf(await vault.loadItems(this.vaultKey, id), 0);
    if (from > 0) return win.windowOf(await vault.loadItemsFrom(this.vaultKey, id, from), from);
    let n = 0;
    const { items, complete } = await vault.loadItemsBack(this.vaultKey, id, 0, (t) => t.at(-1)!.kind !== "reaction" && ++n >= win.OPEN_LINES);
    return win.windowOf(items, complete ? 0 : (items[0]?.seq ?? 0));
  }

  private async show(id: string) {
    const secure = this.secure.has(id);
    const c = this.conversations.get(id);
    if (this.stopped || (!secure && !c)) return;
    if (this.focused(id)) this.touch(id);
    const open = this.recent.includes(id);
    const pending = this.changes.get(id);
    this.changes.delete(id);
    const had = this.windows.get(id);
    const [note, members, window, tail] = await Promise.all([
      vault.loadNote(this.vaultKey, id),
      exclusive(this.lock, async () => {
        await this.fresh();
        return this.device.isMember(id) ? (this.device.members(id) as WasmMember[]) : [];
      }),
      !open
        ? undefined
        : had && pending !== "all"
          ? pending
            ? win.mergeWindow(had, pending.values())
            : had
          : this.loadWindow(id, secure, had?.from),
      // Not opened lately: only the last message on, for the list.
      open
        ? undefined
        : vault.loadItemsBack(this.vaultKey, id, 0, (t) => vault.isMessage(t.at(-1)!) || t.length >= win.TAIL_SCAN).then((r) => win.tailOf(r.items)),
    ]);
    if (this.stopped || (secure && !this.secure.has(id))) return;
    // Left while this was read: only the tail stays.
    const keep = window && this.recent.includes(id) ? window : undefined;
    if (keep) this.windows.set(id, keep);
    else this.windows.delete(id);
    const items = keep ? keep.lines : (tail ?? (window ? win.tailOf(window.lines) : []));
    const looking = this.focused(id) && document.visibilityState === "visible";
    const unread = looking ? 0 : await this.unreadIn(id, secure, note.read, keep);
    if (this.stopped) return;
    const reactions = keep ? (keep === had ? store.get().instances[this.key]?.dms.reactions[id] : undefined) ?? tallyDm([...keep.lines, ...keep.reactions], this.me.id) : undefined;
    const more = !!keep && keep.from > 0;
    const safety = c ? this.safetyNumber(c, members) : "";
    updateDms(this.key, (d) => {
      const next = {
        ...d,
        items: d.items[id] === items || (!keep && win.sameLines(d.items[id], items)) ? d.items : { ...d.items, [id]: items },
        reactions: reactions ? (d.reactions[id] === reactions ? d.reactions : { ...d.reactions, [id]: reactions }) : without(d.reactions, id),
        more: more ? (d.more[id] ? d.more : { ...d.more, [id]: true }) : without(d.more, id),
        members: { ...d.members, [id]: members satisfies DmMember[] },
      };
      if (secure) return { ...next, threadNotes: { ...d.threadNotes, [id]: { follows: note.follows ?? {}, read: note.threadRead ?? {} } } };
      return {
        ...next,
        unread: d.unread[id] === unread ? d.unread : { ...d.unread, [id]: unread },
        safety: d.safety[id] === safety ? d.safety : { ...d.safety, [id]: safety },
        verified: d.verified[id] === note.verified ? d.verified : { ...d.verified, [id]: note.verified },
      };
    });
    // A secure channel's unread count goes with the server's channels.
    if (secure) updateInstance(this.key, (i) => (i.unread[id] === unread ? i : { ...i, unread: { ...i.unread, [id]: unread } }));
    if (looking && items.length && note.read < items.at(-1)!.seq) void this.markRead(id).catch(() => {});
  }

  /**
   * Messages from others after the last one you saw: in a secure channel,
   * only those the channel shows (replies kept to their threads count there).
   * Read from the window when it reaches back that far, else from the vault.
   */
  private async unreadIn(id: string, secure: boolean, read: number, window: win.Window | undefined): Promise<number> {
    const lines =
      window && win.coversUnread(window, read)
        ? window.lines
        : (await vault.loadItemsFrom(this.vaultKey, id, read + 1)).filter((i) => i.kind !== "reaction");
    const fresh = lines.filter((i) => i.seq > read && !i.deleted && i.senderId !== this.me.id);
    if (!secure) return fresh.filter(vault.isMessage).length;
    const texts = fresh.filter((i) => i.kind === "text");
    const bySeq = new Map(lines.map((i) => [i.seq, i]));
    // The messages replies are under, from before what was read here.
    const parents = [...new Set(texts.map((i) => i.thread ?? 0).filter((p) => p > 0 && !bySeq.has(p)))];
    if (parents.length) {
      for (const p of await vault.loadItemsAt(parents.map((seq) => [this.vaultKey, id, seq]))) if (p) bySeq.set(p.seq, p);
    }
    return texts.filter((i) => !threads.threadOf(i, bySeq) || !!i.inChannel).length;
  }

  /** Reaches an opened conversation's window further back, by one step. */
  loadOlderLines(id: string): Promise<void> {
    return this.serial(id, async () => {
      const w = this.windows.get(id);
      if (!w || w.from === 0) return;
      let n = 0;
      const { items, complete } = await vault.loadItemsBack(this.vaultKey, id, w.from, (t) => t.at(-1)!.kind !== "reaction" && ++n >= win.OPEN_LINES);
      this.extend(id, w, items, complete);
    });
  }

  /** Reaches an opened conversation's window back to `seq` (a pin, a reply's message), when it doesn't yet. */
  loadLinesThrough(id: string, seq: number): Promise<void> {
    return this.serial(id, async () => {
      const w = this.windows.get(id);
      if (!w || w.from === 0 || seq >= w.from) return;
      const { items, complete } = await vault.loadItemsBack(this.vaultKey, id, w.from, (t) => t.at(-1)!.seq <= seq);
      this.extend(id, w, items, complete);
    });
  }

  private extend(id: string, w: win.Window, older: vault.Item[], complete: boolean) {
    if (this.stopped || this.windows.get(id) !== w) return;
    const next = win.extendWindow(w, older, complete ? 0 : (older[0]?.seq ?? w.from));
    this.windows.set(id, next);
    const reactions = tallyDm([...next.lines, ...next.reactions], this.me.id);
    updateDms(this.key, (d) => ({
      ...d,
      items: { ...d.items, [id]: next.lines },
      reactions: { ...d.reactions, [id]: reactions },
      more: next.from > 0 ? { ...d.more, [id]: true } : without(d.more, id),
    }));
  }

  /** Whether this device joined a secure channel after it started (so it holds less of its history). */
  private async joinedLate(id: string): Promise<boolean> {
    const known = this.late.get(id);
    if (known !== undefined) return known;
    const w = this.windows.get(id);
    const lines = w && w.from === 0 ? w.lines : await vault.loadItems(this.vaultKey, id);
    const late = lines.some((i) => i.kind === "joined" && i.senderId === this.me.id && i.seq > 1);
    this.late.set(id, late);
    return late;
  }

  // ───────────────────────── Secure channels ─────────────────────────

  /** Starts following a secure channel (opened, or it had news): catches up on it. */
  followChannel(serverId: string, channelId: string): Promise<void> {
    if (!this.secure.has(channelId)) this.secure.set(channelId, { serverId, memberIds: [], shareHistory: false });
    return this.queue(channelId);
  }

  /** Follows the secure channels this device was already in, when their server comes in: what came while away is read. */
  async followServer(serverId: string, channelIds: string[]) {
    const notes = new Set((await vault.loadNotes(this.vaultKey)).map((n) => n.conversation));
    for (const id of channelIds) if (notes.has(id)) void this.followChannel(serverId, id);
  }

  /** A server event, as it arrives live: secure channels' records, and changes to who can see them. */
  onServerEvent(event: Event) {
    const p = event.payload;
    switch (p.case) {
      case "secureRecordAdded":
        if (p.value.record) void this.followChannel(event.serverId, p.value.record.channelId);
        return;
      case "secureRecordDeleted":
        if (this.secure.has(p.value.channelId)) void this.forgetDeleted(p.value.channelId, Number(p.value.sequence)).catch(() => {});
        return;
      case "channelDeleted":
        if (this.secure.has(p.value.channelId)) void this.leaveChannel(p.value.channelId).catch(() => {});
        return;
      case "channelUpdated":
      case "roleUpdated":
      case "roleDeleted":
      case "memberUpdated":
      case "memberLeft":
      case "memberJoined":
        this.settle(event.serverId);
        return;
    }
  }

  /**
   * After a change to who can see what, brings the server's secure channels
   * this device is in back in step: people who lost access go, people who
   * gained it come in. Every device that's online would do it, so each waits
   * a moment first; the first commit wins and the rest find nothing to do.
   */
  private settle(serverId: string) {
    if (this.settling.has(serverId)) return;
    const ids = [...this.secure].filter(([, sc]) => sc.serverId === serverId).map(([id]) => id);
    if (!ids.length) return;
    this.settling.set(serverId, null);
    // The device whose commit adds someone is the one that passes history on,
    // so devices that joined after the channel started (and hold less of it)
    // let the others go first.
    void Promise.all(ids.map((id) => this.joinedLate(id).catch(() => false))).then((lates) => {
      if (this.stopped || this.settling.get(serverId) !== null) return;
      const late = lates.some(Boolean);
      const timer = setTimeout(
        () => {
          this.settling.delete(serverId);
          for (const id of ids) {
            // Only people who may write commit for the group; the server refuses the others'.
            if (!this.secure.has(id) || !this.canWrite(serverId, id)) continue;
            void exclusive(this.lock, async () => {
              await this.fresh();
              await this.catchUp(id);
              const c = this.room(id);
              if (c && this.device.isMember(id)) await this.reconcile(c);
            })
              .catch((err: unknown) => {
                if (!(err instanceof DmError)) reportError("e2ee.secure_settle", "secure_channel");
              })
              .finally(() => void this.refresh(id).catch(() => {}));
          }
        },
        400 + Math.random() * ACCESS_SETTLE_MS + (late ? LATE_SETTLE_MS : 0),
      );
      this.settling.set(serverId, timer);
    });
  }

  /**
   * After a channel's encryption started over: someone who may write starts
   * its new group. Every such device that's online would, so each waits a
   * moment; the first commit wins and the rest join from its welcome.
   */
  private restart(serverId: string, id: string) {
    if (!this.canWrite(serverId, id)) return;
    setTimeout(
      () => {
        if (this.secure.has(id) && !this.stopped) void this.prepare(id).catch(() => {});
      },
      400 + Math.random() * ACCESS_SETTLE_MS,
    );
  }

  /** Whether you may write in a server's channel, worked out as the server does, from what this app knows. */
  private canWrite(serverId: string, channelId: string): boolean {
    const i = store.get().instances[this.key];
    const server = byId(i?.servers, serverId);
    if (!i || !server) return false;
    const member = memberById(i.members[serverId], this.me.id);
    const access = accessOf(
      serverId,
      server.ownerId,
      i.roles[serverId] ?? [],
      i.channels[serverId] ?? [],
      this.me.id,
      member?.roleIds ?? [],
      !!member?.pending && server.hasRules,
      !!member?.timedOutUntil && timestampMs(member.timedOutUntil) > Date.now(),
    );
    return hasIn(access, channelId, Permission.SEND_MESSAGES);
  }

  /** A secure channel you can't see any more (or that was deleted): this device lets go of it and what it kept. */
  private async leaveChannel(id: string) {
    this.secure.delete(id);
    await exclusive(this.lock, async () => {
      await this.fresh();
      this.device.forget(id);
      await vault.write(this.vaultKey, { device: this.saved() });
      await vault.forget(this.vaultKey, id);
      this.tell(id);
    });
    this.windows.delete(id);
    this.changes.delete(id);
    this.late.delete(id);
    this.recent = this.recent.filter((x) => x !== id);
    updateDms(this.key, (d) => ({
      ...d,
      items: without(d.items, id),
      reactions: without(d.reactions, id),
      more: without(d.more, id),
      members: without(d.members, id),
    }));
  }

  /** Both people's safety number, from the devices in the group. Empty until both have one there. */
  private safetyNumber(c: Conversation, members: WasmMember[]): string {
    const partner = c.users.find((u) => u.id !== this.me.id);
    if (!partner) return "";
    const keys = (userId: string) => members.filter((m) => m.userId === userId).map((m) => m.signatureKey);
    const mine = keys(this.me.id);
    const theirs = keys(partner.id);
    if (!mine.length || !theirs.length) return "";
    return this.e2ee.safetyNumber(this.me.id, mine, partner.id, theirs);
  }
}

// ───────────────────────── One engine per instance ─────────────────────────

const engines = new Map<string, DmEngine>();
const starting = new Map<string, symbol>();

export const dmEngine = (key: string): DmEngine | undefined => engines.get(key);

/**
 * Starts direct messages for an instance's signed-in account. Failing leaves
 * the rest of the app alone. With `live`, the feed comes over the instance's
 * live connection (`DmEngine.deliver`) instead of a stream of its own.
 */
export function startDms(key: string, api: Api, me: User, token: string, live = false) {
  stopDms(key);
  if (typeof indexedDB === "undefined" || typeof WebAssembly === "undefined") {
    updateDms(key, (d) => ({ ...d, status: "unsupported", problem: i18n().t("system.e2ee.unsupported") }));
    return;
  }
  const attempt = Symbol(key);
  starting.set(key, attempt);
  updateDms(key, (d) => ({ ...d, status: "starting", problem: null }));
  DmEngine.start(key, api, me, token)
    .then((engine) => {
      if (starting.get(key) !== attempt) return engine.stop();
      starting.delete(key);
      engines.set(key, engine);
      updateDms(key, (d) => ({ ...d, status: "ready", deviceId: engine.deviceId }));
      // On a live connection, the feed's `ready` may have come before the
      // device was ready, so it catches up once by itself.
      if (live) engine.deliver(create(WatchResponseSchema, { ready: true }));
      else void engine.follow((problem) => updateDms(key, (d) => ({ ...d, problem })));
      engine.backup.start().catch((err: unknown) => {
        console.warn("fuwa: couldn't check the message backup", err);
        updateDms(key, (d) => ({ ...d, backup: { ...d.backup, problem: toFuwaError(err).message } }));
      });
    })
    .catch((err: unknown) => {
      if (starting.get(key) !== attempt) return;
      starting.delete(key);
      const e = toFuwaError(err);
      console.warn("fuwa: direct messages didn't start", err);
      updateDms(key, (d) => ({
        ...d,
        status: "failed",
        problem: e.code === Code.Unimplemented ? i18n().t("system.e2ee.noDms") : e.message,
      }));
    });
}

export function stopDms(key: string) {
  starting.delete(key);
  engines.get(key)?.stop();
  engines.delete(key);
}

/**
 * Forgets everything this browser kept for direct messages and secure
 * channels as one account on an instance (signing out of it), or, without
 * `userId`, as every account on it (forgetting the instance). Other
 * accounts' devices are left alone.
 */
export async function wipeDms(key: string, userId?: string) {
  if (userId === undefined || engines.get(key)?.me.id === userId) stopDms(key);
  try {
    if (userId === undefined) await vault.wipe(`${key}|`);
    else await vault.wipeVault(`${key}|${userId}`);
  } catch {
    console.warn("fuwa: couldn't wipe encrypted messages from this browser");
  }
}
