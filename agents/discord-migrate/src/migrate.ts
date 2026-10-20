// One migration: a Discord server copied into a fuwa server, step by step,
// each step written down (state.ts) as it's done so a stopped run picks up
// where it left off.
//
//   1. the server's name, description, icon and banner
//   2. roles, with their colors and permissions, in Discord's order, and @everyone's permissions
//   3. custom emoji, and stickers fuwa can show as emoji
//   4. soundboard sounds, turned into Ogg Opus
//   5. categories and channels, in Discord's order, with their roles' permission overwrites
//   6. an invite that never runs out, for the members
//   7. each text channel's history, through a webhook per channel so every
//      message shows its author's name and picture; files follow from the
//      agent, pins are pinned again, and a divider marks each new day. Then
//      its threads, as fuwa threads under the copied messages they started
//      from, with the agent posting each reply under its author's name. A
//      forum's posts each open with their first message through the webhook,
//      with the rest of the post as its thread.
import {
  type Mapping,
  chunks,
  cleanName,
  dayDivider,
  dayOf,
  displayOrder,
  embedsOf,
  emojiName,
  messageText,
  overwritesOf,
  permissionsOf,
  replyText,
  soundName,
  MAX_FILES,
} from "./convert.ts";
import {
  type Download,
  type Hook,
  Refused,
  type Source,
  type SourceGuild,
  type SourceMessage,
  type SourceThread,
  type Target,
  type TargetFile,
  type TargetPost,
} from "./model.ts";
import type { ChannelHistory, JobState, StateStore } from "./state.ts";

export interface MigrateOptions {
  /** The most recent messages to copy from each text channel; 0 copies none. */
  messagesPerChannel: number;
  /** Only messages (and forum posts) from this time on, in milliseconds since 1970. */
  since?: number;
  /** How many channels' histories are copied at once. */
  channelsAtOnce: number;
  /** Files bigger than this are named instead of copied. */
  maxFileBytes: number;
  /** What to copy; everything unless set. History also needs `messagesPerChannel`. */
  parts?: readonly Part[];
  /** Discord channels to leave out; a category leaves out the channels in it too. */
  leaveOut?: readonly string[];
  signal?: AbortSignal;
  /** Called whenever something changes; `report` is the same object each time. */
  onProgress?: (report: Report) => void;
}

export type Phase = "server" | "roles" | "emoji" | "sounds" | "channels" | "invite" | "history" | "done";
/** A part of a server someone can choose to copy or not. */
export type Part = Exclude<Phase, "done">;
export const PARTS: readonly Part[] = ["server", "roles", "emoji", "sounds", "channels", "invite", "history"];

/** How a migration is going, and what it did. */
export interface Report {
  phase: Phase;
  guildName: string;
  /** What was chosen to copy. */
  parts: Part[];
  roles: number;
  emojis: number;
  stickers: number;
  sounds: number;
  channels: number;
  /** Text channels whose history is copied, and how many there are. */
  historyDone: number;
  historyTotal: number;
  messages: number;
  /** Threads and forum posts. */
  threads: number;
  files: number;
  pins: number;
  invite?: string;
  /** What couldn't be copied, and why: the first few, with a count of the rest. */
  skipped: string[];
  skippedMore: number;
}

/** The migration can't start: what's missing, in words for the person who ran it. */
export class NotReadyError extends Error {}

const MAX_SKIPPED = 15;

/** Notes something that wasn't copied; with the error that stopped it, unless that error should stop the run. */
type Skip = (what: string, err?: unknown) => void;
/** What the webhooks post under when a message isn't anyone's (the day dividers). */
export const IMPORTER_NAME = "Discord";

export function newReport(guildName = ""): Report {
  return {
    phase: "server",
    guildName,
    parts: [...PARTS],
    roles: 0,
    emojis: 0,
    stickers: 0,
    sounds: 0,
    channels: 0,
    historyDone: 0,
    historyTotal: 0,
    messages: 0,
    threads: 0,
    files: 0,
    pins: 0,
    skipped: [],
    skippedMore: 0,
  };
}

