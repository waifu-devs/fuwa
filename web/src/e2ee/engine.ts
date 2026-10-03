import { create, fromBinary, toBinary } from "@bufbuild/protobuf";
import { timestampMs } from "@bufbuild/protobuf/wkt";
import { Code } from "@connectrpc/connect";
import type { Api } from "@/fuwa/client";
import { toFuwaError } from "@/fuwa/errors";
import { store, updateDms, type DmMember } from "@/fuwa/store";
import { onDirectMessage } from "@/lib/notify";
import {
  ConversationRecordKind,
  DirectMessageContentSchema,
  DirectMessageEditSchema,
  DirectMessageTextSchema,
  type Conversation,
  type ConversationRecord,
  type DirectMessageContent,
  type DirectMessageEvent,
} from "@/gen/fuwa/v1/dm_pb";
import type { DmCall } from "@/gen/fuwa/v1/call_pb";
import type { User } from "@/gen/fuwa/v1/types_pb";
import * as vault from "./vault";
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
/** The longest message, in characters, as the composer allows. */
export const MAX_DM = 4000;

/** What a conversation's group exports its call secret under. */
const CALL_LABEL = "fuwa call v1";

/** Something a person can be told about why sending didn't work. */
export class DmError extends Error {
  override name = "DmError";
}

const ms = (record: ConversationRecord) => (record.createdAt ? timestampMs(record.createdAt) : Date.now());
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
export type Content = { text: string; replyTo?: number } | { edit: number; text: string };

function encode(content: Content): Uint8Array {
  const body: DirectMessageContent["body"] =
    "edit" in content
      ? { case: "edit", value: create(DirectMessageEditSchema, { sequence: BigInt(content.edit), content: content.text }) }
      : { case: "text", value: create(DirectMessageTextSchema, { content: content.text, replyToSequence: BigInt(content.replyTo ?? 0) }) };
  return toBinary(DirectMessageContentSchema, create(DirectMessageContentSchema, { body }));
}

