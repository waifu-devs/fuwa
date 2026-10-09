import type { DescService } from "@bufbuild/protobuf";
import { createClient, type Client, type Interceptor, type Transport } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { AccountService } from "./gen/fuwa/v1/account_pb.js";
import { AdminService } from "./gen/fuwa/v1/admin_pb.js";
import { AgentService } from "./gen/fuwa/v1/agent_pb.js";
import { AuthService } from "./gen/fuwa/v1/auth_pb.js";
import { AutoModService } from "./gen/fuwa/v1/automod_pb.js";
import { CallService } from "./gen/fuwa/v1/call_pb.js";
import { ChannelService, SharedChannelService } from "./gen/fuwa/v1/channel_pb.js";
import { CommandService } from "./gen/fuwa/v1/command_pb.js";
import { LiveTileService } from "./gen/fuwa/v1/live_tile_pb.js";
import { DirectMessageService } from "./gen/fuwa/v1/dm_pb.js";
import { EmojiService } from "./gen/fuwa/v1/emoji_pb.js";
import { EventService } from "./gen/fuwa/v1/event_pb.js";
import { FriendService } from "./gen/fuwa/v1/friend_pb.js";
import { GifService } from "./gen/fuwa/v1/gif_pb.js";
import { InviteService } from "./gen/fuwa/v1/invite_pb.js";
import { JoinService } from "./gen/fuwa/v1/join_pb.js";
import { MediaService } from "./gen/fuwa/v1/media_pb.js";
import { MessageService } from "./gen/fuwa/v1/message_pb.js";
import { NodeService } from "./gen/fuwa/v1/node_pb.js";
import { LiveService } from "./gen/fuwa/v1/live_pb.js";
import { PresenceService } from "./gen/fuwa/v1/presence_pb.js";
import { RoleService } from "./gen/fuwa/v1/role_pb.js";
import { SearchService } from "./gen/fuwa/v1/search_pb.js";
import { SecureChannelService } from "./gen/fuwa/v1/secure_pb.js";
import { ServerService } from "./gen/fuwa/v1/server_pb.js";
import { SsoService } from "./gen/fuwa/v1/sso_pb.js";
import type { Build, Node } from "./gen/fuwa/v1/types_pb.js";
import { WebhookService } from "./gen/fuwa/v1/webhook_pb.js";
import { CanceledError, Code, TimeoutError, toFuwaError } from "./errors.js";
import { DEFAULT_RETRY, retryDelay, sleep, type RetryOptions } from "./retry.js";

export interface FuwaOptions {
  /** The instance's address, such as "https://fuwa.chat". */
  url: string;
  /**
   * The bearer token: an agent's token, or a person's session token. A
   * function is read on every call, so a token can change without a new
   * client. Tokens are secrets: the SDK sends them only to `url`, in the
   * `authorization` header, and never logs them.
   */
  token?: string | (() => string | null | undefined);
  /** Retries for calls that fail in ways worth trying again (see RetryOptions). */
  retry?: RetryOptions | false;
  /**
   * A deadline for each try of a call that isn't a stream, in milliseconds.
   * Default 30 s; 0 for none. A call's own `timeoutMs` bounds all its tries.
   */
  timeoutMs?: number;
  /** A fetch to use instead of the global one (tests, proxies, older runtimes). */
  fetch?: typeof globalThis.fetch;
  /** More interceptors, run after the SDK's own (auth, retries, typed errors). */
  interceptors?: Interceptor[];
  /**
   * Allow plain http to an address other than this computer. Off by default:
   * the token would cross the network unencrypted.
   */
  allowInsecure?: boolean;
}

/** Typed clients for every fuwa service on one instance. */
export interface Fuwa {
  /** The instance's address, without a trailing slash. */
  readonly url: string;
  /** The gRPC-Web transport under the clients, for services added later. */
  readonly transport: Transport;
  node: Client<typeof NodeService>;
  auth: Client<typeof AuthService>;
  account: Client<typeof AccountService>;
  servers: Client<typeof ServerService>;
  channels: Client<typeof ChannelService>;
  messages: Client<typeof MessageService>;
  events: Client<typeof EventService>;
  admin: Client<typeof AdminService>;
  media: Client<typeof MediaService>;
  roles: Client<typeof RoleService>;
  invites: Client<typeof InviteService>;
  join: Client<typeof JoinService>;
  dms: Client<typeof DirectMessageService>;
  /** Friends, requests and blocks; people only (agents can't have friends). */
  friends: Client<typeof FriendService>;
  secure: Client<typeof SecureChannelService>;
  automod: Client<typeof AutoModService>;
  emojis: Client<typeof EmojiService>;
  calls: Client<typeof CallService>;
  webhooks: Client<typeof WebhookService>;
  agents: Client<typeof AgentService>;
  sso: Client<typeof SsoService>;
  shared: Client<typeof SharedChannelService>;
  /** Slash commands and buttons: agents set theirs, members run and press them. */
  commands: Client<typeof CommandService>;
  /** Live tiles: agents keep cards above a server's channel list up to date (`agent.liveTile`). */
  liveTiles: Client<typeof LiveTileService>;
  gifs: Client<typeof GifService>;
  presence: Client<typeof PresenceService>;
  /**
   * One stream for everything (instances with the `live-connection`
   * feature): agents can ask for only the messages that mention them
   * (`messages: MessageIntent.MENTIONS`) and get channel heads for the rest.
   */
  live: Client<typeof LiveService>;
  search: Client<typeof SearchService>;
  /** What the instance says it runs: its version and build, from NodeService.GetNode. */
  serverVersion(): Promise<ServerVersion>;
  /** The fetch the client uses, for uploads. */
  readonly fetch: typeof globalThis.fetch;
}

