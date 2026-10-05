import { createFuwa, type Fuwa, type FuwaOptions } from "./client.js";
import { Code, FuwaError, UnauthenticatedError, toFuwaError } from "./errors.js";
import { EventFollower, type EventKind, type EventPayload } from "./events.js";
import type { ServerHead } from "./gen/fuwa/v1/event_pb.js";
import type { MessageInitShape } from "@bufbuild/protobuf";
import type { Command, CommandSchema } from "./gen/fuwa/v1/command_pb.js";
import type { SendMessageRequestSchema } from "./gen/fuwa/v1/message_pb.js";
import {
  AccountKind,
  InteractionKind,
  MessageKind,
  type Event,
  type Interaction,
  type Message,
  type User,
} from "./gen/fuwa/v1/types_pb.js";
import type { Media } from "./gen/fuwa/v1/media_pb.js";
import { messages, type MessagePagesOptions, type MessageWithAuthor } from "./pages.js";
import { parseCommand, mentions, type ParsedCommand } from "./text.js";
import { uploadPicture, type UploadOptions } from "./upload.js";
import { VoiceConnection, type JoinVoiceOptions } from "./voice.js";

export interface AgentOptions extends Omit<FuwaOptions, "token"> {
  /**
   * The agent's token, from Settings > Agents (or AgentService.CreateAgent).
   * It's a secret: anyone with it can post as the agent. Keep it out of code
   * and logs; read it from the environment or a secret store.
   */
  token: string;
  /** What starts a command, such as "/dice". Default "/". Mentioning the agent first works too: "@helper dice". */
  prefix?: string;
  /**
   * Where to resume each server's events from, from `agent.cursors` saved
   * earlier. Servers without one start with new events only.
   */
  cursors?: Record<string, bigint> | Map<string, bigint>;
  /** Skip messages from other agents (no message, mention or command handlers). Default true, so agents can't loop. */
  ignoreAgents?: boolean;
  /** Skip messages posted through webhooks. Default true. */
  ignoreWebhooks?: boolean;
  /**
   * How often to look for servers the agent was added to or removed from,
   * in milliseconds, on instances without the `agent-streams` feature
   * (newer ones say so on the event stream, and nothing is polled).
   * Default 30 s; 0 never.
   */
  serverRefreshMs?: number;
  /**
   * Where errors go that no call returned to you: a handler that threw, a
   * lost connection, a failed refresh. Default: the "error" handlers, or
   * console.error with the error's name and message if there are none.
   */
  onError?: (error: unknown) => void;
}

/** What to send: text, or the whole request (attachments, embeds, a reply). */
export type Outgoing = string | Omit<MessageInitShape<typeof SendMessageRequestSchema>, "$typeName" | "serverId" | "channelId">;

/** A message someone sent, and ways to answer it. */
export interface MessageContext {
  agent: Agent;
  message: Message;
  serverId: string;
  channelId: string;
  content: string;
  /** It mentions the agent: <@id> (as the apps write it) or @username. */
  mentioned: boolean;
  /** Who wrote it (from the instance, cached). Undefined for webhook messages. */
  author(): Promise<User | undefined>;
  /** Sends a message in the same channel, as a reply to this one. */
  reply(content: Outgoing): Promise<Message>;
  /** Sends a message in the same channel. */
  send(content: Outgoing): Promise<Message>;
}

/** A command someone sent, such as "/dice 2d6". */
export interface CommandContext extends MessageContext {
  command: ParsedCommand;
  /** The words after the command. */
  args: string[];
  /** Everything after the command, as written. */
  rest: string;
}

/**
 * Someone ran one of the agent's slash commands or pressed a button on one of
 * its messages (CommandService).
 */
export interface InteractionContext {
  agent: Agent;
  interaction: Interaction;
  serverId: string;
  channelId: string;
  /** Who used it. */
  userId: string;
  kind: InteractionKind;
  /** A command: its name. */
  command: string;
  /**
   * A command: what was filled in, by option name, as text (integers as
   * digits, booleans as "true" or "false", members, channels and roles as
   * ids). The instance keeps these only while the interaction can be
   * answered, so keep what you need yourself.
   */
  options: Record<string, string>;
  /** A button: its custom_id, and the message it's on. */
  customId: string;
  messageId: string;
  /** Who used it (from the instance, cached). */
  user(): Promise<User | undefined>;
  /**
   * Answers it with a message in its channel that shows who used what. Up
   * to five times, within 15 minutes of it.
   */
  reply(content: Outgoing): Promise<Message>;
}