export async function migrate(
  source: Source,
  target: Target,
  store: StateStore,
  options: MigrateOptions,
  report: Report = newReport(),
): Promise<Report> {
  const { signal } = options;
  const whole = await source.guild();
  report.guildName = whole.name;
  report.parts = PARTS.filter(
    (p) => (options.parts ?? PARTS).includes(p) && (p !== "history" || options.messagesPerChannel > 0),
  );
  const wanted = (p: Part) => report.parts.includes(p);
  const out = new Set(options.leaveOut);
  // The channels left out aren't copied, nor their history, and aren't listed as skipped either.
  const guild = { ...whole, channels: whole.channels.filter((c) => !out.has(c.id) && !out.has(c.parentId ?? "")) };
  const missing = await target.missing();
  if (missing.length) throw new NotReadyError(missing.join("\n"));
  const soundboard = await target.soundboard();
  const state = await store.load(guild.id, target.serverId);
  const save = () => store.save(state);
  const progress = () => options.onProgress?.(report);
  const skip: Skip = (what, err) => {
    // Only what fuwa turned down is passed over; anything else stops the run.
    if (err !== undefined && !(err instanceof Refused)) throw err;
    if (err !== undefined) what = `${what} (${reason(err)})`;
    if (report.skipped.length < MAX_SKIPPED) report.skipped.push(what);
    else report.skippedMore++;
    progress();
  };
  const step = async (phase: Part, work: () => Promise<void>) => {
    if (!wanted(phase)) return;
    signal?.throwIfAborted();
    report.phase = phase;
    progress();
    await work();
  };

  await step("server", async () => {
    if (state.server) return;
    const icon = guild.iconUrl ? await tryDownload(source, guild.iconUrl, 10 << 20) : undefined;
    const banner = guild.bannerUrl ? await tryDownload(source, guild.bannerUrl, 10 << 20) : undefined;
    try {
      await target.updateServer({
        name: cleanName(guild.name, "Discord server"),
        description: guild.description,
        icon,
        banner,
      });
    } catch (err) {
      skip("The server's name, icon or banner", err);
    }
    state.server = true;
    await save();
  });

  await step("roles", () => copyRoles(guild, target, state, save, report, skip, soundboard));
  await step("emoji", () => copyEmoji(guild, source, target, state, save, report, skip, signal));
  await step("sounds", async () => {
    if (soundboard) await copySounds(guild, source, target, state, save, report, skip, signal);
    else if (guild.sounds.length && !state.soundsDone) {
      skip(
        `${guild.sounds.length === 1 ? "The sound" : `${guild.sounds.length} sounds`} (this fuwa has no soundboard yet)`,
      );
      state.soundsDone = true;
      await save();
    }
  });
  await step("channels", () => copyChannels(guild, target, state, save, report, skip, soundboard, signal));

  await step("invite", async () => {
    if (!state.invite) {
      try {
        state.invite = await target.createInvite();
        await save();
      } catch (err) {
        skip("An invite", err);
      }
    }
    report.invite = state.invite;
  });

  await step("history", async () => {
    const ctx: Ctx = {
      source,
      target,
      state,
      save,
      options,
      report,
      skip,
      progress,
      map: {
        guildId: guild.id,
        serverId: target.serverId,
        roles: state.roles,
        roleNames: Object.fromEntries(whole.roles.map((r) => [r.id, r.name])),
        channels: state.channels,
        channelNames: Object.fromEntries(whole.channels.map((c) => [c.id, c.name])),
        emojis: { ...state.emojis },
      },
    };
    const channels = displayOrder(guild.channels).filter(
      (c) => (c.kind === "text" || c.kind === "announcement" || c.kind === "forum") && state.channels[c.id],
    );
    report.historyTotal = channels.length;
    report.historyDone = channels.filter((c) => state.history[c.id]?.done).length;
    for (const c of channels) {
      const history = state.history[c.id];
      report.messages += history?.messages ?? 0;
      for (const thread of Object.keys(history?.threads ?? {})) {
        report.threads++;
        report.messages += state.history[thread]?.messages ?? 0;
      }
    }
    progress();
    const todo = channels.filter((c) => !state.history[c.id]?.done);
    await pool(todo, Math.max(1, options.channelsAtOnce), async (channel) => {
      const fuwaChannel = state.channels[channel.id]!;
      const history = (state.history[channel.id] ??= { messages: 0 });
      let hook: Hook;
      try {
        hook = await target.createHook(fuwaChannel, IMPORTER_NAME);
      } catch (err) {
        skip(`#${channel.name}'s messages`, err);
        return;
      }
      const where = { hook, channelId: fuwaChannel, name: channel.name };
      try {
        if (channel.kind === "forum") {
          for (const post of await source.forumPosts(channel.id, options.messagesPerChannel, options.since)) {
            await copyThread(ctx, where, history, post, true);
          }
        } else {
          await copyChannel(ctx, where, channel.id, history);
          for (const [id, anchor] of Object.entries(history.threads ?? {})) {
            await copyThread(ctx, where, history, { id, name: "" }, false, anchor);
          }
        }
        history.done = true;
        report.historyDone++;
        await save();
        progress();
      } catch (err) {
        skip(`#${channel.name}'s messages`, err);
      } finally {
        await target.deleteHook(hook).catch(() => {});
      }
    });
  });

  report.phase = "done";
  progress();
  return report;
}