export interface ServerVersion {
  /** Such as "0.4.2". */
  version: string;
  /** The commit and source it says it was built from. A claim, not proof. */
  build: Build | undefined;
  node: Node;
}

const LOOPBACK = /^(localhost|127(\.\d{1,3}){3}|\[::1\])$/i;

/** Checks and tidies an instance address. */
export function instanceUrl(raw: string, allowInsecure = false): string {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw new TypeError("the instance URL isn't a valid URL");
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") {
    throw new TypeError("the instance URL must start with https://");
  }
  if (url.protocol === "http:" && !allowInsecure && !LOOPBACK.test(url.hostname)) {
    throw new TypeError(
      "the instance URL must use https:// (plain http is only for this computer, or pass allowInsecure)",
    );
  }
  if (url.username || url.password) throw new TypeError("the instance URL can't hold a username or password");
  return `${url.origin}${url.pathname.replace(/\/+$/, "")}`;
}

/** Creates typed clients for the instance at `options.url`, over gRPC-Web (browsers and Node 20+). */
export function createFuwa(options: FuwaOptions): Fuwa {
  const url = instanceUrl(options.url, options.allowInsecure);
  const token = typeof options.token === "function" ? options.token : () => options.token as string | undefined;
  const retry = options.retry === false ? undefined : { ...DEFAULT_RETRY, ...options.retry };
  const timeoutMs = options.timeoutMs ?? 30_000;
  const fetchFn = options.fetch ?? globalThis.fetch.bind(globalThis);

  const auth: Interceptor = (next) => (req) => {
    const t = token();
    if (t) req.header.set("authorization", `Bearer ${t}`);
    return next(req);
  };

  // Typed errors for every call, and for those that aren't streams a deadline
  // per try and retries.
  const errors: Interceptor = (next) => async (req) => {
    const method = `${req.service.typeName}/${req.method.name}`;
    for (let attempt = 0; ; attempt++) {
      const deadline = !req.stream && timeoutMs > 0 ? AbortSignal.timeout(timeoutMs) : undefined;
      try {
        const res = await next(deadline ? { ...req, signal: AbortSignal.any([req.signal, deadline]) } : req);
        if (!res.stream) return res;
        // A stream's failures come while it's read: give them the same types.
        const messages = res.message;
        return {
          ...res,
          message: (async function* () {
            try {
              yield* messages;
            } catch (cause) {
              throw toFuwaError(cause, method);
            }
          })(),
        };
      } catch (cause) {
        if (req.signal.aborted) throw new CanceledError(Code.Canceled, "the call was cancelled", { cause, method });
        const err = deadline?.aborted
          ? new TimeoutError(Code.DeadlineExceeded, `no answer within ${timeoutMs} ms`, { cause, method, network: true })
          : toFuwaError(cause, method);
        const wait = retry && !req.stream ? retryDelay(err, req.method.name, attempt, retry) : undefined;
        if (wait === undefined) throw err;
        await sleep(wait, req.signal);
      }
    }
  };

  const transport = createGrpcWebTransport({
    baseUrl: url,
    fetch: fetchFn,
    interceptors: [errors, auth, ...(options.interceptors ?? [])],
  });

  const client = <S extends DescService>(service: S): Client<S> => typed(createClient(service, transport), service.typeName);
  const node = client(NodeService);
  return {
    url,
    transport,
    fetch: fetchFn,
    node,
    auth: client(AuthService),
    account: client(AccountService),
    servers: client(ServerService),
    channels: client(ChannelService),
    messages: client(MessageService),
    events: client(EventService),
    admin: client(AdminService),
    media: client(MediaService),
    roles: client(RoleService),
    invites: client(InviteService),
    join: client(JoinService),
    dms: client(DirectMessageService),
    friends: client(FriendService),
    secure: client(SecureChannelService),
    automod: client(AutoModService),
    emojis: client(EmojiService),
    calls: client(CallService),
    webhooks: client(WebhookService),
    agents: client(AgentService),
    sso: client(SsoService),
    shared: client(SharedChannelService),
    commands: client(CommandService),
    liveTiles: client(LiveTileService),
    gifs: client(GifService),
    presence: client(PresenceService),
    live: client(LiveService),
    search: client(SearchService),
    async serverVersion() {
      const { node: n } = await node.getNode({});
      if (!n) throw toFuwaError(new Error("the instance didn't describe itself"), "fuwa.v1.NodeService/GetNode");
      return { version: n.version, build: n.build, node: n };
    },
  };
}

/**
 * Every failure a client's methods report is a FuwaError subclass, including
 * ones connect raises itself (a caller's abort or deadline).
 */
function typed<S extends DescService>(c: Client<S>, typeName: string): Client<S> {
  return new Proxy(c, {
    get(target, prop, receiver) {
      const fn = Reflect.get(target, prop, receiver) as unknown;
      if (typeof fn !== "function") return fn;
      const method = `${typeName}/${String(prop).replace(/^./, (ch) => ch.toUpperCase())}`;
      return (...args: unknown[]) => {
        const out = (fn as (...a: unknown[]) => unknown).apply(target, args);
        if (out instanceof Promise) return out.catch((cause) => Promise.reject(toFuwaError(cause, method)));
        if (out && typeof out === "object" && Symbol.asyncIterator in out) {
          const stream = out as AsyncIterable<unknown>;
          return (async function* () {
            try {
              yield* stream;
            } catch (cause) {
              throw toFuwaError(cause, method);
            }
          })();
        }
        return out;
      };
    },
  });
}
