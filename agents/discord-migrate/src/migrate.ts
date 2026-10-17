// One migration: a Discord server copied into a fuwa server, step by step,
// each step written down (state.ts) as it's done so a stopped run picks up
// where it left off.
//
//   1. the server's name, description, icon and banner
//   2. roles, with their colors and permissions, in Discord's order, and @everyone's permissions
//   3. custom emoji, and stickers fuwa can show as emoji
//   4. categories and channels, in Discord's order, with their roles' permission overwrites
//   5. an invite that never runs out, for the members
//   6. each text channel's history, through a webhook per channel so every
//      message shows its author's name and picture; files follow from the
//      agent, pins are pinned again, and a divider marks each new day
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
  MAX_FILES,
} from "./convert.ts";
import {
  type Download,
  type Hook,
  Refused,
  type Source,
  type SourceGuild,
  type SourceMessage,
  type Target,
  type TargetFile,
} from "./model.ts";
import type { JobState, StateStore } from "./state.ts";

export interface MigrateOptions {
  /** The most recent messages to copy from each text channel; 0 copies none. */
  messagesPerChannel: number;
  /** How many channels' histories are copied at once. */
  channelsAtOnce: number;
  /** Files bigger than this are named instead of copied. */
  maxFileBytes: number;
  signal?: AbortSignal;
  /** Called whenever something changes; `report` is the same object each time. */
  onProgress?: (report: Report) => void;
}

export type Phase = "server" | "roles" | "emoji" | "channels" | "invite" | "history" | "done";

/** How a migration is going, and what it did. */
export interface Report {
  phase: Phase;
  guildName: string;
  roles: number;
  emojis: number;
  stickers: number;
  channels: number;
  /** Text channels whose history is copied, and how many there are. */
  historyDone: number;
  historyTotal: number;
  messages: number;
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
    roles: 0,
    emojis: 0,
    stickers: 0,
    channels: 0,
    historyDone: 0,
    historyTotal: 0,
    messages: 0,
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
  const guild = await source.guild();
  report.guildName = guild.name;
  const missing = await target.missing();
  if (missing.length) throw new NotReadyError(missing.join("\n"));
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
  const step = async (phase: Phase, work: () => Promise<void>) => {
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

  await step("roles", () => copyRoles(guild, target, state, save, report, skip));
  await step("emoji", () => copyEmoji(guild, source, target, state, save, report, skip, signal));
  await step("channels", () => copyChannels(guild, target, state, save, report, skip, signal));

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
    if (options.messagesPerChannel <= 0) return;
    const map: Mapping = {
      guildId: guild.id,
      serverId: target.serverId,
      roles: state.roles,
      roleNames: Object.fromEntries(guild.roles.map((r) => [r.id, r.name])),
      channels: state.channels,
      channelNames: Object.fromEntries(guild.channels.map((c) => [c.id, c.name])),
      emojis: { ...state.emojis },
    };
    const channels = displayOrder(guild.channels).filter(
      (c) => (c.kind === "text" || c.kind === "announcement") && state.channels[c.id],
    );
    report.historyTotal = channels.length;
    report.historyDone = channels.filter((c) => state.history[c.id]?.done).length;
    for (const c of channels) report.messages += state.history[c.id]?.messages ?? 0;
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
      try {
        for await (const message of source.messages(channel.id, options.messagesPerChannel, history.lastId)) {
          signal?.throwIfAborted();
          if (!message.system) {
            await copyMessage(
              message,
              hook,
              fuwaChannel,
              map,
              history,
              save,
              source,
              target,
              options,
              report,
              skip,
              channel.name,
            );
            history.messages++;
            report.messages++;
          }
          history.lastId = message.id;
          await save();
          progress();
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
        permissions: permissionsOf(role.permissions),
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
    await target.setEveryone(permissionsOf(everyone.permissions)).catch((err) => skip("@everyone's permissions", err));
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

async function copyChannels(
  guild: SourceGuild,
  target: Target,
  state: JobState,
  save: () => Promise<void>,
  report: Report,
  skip: Skip,
  signal?: AbortSignal,
): Promise<void> {
  const ordered = displayOrder(guild.channels);
  report.channels = ordered.filter((c) => state.channels[c.id]).length;
  if (state.channelsDone) return;
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
    if (channel.kind === "forum")
      skip(`#${channel.name}'s posts (forums come over as text channels, without their posts)`);
    try {
      state.channels[channel.id] = await target.createChannel({
        name: cleanName(channel.name, "channel"),
        kind: channel.kind === "forum" ? "text" : channel.kind,
        parentId: channel.parentId ? state.channels[channel.parentId] : undefined,
        topic: [...channel.topic].slice(0, 1024).join(""),
        slowmodeSeconds: Math.min(channel.slowmodeSeconds, 21_600),
        overwrites: overwritesOf(channel.overwrites, guild.id, target.serverId, state.roles),
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

async function copyMessage(
  message: SourceMessage,
  hook: Hook,
  channelId: string,
  map: Mapping,
  history: JobState["history"][string],
  save: () => Promise<void>,
  source: Source,
  target: Target,
  options: MigrateOptions,
  report: Report,
  skip: Skip,
  channelName: string,
): Promise<void> {
  const day = dayOf(message.at);
  if (day !== history.lastDay) {
    await target.post(hook, { content: dayDivider(message.at), username: IMPORTER_NAME, embeds: [] });
    history.lastDay = day;
    await save();
  }
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
  let text = messageText(message, map);
  if (left.length)
    text += `${text ? "\n" : ""}*(${left.length === 1 ? "a file" : `${left.length} files`} not copied: ${left.join(", ")})*`;
  const embeds = embedsOf(message.embeds);
  const pieces = chunks(text);
  if (!pieces.length && embeds.length) pieces.push("");
  const post = { username: cleanName(message.authorName, "someone", 80), avatarUrl: message.authorAvatarUrl };
  let firstId: string | undefined;
  try {
    for (const [i, piece] of pieces.entries()) {
      const id = await target.post(hook, { ...post, content: piece, embeds: i === pieces.length - 1 ? embeds : [] });
      firstId ??= id;
    }
  } catch (err) {
    skip(`A message by ${message.authorName} in #${channelName}`, err);
  }
  for (let i = 0; i < files.length; i += MAX_FILES) {
    try {
      const id = await target.sendFiles(channelId, `Files from **${post.username}**`, files.slice(i, i + MAX_FILES));
      firstId ??= id;
      report.files += Math.min(MAX_FILES, files.length - i);
    } catch (err) {
      skip(`Files from ${message.authorName} in #${channelName}`, err);
    }
  }
  if (message.pinned && firstId) {
    try {
      await target.pin(channelId, firstId);
      report.pins++;
    } catch (err) {
      skip(`A pin in #${channelName}`, err);
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