async function copyRoles(
  guild: SourceGuild,
  target: Target,
  state: JobState,
  save: () => Promise<void>,
  report: Report,
  skip: Skip,
  soundboard: boolean,
): Promise<void> {
  const roles = guild.roles.filter((r) => !r.managed && !r.everyone).sort((a, b) => a.position - b.position);
  report.roles = roles.filter((r) => state.roles[r.id]).length;
  if (state.rolesDone) return;
  for (const role of roles) {
    if (state.roles[role.id]) continue;
    try {
      state.roles[role.id] = await target.createRole({
        name: cleanName(role.name, "role"),
        color: role.color || undefined,
        permissions: permissionsOf(role.permissions, false, soundboard),
        hoist: role.hoist,
        mentionable: role.mentionable,
      });
      report.roles++;
      await save();
    } catch (err) {
      skip(`The role ${role.name}`, err);
    }
  }
  const everyone = guild.roles.find((r) => r.everyone);
  if (everyone) {
    await target
      .setEveryone(permissionsOf(everyone.permissions, false, soundboard))
      .catch((err) => skip("@everyone's permissions", err));
  }
  const highestFirst = [...roles].reverse().flatMap((r) => state.roles[r.id] ?? []);
  if (highestFirst.length) await target.orderRoles(highestFirst).catch((err) => skip("The roles' order", err));
  state.rolesDone = true;
  await save();
}

async function copyEmoji(
  guild: SourceGuild,
  source: Source,
  target: Target,
  state: JobState,
  save: () => Promise<void>,
  report: Report,
  skip: Skip,
  signal?: AbortSignal,
): Promise<void> {
  report.emojis = Object.keys(state.emojis).length;
  report.stickers = Object.keys(state.stickers).length;
  if (state.emojisDone) return;
  const taken = new Set(
    [...Object.values(state.emojis), ...Object.values(state.stickers)].map((e) => e.name.toLowerCase()),
  );
  const copy = async (name: string, url: string, what: string) => {
    signal?.throwIfAborted();
    const picture = await tryDownload(source, url, 2 << 20);
    if (!picture) {
      skip(`${what} (couldn't download it)`);
      return undefined;
    }
    try {
      return await target.createEmoji(emojiName(name, taken), picture);
    } catch (err) {
      skip(what, err);
      return undefined;
    }
  };
  for (const emoji of guild.emojis) {
    if (state.emojis[emoji.id]) continue;
    const made = await copy(emoji.name, emoji.url, `The emoji :${emoji.name}:`);
    if (!made) continue;
    state.emojis[emoji.id] = made;
    report.emojis++;
    await save();
  }
  for (const sticker of guild.stickers) {
    if (state.stickers[sticker.id]) continue;
    if (sticker.format === "lottie") {
      skip(`The sticker ${sticker.name} (an animated drawing fuwa can't show)`);
      continue;
    }
    const made = await copy(sticker.name, sticker.url, `The sticker ${sticker.name}`);
    if (!made) continue;
    state.stickers[sticker.id] = made;
    report.stickers++;
    await save();
  }
  state.emojisDone = true;
  await save();
}

async function copySounds(
  guild: SourceGuild,
  source: Source,
  target: Target,
  state: JobState,
  save: () => Promise<void>,
  report: Report,
  skip: Skip,
  signal?: AbortSignal,
): Promise<void> {
  report.sounds = Object.keys(state.sounds).length;
  if (state.soundsDone) return;
  const taken = new Set(Object.values(state.sounds).map((n) => n.toLowerCase()));
  for (const sound of guild.sounds) {
    signal?.throwIfAborted();
    if (state.sounds[sound.id]) continue;
    const what = `The sound ${sound.name}`;
    // Discord keeps sounds under 512 KB.
    const file = await tryDownload(source, sound.url, 2 << 20);
    if (!file) {
      skip(`${what} (couldn't download it)`);
      continue;
    }
    const own = sound.emojiId ? state.emojis[sound.emojiId] : undefined;
    const emoji = own ? `<:${own.name}:${own.id}>` : sound.emojiId ? "" : (sound.emojiName ?? "");
    const name = soundName(sound.name, taken);
    try {
      await target.createSound({ name, emoji, volume: sound.volume, file });
    } catch (err) {
      skip(what, err);
      continue;
    }
    state.sounds[sound.id] = name;
    report.sounds++;
    await save();
  }
  state.soundsDone = true;
  await save();
}

