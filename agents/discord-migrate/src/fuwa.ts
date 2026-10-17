// fuwa's side of a migration: one server, through the agent's own calls
// (SDK), and the server's webhooks over plain HTTP for the history.
import {
  type Agent,
  AlreadyExistsError,
  ChannelType,
  FailedPreconditionError,
  InvalidArgumentError,
  MediaPurpose,
  NotFoundError,
  OverwriteTarget,
  Permission,
  PermissionDeniedError,
  RateLimitedError,
} from "@waifu-devs/fuwa";
import {
  type Download,
  type Hook,
  Refused,
  type SourceEmbed,
  type Target,
  type TargetFile,
  type TargetOverwrite,
  type TargetPost,
} from "./model.ts";
import { type SoundboardClient, hasSoundboard, soundboard, toOggOpus } from "./soundboard.ts";

/** An upload for a soundboard sound (MEDIA_PURPOSE_SOUND), which the SDK on npm doesn't name yet. */
const SOUND = 11 as MediaPurpose;

/** A webhook may post 30 messages a minute: one every two seconds keeps under it. */
const POST_EVERY_MS = 2_000;
const TRIES = 6;

const KINDS = {
  text: ChannelType.TEXT,
  announcement: ChannelType.ANNOUNCEMENT,
  voice: ChannelType.VOICE,
  category: ChannelType.CATEGORY,
} as const;

/**
 * What fuwa turned down for good becomes `Refused`, so the migration skips
 * just that; anything else (the instance away, the agent's token reset) goes
 * on as it is and stops the run.
 */
function refusal(err: unknown): unknown {
  const forGood =
    err instanceof InvalidArgumentError ||
    err instanceof FailedPreconditionError ||
    err instanceof PermissionDeniedError ||
    err instanceof AlreadyExistsError ||
    err instanceof NotFoundError ||
    // A limit (emoji, roles, storage), rather than going too fast.
    (err instanceof RateLimitedError && err.retryAfterMs === undefined);
  return forGood ? new Refused((err as Error).message, { cause: err }) : err;
}

async function refusing<T>(work: () => Promise<T>): Promise<T> {
  try {
    return await work();
  } catch (err) {
    throw refusal(err);
  }
}

export class FuwaTarget implements Target {
  readonly serverId: string;
  readonly #agent: Agent;
  readonly #tokens = new Map<string, string>();
  readonly #lastPost = new Map<string, number>();
  readonly #sleep: (ms: number) => Promise<void>;
  readonly #sounds: SoundboardClient;