export class DmEngine {
  private readonly lock: string;
  private readonly controller = new AbortController();
  private readonly tabs: BroadcastChannel | null;
  private conversations = new Map<string, Conversation>();
  /** Catch-ups waiting their turn, so a burst of records reads each conversation once. */
  private queued = new Set<string>();
  private work: Promise<void> = Promise.resolve();

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
    this.tabs = typeof BroadcastChannel === "undefined" ? null : new BroadcastChannel("fuwa-e2ee");
    if (this.tabs) {
      this.tabs.onmessage = (e: MessageEvent<{ vault: string; conversation?: string; list?: boolean }>) => {
        if (e.data?.vault !== this.vaultKey || this.stopped) return;
        if (e.data.list) void this.resync().catch(() => {});
        else if (e.data.conversation) void this.refresh(e.data.conversation).catch(() => {});
      };
    }
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
        await vault.wipe(vaultKey);
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
    this.tabs?.close();
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
            await this.resync();
          }
          if (res.event) this.onEvent(res.event);
        }
      } catch (err) {
        if (signal.aborted) return;
        const e = toFuwaError(err);
        // Signing out is the instance's sync to notice; an instance without DMs has nothing to follow.
        if (e.signedOut) return;
        if (e.code === Code.Unimplemented) {
          onProblem("This instance doesn't have direct messages yet.");
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
        void this.forgetDeleted(p.value.conversationId, Number(p.value.sequence)).catch(() => {});
        break;
      case "callUpdated":
        this.setCall(p.value);
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
      } catch (err) {
        console.warn("fuwa: couldn't catch up on a conversation", err);
      }
      await this.refresh(conversation).catch(() => {});
    });
    return this.work;
  }

  // ───────────────────────── Under the lock ─────────────────────────

  /** Picks up the device another tab saved since this one last looked. */
  private async fresh() {
    const stored = await vault.loadDevice(this.vaultKey);
    if (!stored || stored.session !== this.session) {
      this.stop();
      throw new DmError("signed out");
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

  private allowed(c: Conversation) {
    return c.users.map((u) => u.id);
  }

  /** Reads a conversation's records this device hasn't, joining it first if it isn't in. */
  private async catchUp(id: string, depth = 0): Promise<void> {
    const c = this.conversations.get(id);
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
      const { records, hasMore } = await this.api.dms.listRecords(
        { conversationId: id, afterSequence: BigInt(note.cursor), limit: PAGE },
        CALL,
      );
      const changed = new Map<number, vault.Item>();
      const had = new Set(known.keys());
      const forgetSent: string[] = [];
      let rejoin = false;
      for (const record of records) {
        const outcome = await this.open(c, record, known, changed, forgetSent);
        note = { ...note, cursor: Number(record.sequence) };
        if (outcome === "rejoin") {
          rejoin = true;
          break;
        }
      }
      await vault.write(this.vaultKey, { device: this.saved(), notes: [note], items: [...changed.values()], forgetSent });
      this.tell(id);
      for (const i of changed.values()) {
        if (i.kind === "text" && !had.has(i.seq) && i.senderId !== this.me.id)
          onDirectMessage(this.key, id, c.users.find((u) => u.id === i.senderId), i.content, i.at);
      }
      if (rejoin) {
        if (depth < 2) await this.catchUp(id, depth + 1);
        return;
      }
      if (!hasMore || records.length === 0) return;
    }
  }

  /** Opens one record and notes what it said. "rejoin" if this device has to join the group again. */
  private async open(
    c: Conversation,
    record: ConversationRecord,
    known: Map<number, vault.Item>,
    changed: Map<number, vault.Item>,
    forgetSent: string[],
  ): Promise<"rejoin" | void> {
    const seq = Number(record.sequence);
    const at = ms(record);
    const put = (i: vault.Item) => {
      known.set(i.seq, i);
      changed.set(i.seq, i);
    };
    if (record.data.length === 0) {
      // Deleted before this device read it.
      const before = known.get(seq);
      if (before && !before.deleted) put({ ...before, deleted: true, content: "" });
      return;
    }
    const own = record.senderDeviceId === this.device.deviceId;
    let out: Processed;
    try {
      out = this.device.process(c.id, record.data, own, this.allowed(c)) as Processed;
    } catch (err) {
      // A commit this device can't follow leaves it out of the group: it joins again.
      if ((err as Error).name === "Behind" || record.kind === ConversationRecordKind.COMMIT) {
        console.warn("fuwa: lost track of a conversation's group; joining it again", err);
        this.device.forget(c.id);
        return "rejoin";
      }
      put(item(this.vaultKey, c.id, { seq, at, kind: "unreadable", senderId: record.senderId, deviceId: record.senderDeviceId }));
      return;
    }
    switch (out.kind) {
      case "message":
        this.read(c.id, seq, at, out.sender.userId, out.sender.deviceId, out.plaintext, known, put);
        return;
      case "own": {
        const hash = this.e2ee.sha256(record.data);
        const plaintext = await vault.loadSent(this.vaultKey, hash);
        if (plaintext) {
          this.read(c.id, seq, at, this.me.id, this.device.deviceId, plaintext, known, put);
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

  /** What a message said: new text, or an edit of the sender's own earlier message. */
  private read(
    conversation: string,
    seq: number,
    at: number,
    senderId: string,
    deviceId: string,
    plaintext: Uint8Array,
    known: Map<number, vault.Item>,
    put: (i: vault.Item) => void,
  ) {
    let content: DirectMessageContent;
    try {
      content = fromBinary(DirectMessageContentSchema, plaintext);
    } catch {
      put(item(this.vaultKey, conversation, { seq, at, kind: "unreadable", senderId, deviceId }));
      return;
    }
    const body = content.body;
    if (body.case === "text") {
      put(
        item(this.vaultKey, conversation, {
          seq,
          at,
          kind: "text",
          senderId,
          deviceId,
          content: body.value.content.slice(0, MAX_DM),
          replyTo: Number(body.value.replyToSequence),
        }),
      );
    } else if (body.case === "edit") {
      const target = known.get(Number(body.value.sequence));
      if (target?.kind === "text" && target.senderId === senderId && !target.deleted) {
        put({ ...target, content: body.value.content.slice(0, MAX_DM), editedAt: at });
      }
    }
    // Anything else is from a newer app: there's nothing to show for it here.
  }

  /**
   * Joins a conversation's group: from a welcome if someone added this
   * device, or else by itself from the group's public state. Null if nobody
   * started the group yet.
   */
  private async join(c: Conversation, note: vault.Note): Promise<vault.Note | null> {
    const allowed = this.allowed(c);
    const { welcomes } = await this.api.dms.listWelcomes({}, CALL);
    const welcome = welcomes.find((w) => w.conversationId === c.id);
    if (welcome) {
      try {
        this.device.joinFromWelcome(c.id, welcome.data, allowed);
        const seq = Number(welcome.sequence);
        const next = { ...note, cursor: seq };
        await vault.write(this.vaultKey, {
          device: this.saved(),
          notes: [next],
          items: [item(this.vaultKey, c.id, { seq, kind: "joined", senderId: this.me.id, deviceId: this.device.deviceId })],
        });
        return next;
      } catch (err) {
        // Used up or out of date: join by itself instead.
        console.warn("fuwa: couldn't join a conversation from its welcome", err);
      }
    }
    for (let attempt = 0; attempt < 3; attempt++) {
      const info = await this.api.dms.getGroupInfo({ conversationId: c.id }, CALL);
      if (info.epoch === 0n || info.groupInfo.length === 0) return null;
      const commit = this.device.joinByItself(c.id, info.groupInfo, allowed) as Commit;
      await vault.write(this.vaultKey, { device: this.saved() });
      try {
        const { record } = await this.api.dms.postCommit(
          { conversationId: c.id, commit: commit.commit, groupInfo: commit.groupInfo },
          CALL,
        );
        const seq = Number(record?.sequence ?? 0n);
        const next = { ...note, cursor: seq };
        await vault.write(this.vaultKey, {
          notes: [next],
          items: [
            item(this.vaultKey, c.id, { seq, at: record ? ms(record) : Date.now(), kind: "joined", senderId: this.me.id, deviceId: this.device.deviceId }),
          ],
        });
        return next;
      } catch (err) {
        this.device.forget(c.id);
        await vault.write(this.vaultKey, { device: this.saved() });
        if (!isPrecondition(err)) throw err;
      }
    }
    throw new DmError("couldn't join this conversation; try again");
  }

  /**
   * Makes the group hold exactly the devices both people are signed in on:
   * starts it if nobody has, adds new devices, drops ones whose sessions ended.
   */
  private async reconcile(c: Conversation, attempt = 0): Promise<void> {
    const allowed = this.allowed(c);
    const { devices } = await this.api.dms.listDevices({ userIds: allowed }, CALL);
    const partner = c.users.find((u) => u.id !== this.me.id);
    if (partner && !devices.some((d) => d.userId === partner.id)) {
      const name = partner.displayName || partner.username;
      throw new DmError(
        partner.username === "deleted"
          ? "This account was deleted."
          : `${name} isn't signed in to fuwa anywhere that can receive encrypted messages yet. You can write once they are.`,
      );
    }
    const starting = !this.device.isMember(c.id);
    if (starting) this.device.createGroup(c.id);
    const members = this.device.members(c.id) as WasmMember[];
    const present = new Set(members.map((m) => m.deviceId));
    const expected = new Set(devices.map((d) => d.id));
    const adds = devices.filter((d) => !present.has(d.id)).map((d) => d.id);
    const removes = members.filter((m) => !expected.has(m.deviceId) && m.deviceId !== this.device.deviceId).map((m) => m.deviceId);
    const claimed = adds.length ? (await this.api.dms.claimKeyPackages({ deviceIds: adds }, CALL)).keyPackages : [];
    if (!claimed.length && !removes.length) {
      if (starting) this.device.forget(c.id);
      if (starting && partner) throw new DmError("couldn't reach their devices yet; try again in a moment");
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
      await this.api.dms.postCommit(
        {
          conversationId: c.id,
          commit: commit.commit,
          groupInfo: commit.groupInfo,
          welcome: commit.welcome ?? new Uint8Array(),
          welcomeDeviceIds: commit.welcome ? commit.added.map((m) => m.deviceId) : [],
        },
        CALL,
      );
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
  }

  // ───────────────────────── What people do ─────────────────────────

  /** Gets a conversation ready to write in. Throws a DmError when it can't be yet. */
  prepare(id: string): Promise<void> {
    return exclusive(this.lock, async () => {
      await this.fresh();
      const c = this.conversations.get(id);
      if (!c) throw new DmError("that conversation isn't here");
      await this.catchUp(id);
      await this.reconcile(c);
    }).finally(() => this.refresh(id).catch(() => {}));
  }

  /** Encrypts and sends a message (or an edit) to everyone in the conversation. */
  send(id: string, content: Content): Promise<void> {
    return exclusive(this.lock, async () => {
      await this.fresh();
      const c = this.conversations.get(id);
      if (!c) throw new DmError("that conversation isn't here");
      await this.catchUp(id);
      await this.reconcile(c);
      const plaintext = encode(content);
      for (let attempt = 0; ; attempt++) {
        const ciphertext = this.device.encrypt(id, plaintext);
        const hash = this.e2ee.sha256(ciphertext);
        // Kept first: this device can't open what it sent, so this is how it knows what it said.
        await vault.write(this.vaultKey, { device: this.saved(), sent: [{ hash, plaintext }] });
        try {
          await this.api.dms.postMessage({ conversationId: id, message: ciphertext }, CALL);
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

  /** Deletes a message you sent: from the instance, and from every device's copy. */
  async remove(id: string, seq: number) {
    await this.api.dms.deleteRecord({ conversationId: id, sequence: BigInt(seq) }, CALL);
    await this.forgetDeleted(id, seq);
  }

  private async forgetDeleted(id: string, seq: number) {
    await exclusive(this.lock, async () => {
      const before = (await vault.loadItems(this.vaultKey, id)).find((i) => i.seq === seq);
      if (before && !before.deleted) await vault.write(this.vaultKey, { items: [{ ...before, deleted: true, content: "" }] });
      this.tell(id);
    });
    await this.refresh(id);
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
    updateDms(this.key, (d) => (d.unread[id] ? { ...d, unread: { ...d.unread, [id]: 0 } } : d));
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

  // ───────────────────────── Showing it ─────────────────────────

  private tell(conversation: string) {
    this.tabs?.postMessage({ vault: this.vaultKey, conversation });
  }

  /** Puts what this browser knows about a conversation in the store. */
  async refresh(id: string) {
    const c = this.conversations.get(id);
    if (!c || this.stopped) return;
    const [items, note, members] = await Promise.all([
      vault.loadItems(this.vaultKey, id),
      vault.loadNote(this.vaultKey, id),
      exclusive(this.lock, async () => {
        await this.fresh();
        return this.device.isMember(id) ? (this.device.members(id) as WasmMember[]) : [];
      }),
    ]);
    if (this.stopped) return;
    const focused = store.get().focus;
    const looking = focused?.instance === this.key && focused.channel === id && document.visibilityState === "visible";
    const unread = looking
      ? 0
      : items.filter((i) => i.kind === "text" && !i.deleted && i.senderId !== this.me.id && i.seq > note.read).length;
    const safety = this.safetyNumber(c, members);
    updateDms(this.key, (d) => ({
      ...d,
      items: { ...d.items, [id]: items },
      unread: { ...d.unread, [id]: unread },
      members: { ...d.members, [id]: members satisfies DmMember[] },
      safety: { ...d.safety, [id]: safety },
      verified: { ...d.verified, [id]: note.verified },
    }));
    if (looking && items.length && note.read < items.at(-1)!.seq) void this.markRead(id).catch(() => {});
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

/** Starts direct messages for an instance's signed-in account. Failing leaves the rest of the app alone. */
export function startDms(key: string, api: Api, me: User, token: string) {
  stopDms(key);
  if (typeof indexedDB === "undefined" || typeof WebAssembly === "undefined") {
    updateDms(key, (d) => ({ ...d, status: "unsupported", problem: "This browser can't keep encrypted messages." }));
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
      void engine.follow((problem) => updateDms(key, (d) => ({ ...d, problem })));
    })
    .catch((err: unknown) => {
      if (starting.get(key) !== attempt) return;
      starting.delete(key);
      const e = toFuwaError(err);
      console.warn("fuwa: direct messages didn't start", err);
      updateDms(key, (d) => ({
        ...d,
        status: "failed",
        problem: e.code === Code.Unimplemented ? "This instance doesn't have direct messages yet." : e.message,
      }));
    });
}

export function stopDms(key: string) {
  starting.delete(key);
  engines.get(key)?.stop();
  engines.delete(key);
}

/** Stops direct messages and forgets everything this browser kept for them on an instance: on signing out. */
export async function wipeDms(key: string) {
  stopDms(key);
  try {
    await vault.wipe(`${key}|`);
  } catch (err) {
    console.warn("fuwa: couldn't wipe encrypted messages from this browser", err);
  }
}