async function copyChannels(
  guild: SourceGuild,
  target: Target,
  state: JobState,
  save: () => Promise<void>,
  report: Report,
  skip: Skip,
  soundboard: boolean,
  signal?: AbortSignal,
): Promise<void> {
  const ordered = displayOrder(guild.channels);
  report.channels = ordered.filter((c) => state.channels[c.id]).length;
  // Done before, unless channels left out then are wanted now.
  if (state.channelsDone && ordered.every((c) => c.kind === "other" || state.channels[c.id])) return;
  // Categories first, so channels can go in them.
  const categoriesFirst = [
    ...ordered.filter((c) => c.kind === "category"),
    ...ordered.filter((c) => c.kind !== "category"),
  ];
  for (const channel of categoriesFirst) {
    signal?.throwIfAborted();
    if (state.channels[channel.id]) continue;
    if (channel.kind === "other") {
      skip(`#${channel.name} (a kind of channel fuwa doesn't have)`);
      continue;
    }
    try {
      state.channels[channel.id] = await target.createChannel({
        name: cleanName(channel.name, "channel"),
        kind: channel.kind === "forum" ? "text" : channel.kind,
        parentId: channel.parentId ? state.channels[channel.parentId] : undefined,
        topic: [...channel.topic].slice(0, 1024).join(""),
        slowmodeSeconds: Math.min(channel.slowmodeSeconds, 21_600),
        overwrites: overwritesOf(channel.overwrites, guild.id, target.serverId, state.roles, soundboard),
      });
      report.channels++;
      await save();
    } catch (err) {
      skip(`#${channel.name}`, err);
    }
  }
  const placed = ordered.flatMap((c) => {
    const id = state.channels[c.id];
    if (!id) return [];
    return [{ id, parentId: c.parentId ? state.channels[c.parentId] : undefined }];
  });
  if (placed.length) await target.orderChannels(placed).catch((err) => skip("The channels' order", err));
  state.channelsDone = true;
  await save();
}

/** What copying history needs, the same for every channel. */
interface Ctx {
  source: Source;
  target: Target;
  state: JobState;
  save: () => Promise<void>;
  options: MigrateOptions;
  report: Report;
  skip: Skip;
  progress: () => void;
  map: Mapping;
}

/** Where a channel's history goes: its webhook and its fuwa channel. */
interface Where {
  hook: Hook;
  channelId: string;
  /** The Discord channel's name, for the summary. */
  name: string;
}

/** Most webhook posts sent in one batch, and about the most bytes of text in one. */
export const BATCH_POSTS = 25;
const BATCH_BYTES = 128 * 1024;

type Step =
  | { post: TargetPost; done?: (id: string) => void | Promise<void>; failed?: (err: Refused) => void }
  | { then: () => void | Promise<void> };

/**
 * Webhook posts that go out together in one call (fuwa's batch endpoint),
 * in order, with what to do once each has its id. What a batch changes in
 * the saved state waits for it too, so a stop never notes as copied what
 * didn't go out.
 */
class Batch {
  #steps: Step[] = [];
  #posts = 0;
  #bytes = 0;

  readonly ctx: Ctx;
  readonly where: Where;

  constructor(ctx: Ctx, where: Where) {
    this.ctx = ctx;
    this.where = where;
  }

  add(post: TargetPost, done?: (id: string) => void | Promise<void>, failed?: (err: Refused) => void) {
    this.#steps.push({ post, done, failed });
    this.#posts++;
    this.#bytes += post.content.length + JSON.stringify(post.embeds).length;
  }

  /** Runs once everything added before it went out. */
  then(work: () => void | Promise<void>) {
    this.#steps.push({ then: work });
  }

  get full(): boolean {
    return this.#posts >= BATCH_POSTS || this.#bytes >= BATCH_BYTES;
  }