/** How long an interaction can be answered, and its arguments are kept. */
const INTERACTION_MS = 15 * 60_000;

export interface CommandInfo {
  name: string;
  description: string;
  aliases: string[];
}

type Handler<A extends unknown[]> = (...args: A) => unknown;

/** Everything `agent.on` takes, besides every event kind ("messageCreated", "memberJoined"...). */
export interface AgentHandlers {
  /** Signed in and following: `me` is the agent's own account. */
  ready: Handler<[{ me: User; servers: string[]; heads: ServerHead[] }]>;
  /** Someone else sent a message in a channel the agent can see. */
  message: Handler<[MessageContext]>;
  /** A message mentions the agent. Also comes as "message". */
  mention: Handler<[MessageContext]>;
  /** A command no handler is registered for. */
  unknownCommand: Handler<[CommandContext]>;
  /**
   * Someone ran one of the agent's slash commands (see `setCommands`) or
   * pressed one of its buttons, and it can still be answered. Ones older
   * than 15 minutes, caught up after a restart, come only as
   * "interactionCreated".
   */
  interaction: Handler<[InteractionContext]>;
  /** Every event, before the handlers for its kind. */
  event: Handler<[Event]>;
  /** Added to a server (noticed by the server refresh). */
  serverAdded: Handler<[string]>;
  /** Removed from a server, or it was deleted. */
  serverRemoved: Handler<[string]>;
  /** The event stream broke; it reconnects after `retryInMs` and catches up. */
  disconnected: Handler<[{ error: FuwaError; retryInMs: number }]>;
  /** Something went wrong that no call returned to you. */
  error: Handler<[unknown]>;
}

type KindHandlers = { [K in EventKind]: Handler<[EventPayload<K>, Event]> };
export type AgentEventName = keyof AgentHandlers | EventKind;
type HandlerFor<N extends AgentEventName> = N extends keyof AgentHandlers
  ? AgentHandlers[N]
  : N extends EventKind
    ? KindHandlers[N]
    : never;

const MAX_AUTHORS = 2000;

/**
 * An agent: an account a program drives. It signs in with its token,
 * follows every server it's in over one event stream (reconnecting and
 * catching up from the last event on its own), and hands your handlers
 * messages, mentions, commands and typed events.
 *
 *     const agent = new Agent({ url: "https://fuwa.chat", token: process.env.FUWA_TOKEN! });
 *     agent.command("ping", (ctx) => ctx.reply("pong"));
 *     await agent.start();
 */
export class Agent {
  /** Typed clients for every service, signed in as the agent. */
  readonly api: Fuwa;
  readonly prefix: string;
  #me: User | undefined;
  #opts: AgentOptions;
  #handlers = new Map<string, Set<Handler<never[]>>>();
  #commands = new Map<string, { info: CommandInfo; run: Handler<[CommandContext]> }>();
  #authors = new Map<string, Promise<User | undefined>>();
  #chains = new Map<string, Promise<void>>();
  #follower: EventFollower | undefined;
  #followsNew = false;
  #stop: AbortController | undefined;
  #closed: Promise<void> | undefined;
  #voices = new Set<VoiceConnection>();

  constructor(options: AgentOptions) {
    if (!options.token) throw new TypeError("an agent needs its token");
    this.#opts = options;
    this.prefix = options.prefix ?? "/";
    this.api = createFuwa(options);
  }