  constructor(agent: Agent, serverId: string, sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms))) {
    this.#agent = agent;
    this.serverId = serverId;
    this.#sleep = sleep;
    this.#sounds = soundboard(agent.api.transport);
  }

  get #api() {
    return this.#agent.api;
  }

  async missing(): Promise<string[]> {
    const serverId = this.serverId;
    const [{ members }, { roles }] = await Promise.all([
      this.#api.servers.listMembers({ serverId }),
      this.#api.roles.listRoles({ serverId }),
    ]);
    const me = members.find((m) => m.user?.id === this.#agent.me.id);
    const mine = new Set([serverId, ...(me?.roleIds ?? [])]);
    const admin = roles.some((r) => mine.has(r.id) && r.permissions.includes(Permission.ADMINISTRATOR));
    if (admin) return [];
    return [
      `Give @${this.#agent.me.username} a role with Administrator in this server, at the top of the role list, so it can make roles, channels and emoji, then run the command again.`,
    ];
  }

  updateServer(change: { name: string; description: string; icon?: Download; banner?: Download }): Promise<void> {
    return refusing(async () => {
      const serverId = this.serverId;
      const upload = async (purpose: MediaPurpose, picture?: Download) =>
        picture
          ? (await this.#agent.upload({ purpose, data: picture.bytes, contentType: picture.contentType, serverId })).url
          : undefined;
      const iconUrl = await upload(MediaPurpose.SERVER_ICON, change.icon);
      const bannerUrl = await upload(MediaPurpose.BANNER, change.banner);
      await this.#api.servers.updateServer({
        serverId,
        name: change.name,
        description: [...change.description].slice(0, 1000).join(""),
        iconUrl,
        bannerUrl,
      });
    });
  }

  createRole(role: {
    name: string;
    color?: number;
    permissions: number[];
    hoist: boolean;
    mentionable: boolean;
  }): Promise<string> {
    return refusing(async () => {
      const { role: made } = await this.#api.roles.createRole({ serverId: this.serverId, ...role });
      return made!.id;
    });
  }

  setEveryone(permissions: number[]): Promise<void> {
    return refusing(async () => {
      await this.#api.roles.updateRole({
        serverId: this.serverId,
        roleId: this.serverId,
        permissions: { permissions },
      });
    });
  }

  orderRoles(roleIds: string[]): Promise<void> {
    return refusing(async () => {
      const serverId = this.serverId;
      const [{ roles }, { members }] = await Promise.all([
        this.#api.roles.listRoles({ serverId }),
        this.#api.servers.listMembers({ serverId }),
      ]);
      const me = members.find((m) => m.user?.id === this.#agent.me.id);
      const myTop = Math.max(0, ...roles.filter((r) => me?.roleIds.includes(r.id)).map((r) => r.position));
      const current = roles.filter((r) => r.id !== serverId).sort((a, b) => b.position - a.position);
      // The agent can't move roles at or above its own: they stay on top.
      const fixed = current.filter((r) => r.position >= myTop).map((r) => r.id);
      const movable = new Set(current.filter((r) => r.position < myTop).map((r) => r.id));
      const wanted = roleIds.filter((id) => movable.has(id));
      const others = [...movable].filter((id) => !wanted.includes(id));
      await this.#api.roles.reorderRoles({ serverId, roleIds: [...fixed, ...wanted, ...others] });
    });
  }

  createChannel(channel: {
    name: string;
    kind: "text" | "announcement" | "voice" | "category";
    parentId?: string;
    topic: string;
    slowmodeSeconds: number;
    overwrites: TargetOverwrite[];
  }): Promise<string> {
    return refusing(async () => {
      const type = KINDS[channel.kind];
      const { channel: made } = await this.#api.channels.createChannel({
        serverId: this.serverId,
        name: channel.name,
        type,
        parentId: channel.parentId ?? "",
        topic: type === ChannelType.CATEGORY ? "" : channel.topic,
        slowmodeSeconds: type === ChannelType.TEXT || type === ChannelType.ANNOUNCEMENT ? channel.slowmodeSeconds : 0,
        permissionOverwrites: channel.overwrites.map((o) => ({
          targetId: o.targetId,
          target: o.kind === "role" ? OverwriteTarget.ROLE : OverwriteTarget.MEMBER,
          allow: o.allow,
          deny: o.deny,
        })),
      });
      return made!.id;
    });
  }

  orderChannels(placed: { id: string; parentId?: string }[]): Promise<void> {
    return refusing(async () => {
      const serverId = this.serverId;
      const { channels } = await this.#api.channels.listChannels({ serverId });
      const ours = new Set(placed.map((p) => p.id));
      const rest = channels
        .filter((c) => !ours.has(c.id) && c.type !== ChannelType.THREAD)
        .map((c) => ({ channelId: c.id, parentId: c.parentId }));
      await this.#api.channels.reorderChannels({
        serverId,
        channels: [...placed.map((p) => ({ channelId: p.id, parentId: p.parentId ?? "" })), ...rest],
      });
    });
  }

  createEmoji(name: string, picture: Download): Promise<{ id: string; name: string }> {
    return refusing(async () => {
      const serverId = this.serverId;
      const media = await this.#agent.upload({
        purpose: MediaPurpose.EMOJI,
        data: picture.bytes,
        contentType: picture.contentType,
        serverId,
      });
      const { emoji } = await this.#api.emojis.createEmoji({ serverId, name, url: media.url });
      return { id: emoji!.id, name: emoji!.name };
    });
  }

  createHook(channelId: string, name: string): Promise<Hook> {
    return refusing(async () => {
      const { webhook } = await this.#api.webhooks.createWebhook({ serverId: this.serverId, channelId, name });
      this.#tokens.set(webhook!.id, webhook!.token);
      return { id: webhook!.id, channelId };
    });
  }

  async deleteHook(hook: Hook): Promise<void> {
    this.#tokens.delete(hook.id);
    this.#lastPost.delete(hook.id);
    await this.#api.webhooks.deleteWebhook({ serverId: this.serverId, webhookId: hook.id });
  }

  async post(hook: Hook, post: TargetPost): Promise<string> {
    const token = this.#tokens.get(hook.id);
    if (!token) throw new Refused("that webhook is gone");
    const url = `${this.#api.url}/webhooks/${this.serverId}/${hook.id}/${token}?wait=true`;
    const body = JSON.stringify({
      content: post.content,
      username: post.username,
      avatar_url: post.avatarUrl ?? "",
      embeds: post.embeds.map(webhookEmbed),
    });
    for (let attempt = 1; ; attempt++) {
      const wait = (this.#lastPost.get(hook.id) ?? 0) + POST_EVERY_MS - Date.now();
      if (wait > 0) await this.#sleep(wait);
      this.#lastPost.set(hook.id, Date.now());
      let res: Response;
      try {
        res = await this.#api.fetch(url, { method: "POST", body, headers: { "content-type": "application/json" } });
      } catch (err) {
        if (attempt >= TRIES) throw err;
        await this.#sleep(1000 * 2 ** attempt);
        continue;
      }
      if (res.ok) return ((await res.json()) as { id: string }).id;
      const message = await res
        .json()
        .then((j) => (j as { message?: string }).message ?? "")
        .catch(() => "");
      if (res.status === 429 || res.status >= 500) {
        if (attempt >= TRIES) throw new Error(message || `the webhook answered ${res.status}`);
        const after = Number(res.headers.get("retry-after"));
        await this.#sleep(Number.isFinite(after) && after > 0 ? after * 1000 : 1000 * 2 ** attempt);
        continue;
      }
      throw new Refused(message || `the webhook answered ${res.status}`);
    }
  }

  sendFiles(channelId: string, content: string, files: TargetFile[]): Promise<string> {
    return refusing(async () => {
      const attachments = await this.#attach(files);
      const message = await this.#agent.send(this.serverId, channelId, { content, attachments });
      return message.id;
    });
  }

  /** Uploads files to attach to a message. */
  async #attach(files: TargetFile[]) {
    const attachments = [];
    for (const file of files) {
      const media = await this.#agent.upload({
        purpose: MediaPurpose.ATTACHMENT,
        data: file.bytes,
        contentType: file.contentType || "application/octet-stream",
        serverId: this.serverId,
      });
      attachments.push({
        url: media.url,
        filename: [...file.name].slice(0, 255).join("") || "file",
        width: file.width ?? 0,
        height: file.height ?? 0,
        spoiler: file.spoiler,
      });
    }
    return attachments;
  }

  reply(channelId: string, threadId: string, content: string, files: TargetFile[]): Promise<string> {
    return refusing(async () => {
      const attachments = await this.#attach(files);
      const message = await this.#agent.send(this.serverId, channelId, { content, attachments, threadId });
      return message.id;
    });
  }

  soundboard(): Promise<boolean> {
    return hasSoundboard(this.#sounds, this.serverId);
  }

  async createSound(sound: { name: string; emoji: string; volume: number; file: Download }): Promise<void> {
    let ogg;
    try {
      ogg = await toOggOpus(sound.file.bytes);
    } catch (err) {
      throw new Refused((err as Error).message);
    }
    await refusing(async () => {
      const serverId = this.serverId;
      const media = await this.#agent.upload({ purpose: SOUND, data: ogg.bytes, contentType: "audio/ogg", serverId });
      await this.#sounds.createSound({
        serverId,
        name: sound.name,
        url: media.url,
        emoji: sound.emoji,
        volume: Math.min(1, Math.max(0, sound.volume)),
        durationMs: ogg.durationMs,
      });
    });
  }

  pin(channelId: string, messageId: string): Promise<void> {
    return refusing(async () => {
      await this.#api.messages.pinMessage({ serverId: this.serverId, channelId, messageId, pinned: true });
    });
  }

  createInvite(): Promise<string> {
    return refusing(async () => {
      const { invite } = await this.#api.invites.createInvite({
        serverId: this.serverId,
        maxUses: 0,
        maxAgeSeconds: 0,
      });
      const { node } = await this.#api.node.getNode({});
      const base = (node?.publicUrl || this.#api.url).replace(/\/$/, "");
      return `${base}/invite/${invite!.code}`;
    });
  }
}

/** An embed as a webhook post takes it, shaped like Discord's. */
function webhookEmbed(e: SourceEmbed) {
  return {
    title: e.title ?? "",
    description: e.description ?? "",
    url: e.url ?? "",
    color: e.color ?? 0,
    fields: e.fields,
    thumbnail: e.thumbnailUrl ? { url: e.thumbnailUrl } : undefined,
    image: e.imageUrl ? { url: e.imageUrl } : undefined,
  };
}