  async flush(): Promise<void> {
    const steps = this.#steps.splice(0);
    this.#posts = 0;
    this.#bytes = 0;
    if (!steps.length) return;
    const posts = steps.flatMap((s) => ("post" in s ? [s.post] : []));
    const results = posts.length ? await this.ctx.target.postMany(this.where.hook, posts) : [];
    let i = 0;
    for (const step of steps) {
      if (!("post" in step)) {
        await step.then();
        continue;
      }
      const result = results[i++]!;
      if (result instanceof Refused) step.failed?.(result);
      else await step.done?.(result);
    }
    await this.ctx.save();
    this.ctx.progress();
  }
}

/** A text channel's latest messages, through its webhook in batches, noting the threads they started. */
async function copyChannel(ctx: Ctx, where: Where, channelId: string, history: ChannelHistory): Promise<void> {
  const { source, options, report } = ctx;
  const batch = new Batch(ctx, where);
  let lastDay = history.lastDay;
  const messages = source.messages(channelId, options.messagesPerChannel, history.lastId, options.since);
  for await (const message of messages) {
    options.signal?.throwIfAborted();
    // A message with files has gone out already, so what it changes is saved straight away.
    let sent = false;
    if (!message.system) {
      const day = dayOf(message.at);
      if (day !== lastDay) {
        lastDay = day;
        batch.add({ content: dayDivider(message.at), username: IMPORTER_NAME, embeds: [] });
        batch.then(() => {
          history.lastDay = day;
        });
      }
      sent = await copyMessage(ctx, batch, message, undefined, (firstId) => {
        if (!message.threadId) return;
        (history.threads ??= {})[message.threadId] = firstId;
        report.threads++;
      });
      batch.then(() => {
        history.messages++;
        report.messages++;
      });
    }
    batch.then(() => {
      history.lastId = message.id;
    });
    if (sent || batch.full) await batch.flush();
  }
  await batch.flush();
}

/**
 * A thread's messages as replies under `anchor`, the copy of the message it
 * started from. A forum post has none yet: its first message opens it
 * through the webhook, with the post's title, and the rest are its replies.
 */
async function copyThread(
  ctx: Ctx,
  where: Where,
  channel: ChannelHistory,
  thread: SourceThread,
  forum: boolean,
  anchor?: string,
): Promise<void> {
  const { source, state, options, report, save, progress } = ctx;
  const history = (state.history[thread.id] ??= { messages: 0 });
  if (history.done) return;
  anchor ??= channel.threads?.[thread.id];
  const what = forum ? `The post ${thread.name} in #${where.name}` : `A thread in #${where.name}`;
  try {
    const messages = source.messages(thread.id, options.messagesPerChannel, history.lastId, options.since);
    for await (const message of messages) {
      options.signal?.throwIfAborted();
      if (!message.system) {
        if (!anchor && forum) {
          const opening = message.id === thread.id;
          const title = `**${thread.name}**`;
          const batch = new Batch(ctx, where);
          const opened = (id: string) => {
            anchor = id;
          };
          if (opening) await copyMessage(ctx, batch, message, title, opened);
          else batch.add({ content: title, username: IMPORTER_NAME, embeds: [] }, opened);
          await batch.flush();
          if (!anchor) break;
          (channel.threads ??= {})[thread.id] = anchor;
          report.threads++;
          if (opening) {
            history.messages++;
            report.messages++;
          }
          await save();
        }
        if (anchor && !(forum && message.id === thread.id)) {
          await copyReply(ctx, where, anchor, message);
          history.messages++;
          report.messages++;
        }
      }
      history.lastId = message.id;
      await save();
      progress();
    }
  } catch (err) {
    ctx.skip(what, err);
  }
  history.done = true;
  await save();
}

/** Downloads a message's files; the ones too big or gone are named instead. */
async function filesOf(ctx: Ctx, message: SourceMessage): Promise<{ files: TargetFile[]; left: string[] }> {
  const { options, source } = ctx;
  const files: TargetFile[] = [];
  const left: string[] = [];
  for (const a of message.attachments) {
    options.signal?.throwIfAborted();
    const got = a.size <= options.maxFileBytes ? await tryDownload(source, a.url, options.maxFileBytes) : undefined;
    if (!got) {
      left.push(a.name);
      continue;
    }
    files.push({
      name: a.name,
      bytes: got.bytes,
      contentType: a.contentType || got.contentType,
      width: a.width,
      height: a.height,
      spoiler: a.spoiler,
    });
  }
  return { files, left };
}