  /** The agent's own account, once started. */
  get me(): User {
    if (!this.#me) throw new Error("the agent hasn't started yet");
    return this.#me;
  }

  /**
   * The last event seen in each server. Save it (bigints: store as strings)
   * and pass it back as `cursors` to catch up on what happened while the
   * program was stopped.
   */
  get cursors(): ReadonlyMap<string, bigint> {
    return this.#follower?.cursors ?? new Map(Object.entries(this.#opts.cursors ?? {}));
  }

  /** The servers being followed. */
  get servers(): ReadonlySet<string> {
    return this.#follower?.servers ?? new Set();
  }

  /** The registered commands, for a help message. */
  get commands(): CommandInfo[] {
    const seen = new Set<CommandInfo>();
    for (const { info } of this.#commands.values()) seen.add(info);
    return [...seen];
  }

  /** Adds a handler. Returns a function that removes it. */
  on<N extends AgentEventName>(name: N, handler: HandlerFor<N>): () => void {
    let set = this.#handlers.get(name);
    if (!set) this.#handlers.set(name, (set = new Set()));
    set.add(handler as Handler<never[]>);
    return () => set.delete(handler as Handler<never[]>);
  }

  /**
   * Answers a command: "/name args" (with the agent's prefix) or "@agent name
   * args". Names are matched without regard to case.
   */
  command(
    name: string,
    run: Handler<[CommandContext]>,
    options: { description?: string; aliases?: string[] } = {},
  ): this {
    const info: CommandInfo = { name: name.toLowerCase(), description: options.description ?? "", aliases: options.aliases ?? [] };
    for (const n of [info.name, ...info.aliases.map((a) => a.toLowerCase())]) {
      if (this.#commands.has(n)) throw new Error(`the command "${n}" is already registered`);
      this.#commands.set(n, { info, run });
    }
    return this;
  }

  /**
   * Signs in, loads the servers the agent is in and starts following them.
   * Resolves once the event stream is live. Fails if the token doesn't work.
   */
  async start(): Promise<void> {
    if (this.#stop) throw new Error("the agent is already running");
    const stop = new AbortController();
    this.#stop = stop;
    try {
      const me = (await this.api.auth.getMe({}, { signal: stop.signal })).user;
      if (!me) throw new UnauthenticatedError(Code.Unauthenticated, "the token didn't sign in");
      this.#me = me;
      const [{ servers }, followNewServers] = await Promise.all([
        this.api.servers.listServers({}, { signal: stop.signal }),
        this.#instanceHas("agent-streams", stop.signal),
      ]);
      this.#followsNew = followNewServers;
      const follower = new EventFollower(this.api, {
        servers: servers.map((s) => s.id),
        cursors: this.#opts.cursors,
        signal: stop.signal,
        followNewServers,
      });
      this.#follower = follower;
      let live!: (heads: ServerHead[]) => void;
      let failed!: (err: unknown) => void;
      const ready = new Promise<ServerHead[]>((resolve, reject) => {
        live = resolve;
        failed = reject;
      });
      if (servers.length === 0) live([]);
      this.#closed = this.#run(follower, stop.signal, live).then(
        () => this.#settle(),
        (err) => {
          failed(err);
          this.#report(err);
          return this.#settle().then(() => Promise.reject(err));
        },
      );
      this.#closed.catch(() => {});
      const heads = await ready;
      this.#emit("ready", { me, servers: [...follower.servers], heads });
    } catch (err) {
      stop.abort();
      this.#stop = undefined;
      throw toFuwaError(err);
    }
  }

  /**
   * Resolves when the agent stops: after `stop()`, or (rejecting) when it
   * can't go on, such as its token being reset.
   */
  get closed(): Promise<void> {
    return this.#closed ?? Promise.resolve();
  }

  /** Stops following and waits for running handlers to finish. */
  async stop(): Promise<void> {
    await Promise.all([...this.#voices].map((v) => v.leave()));
    this.#stop?.abort();
    await this.#closed?.catch(() => {});
  }

  /** Sends a message. Rate limits and slow mode are waited out (up to a minute). */
  async send(serverId: string, channelId: string, content: Outgoing): Promise<Message> {
    const req = typeof content === "string" ? { content } : content;
    const { message } = await this.api.messages.sendMessage({ ...req, serverId, channelId });
    if (!message) throw toFuwaError(new Error("the instance didn't return the message"));
    return message;
  }

  /**
   * Replaces the agent's slash commands in a server: what members see when
   * they type "/". Runs come to `on("interaction")`.
   */
  async setCommands(serverId: string, commands: MessageInitShape<typeof CommandSchema>[]): Promise<Command[]> {
    const res = await this.api.commands.setCommands({ serverId, commands });
    return res.commands;
  }

  /** Answers a message in its channel. */
  reply(to: Message, content: Outgoing): Promise<Message> {
    const req = typeof content === "string" ? { content } : content;
    return this.send(to.serverId, to.channelId, { ...req, replyToId: to.id });
  }

  /** Changes the text of one of the agent's own messages. */
  async edit(message: Message, content: string): Promise<Message> {
    const res = await this.api.messages.updateMessage({
      serverId: message.serverId,
      channelId: message.channelId,
      messageId: message.id,
      content,
    });
    if (!res.message) throw toFuwaError(new Error("the instance didn't return the message"));
    return res.message;
  }

  /** Deletes a message: the agent's own, or any with Manage Messages. */
  async delete(message: Message): Promise<void> {
    await this.api.messages.deleteMessage({
      serverId: message.serverId,
      channelId: message.channelId,
      messageId: message.id,
    });
  }

  /** Uploads a picture to the instance (see uploadPicture). */
  upload(options: UploadOptions): Promise<Media> {
    return uploadPicture(this.api, options);
  }

  /**
   * Joins a voice channel to hear and talk (see VoiceConnection): no WebRTC,
   * only the instance. Stopping the agent leaves it too.
   */
  async joinVoice(serverId: string, channelId: string, options: Omit<JoinVoiceOptions, "serverId" | "channelId"> = {}): Promise<VoiceConnection> {
    const voice = await VoiceConnection.join(this.api, { ...options, serverId, channelId });
    this.#voices.add(voice);
    voice.closed.catch(() => {}).finally(() => this.#voices.delete(voice));
    return voice;
  }

  /** A channel's messages, newest first by default (see `messages`). */
  history(options: MessagePagesOptions): AsyncGenerator<MessageWithAuthor> {
    return messages(this.api, options);
  }

  /** Someone's profile, cached while the agent runs. */
  user(userId: string): Promise<User | undefined> {
    let found = this.#authors.get(userId);
    if (!found) {
      found = this.api.auth.getProfile({ userId }).then(
        (r) => r.profile?.user,
        (err) => {
          this.#authors.delete(userId);
          if (err instanceof FuwaError && err.code === Code.NotFound) return undefined;
          throw err;
        },
      );
      if (this.#authors.size >= MAX_AUTHORS) this.#authors.delete(this.#authors.keys().next().value!);
      this.#authors.set(userId, found);
    }
    return found;
  }

  async #run(follower: EventFollower, signal: AbortSignal, live: (heads: ServerHead[]) => void): Promise<void> {
    // An instance that announces new servers on the stream needs no polling.
    const refresh = this.#followsNew ? 0 : (this.#opts.serverRefreshMs ?? 30_000);
    const timer = refresh > 0 ? setInterval(() => void this.#refreshServers(follower, signal), refresh) : undefined;
    try {
      for await (const update of follower) {
        if (update.type === "ready") live(update.heads);
        else if (update.type === "followed") this.#emit("serverAdded", update.head.serverId);
        else if (update.type === "disconnected") this.#emit("disconnected", update);
        else this.#dispatch(update.event, follower);
      }
    } finally {
      clearInterval(timer);
    }
  }

  /** Whether the instance lists a feature; an unanswered question counts as no. */
  async #instanceHas(id: string, signal: AbortSignal): Promise<boolean> {
    try {
      const { node } = await this.api.node.getNode({}, { signal });
      return node?.versions?.features.some((f) => f.id === id) ?? false;
    } catch (err) {
      if (signal.aborted) throw err;
      return false;
    }
  }

  async #refreshServers(follower: EventFollower, signal: AbortSignal): Promise<void> {
    try {
      const { servers } = await this.api.servers.listServers({}, { signal });
      const ids = servers.map((s) => s.id);
      const before = new Set(follower.servers);
      follower.setServers(ids);
      for (const id of ids) if (!before.has(id)) this.#emit("serverAdded", id);
      for (const id of before) if (!ids.includes(id)) this.#emit("serverRemoved", id);
    } catch (err) {
      if (!signal.aborted) this.#report(err);
    }
  }

  #dispatch(event: Event, follower: EventFollower): void {
    const payload = event.payload;
    let key = event.serverId;
    if (payload.case === "messageCreated" || payload.case === "messageUpdated") {
      key += `/${payload.value.message?.channelId ?? ""}`;
    }
    if (payload.case === "userUpdated" && payload.value.user) {
      this.#authors.set(payload.value.user.id, Promise.resolve(payload.value.user));
    }
    const gone =
      payload.case === "serverDeleted" ||
      (payload.case === "memberLeft" && payload.value.userId === this.#me?.id);
    if (gone) {
      follower.setServers([...follower.servers].filter((id) => id !== event.serverId));
    }
    // One channel's messages are handled in order; channels run side by side.
    const prev = this.#chains.get(key) ?? Promise.resolve();
    const next = prev
      .then(() => this.#handle(event))
      .catch((err) => this.#report(err))
      .finally(() => {
        if (this.#chains.get(key) === next) this.#chains.delete(key);
      });
    this.#chains.set(key, next);
    if (gone) this.#emit("serverRemoved", event.serverId);
  }

  async #handle(event: Event): Promise<void> {
    await this.#call("event", event);
    if (event.payload.case) await this.#call(event.payload.case, event.payload.value, event);
    if (event.payload.case === "interactionCreated") {
      const interaction = event.payload.value.interaction;
      if (interaction && interaction.agentId === this.#me!.id && answerable(interaction)) {
        await this.#call("interaction", this.#interactionContext(interaction));
      }
      return;
    }
    if (event.payload.case !== "messageCreated") return;
    const message = event.payload.value.message;
    if (!message || message.kind !== MessageKind.UNSPECIFIED) return;
    const me = this.#me!;
    if (message.authorId === me.id) return;
    if (message.webhook && (this.#opts.ignoreWebhooks ?? true)) return;
    if (this.#opts.ignoreAgents ?? true) {
      const author = message.shared?.user ?? (message.webhook ? undefined : await this.user(message.authorId));
      if (author?.kind === AccountKind.AGENT) return;
    }
    const ctx = this.#context(message);
    await this.#call("message", ctx);
    if (ctx.mentioned) await this.#call("mention", ctx);
    const command = parseCommand(message.content, { prefix: this.prefix, username: me.username });
    if (!command) return;
    const cctx: CommandContext = { ...ctx, command, args: command.args, rest: command.rest };
    const found = this.#commands.get(command.name);
    if (found) await found.run(cctx);
    else await this.#call("unknownCommand", cctx);
  }

  #context(message: Message): MessageContext {
    return {
      agent: this,
      message,
      serverId: message.serverId,
      channelId: message.channelId,
      content: message.content,
      mentioned: message.mentionUserIds.includes(this.#me!.id) || mentions(message.content, this.#me!.username),
      author: () => (message.webhook ? Promise.resolve(undefined) : message.shared?.user ? Promise.resolve(message.shared.user) : this.user(message.authorId)),
      reply: (content) => this.reply(message, content),
      send: (content) => this.send(message.serverId, message.channelId, content),
    };
  }

  #interactionContext(interaction: Interaction): InteractionContext {
    return {
      agent: this,
      interaction,
      serverId: interaction.serverId,
      channelId: interaction.channelId,
      userId: interaction.userId,
      kind: interaction.kind,
      command: interaction.command,
      options: Object.fromEntries(interaction.arguments.map((a) => [a.name, a.value])),
      customId: interaction.customId,
      messageId: interaction.messageId,
      user: () => this.user(interaction.userId),
      reply: (content) => {
        const req = typeof content === "string" ? { content } : content;
        return this.send(interaction.serverId, interaction.channelId, { ...req, interactionId: interaction.id });
      },
    };
  }

  async #call(name: string, ...args: unknown[]): Promise<void> {
    const set = this.#handlers.get(name);
    if (!set) return;
    for (const handler of [...set]) {
      try {
        await (handler as Handler<unknown[]>)(...args);
      } catch (err) {
        this.#report(err);
      }
    }
  }

  #emit(name: string, ...args: unknown[]): void {
    void this.#call(name, ...args);
  }

  #report(err: unknown): void {
    if (this.#opts.onError) return this.#opts.onError(err);
    const set = this.#handlers.get("error");
    if (set?.size) {
      for (const handler of set) {
        try {
          void Promise.resolve((handler as Handler<unknown[]>)(err)).catch(() => {});
        } catch {
          // An error handler that throws has nowhere left to go.
        }
      }
      return;
    }
    // Name and message only: a cause may hold addresses.
    const e = err instanceof Error ? err : new Error(String(err));
    console.error(`[fuwa agent] ${e.name}: ${e.message}`);
  }

  async #settle(): Promise<void> {
    await Promise.allSettled([...this.#chains.values()]);
    this.#stop = undefined;
  }
}

/** Whether an interaction is recent enough to answer (and still has its arguments). */
function answerable(interaction: Interaction): boolean {
  const at = interaction.createdAt;
  if (!at) return true;
  const ms = Number(at.seconds) * 1000 + Math.floor(at.nanos / 1e6);
  return Date.now() - ms < INTERACTION_MS;
}