function leftText(text: string, left: string[]): string {
  if (!left.length) return text;
  return `${text}${text ? "\n" : ""}*(${left.length === 1 ? "a file" : `${left.length} files`} not copied: ${left.join(", ")})*`;
}

/**
 * Adds one message to the batch, and sends its files right after it as the
 * agent (going out with the batch so far); `first` gets the first fuwa
 * message's id once it has one. Whether the batch went out for its files.
 */
async function copyMessage(
  ctx: Ctx,
  batch: Batch,
  message: SourceMessage,
  title?: string,
  first?: (id: string) => void,
): Promise<boolean> {
  const { target, report, skip, map } = ctx;
  const { where } = batch;
  const { files, left } = await filesOf(ctx, message);
  let text = leftText(messageText(message, map), left);
  if (title) text = text ? `${title}\n${text}` : title;
  const embeds = embedsOf(message.embeds);
  const pieces = chunks(text);
  if (!pieces.length && embeds.length) pieces.push("");
  const post = { username: cleanName(message.authorName, "someone", 80), avatarUrl: message.authorAvatarUrl };
  let firstId: string | undefined;
  const got = async (id: string) => {
    if (firstId) return;
    firstId = id;
    first?.(id);
    if (!message.pinned) return;
    try {
      await target.pin(where.channelId, id);
      report.pins++;
    } catch (err) {
      skip(`A pin in #${where.name}`, err);
    }
  };
  let failed = false;
  const fail = (err: Refused) => {
    if (failed) return;
    failed = true;
    skip(`A message by ${message.authorName} in #${where.name}`, err);
  };
  for (const [i, piece] of pieces.entries()) {
    batch.add({ ...post, content: piece, embeds: i === pieces.length - 1 ? embeds : [] }, got, fail);
  }
  if (!files.length) return false;
  await batch.flush();
  for (let i = 0; i < files.length; i += MAX_FILES) {
    try {
      const id = await target.sendFiles(
        where.channelId,
        `Files from **${post.username}**`,
        files.slice(i, i + MAX_FILES),
      );
      report.files += Math.min(MAX_FILES, files.length - i);
      await got(id);
    } catch (err) {
      skip(`Files from ${message.authorName} in #${where.name}`, err);
    }
  }
  return true;
}

/** Posts one message in a thread as the agent, under its author's name, with its files. */
async function copyReply(ctx: Ctx, where: Where, anchor: string, message: SourceMessage): Promise<void> {
  const { target, report, skip, map } = ctx;
  const { files, left } = await filesOf(ctx, message);
  const pieces = chunks(leftText(replyText(message, map), left));
  for (const [i, piece] of pieces.entries()) {
    const last = i === pieces.length - 1;
    // Files go with the last piece, ten to a message.
    const batches = last && files.length ? Math.ceil(files.length / MAX_FILES) : 1;
    for (let b = 0; b < batches; b++) {
      const batch = last ? files.slice(b * MAX_FILES, (b + 1) * MAX_FILES) : [];
      try {
        await target.reply(where.channelId, anchor, b === 0 ? piece : "", batch);
        report.files += batch.length;
      } catch (err) {
        skip(`A reply by ${message.authorName} in a thread in #${where.name}`, err);
      }
    }
  }
}

async function tryDownload(source: Source, url: string, maxBytes: number): Promise<Download | undefined> {
  try {
    return await source.download(url, maxBytes);
  } catch {
    return undefined;
  }
}

/** Why something failed, short, for the summary. */
export function reason(err: unknown): string {
  const text = err instanceof Error ? err.message : String(err);
  const line = text
    .split("\n")[0]!
    .replace(/^\[\w+\]\s*/, "")
    .trim();
  return line.length > 120 ? `${line.slice(0, 119)}…` : line || "it failed";
}

/** Runs `work` on each item, at most `size` at a time; the first failure stops the rest and is thrown. */
export async function pool<T>(items: T[], size: number, work: (item: T) => Promise<void>): Promise<void> {
  let next = 0;
  let failed: { err: unknown } | undefined;
  const runner = async () => {
    while (!failed && next < items.length) {
      const item = items[next++]!;
      try {
        await work(item);
      } catch (err) {
        failed ??= { err };
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(size, items.length) }, runner));
  if (failed) throw failed.err;
}
