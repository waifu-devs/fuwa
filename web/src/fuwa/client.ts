import { createClient, type Client, type Interceptor } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { AccountService } from "@/gen/fuwa/v1/account_pb";
import { AgentService } from "@/gen/fuwa/v1/agent_pb";
import { AdminService } from "@/gen/fuwa/v1/admin_pb";
import { AuthService } from "@/gen/fuwa/v1/auth_pb";
import { AutoModService } from "@/gen/fuwa/v1/automod_pb";
import { CallService } from "@/gen/fuwa/v1/call_pb";
import { ChannelService, SharedChannelService } from "@/gen/fuwa/v1/channel_pb";
import { DirectMessageService } from "@/gen/fuwa/v1/dm_pb";
import { EmojiService } from "@/gen/fuwa/v1/emoji_pb";
import { EventService } from "@/gen/fuwa/v1/event_pb";
import { FriendService } from "@/gen/fuwa/v1/friend_pb";
import { GifService } from "@/gen/fuwa/v1/gif_pb";
import { InviteService } from "@/gen/fuwa/v1/invite_pb";
import { JoinService } from "@/gen/fuwa/v1/join_pb";
import { MediaService } from "@/gen/fuwa/v1/media_pb";
import { MessageService } from "@/gen/fuwa/v1/message_pb";
import { NodeService } from "@/gen/fuwa/v1/node_pb";
import { RoleService } from "@/gen/fuwa/v1/role_pb";
import { SecureChannelService } from "@/gen/fuwa/v1/secure_pb";
import { ServerService } from "@/gen/fuwa/v1/server_pb";
import { SsoService } from "@/gen/fuwa/v1/sso_pb";
import { WebhookService } from "@/gen/fuwa/v1/webhook_pb";
import { timeCalls } from "@/lib/reports";

/** Typed clients for every fuwa service on one instance. */
export type Api = {
  node: Client<typeof NodeService>;
  auth: Client<typeof AuthService>;
  account: Client<typeof AccountService>;
  servers: Client<typeof ServerService>;
  channels: Client<typeof ChannelService>;
  messages: Client<typeof MessageService>;
  events: Client<typeof EventService>;
  admin: Client<typeof AdminService>;
  media: Client<typeof MediaService>;
  gifs: Client<typeof GifService>;
  roles: Client<typeof RoleService>;
  invites: Client<typeof InviteService>;
  join: Client<typeof JoinService>;
  dms: Client<typeof DirectMessageService>;
  friends: Client<typeof FriendService>;
  secure: Client<typeof SecureChannelService>;
  automod: Client<typeof AutoModService>;
  emojis: Client<typeof EmojiService>;
  calls: Client<typeof CallService>;
  webhooks: Client<typeof WebhookService>;
  agents: Client<typeof AgentService>;
  sso: Client<typeof SsoService>;
  shared: Client<typeof SharedChannelService>;
};

/**
 * Clients for the instance at `url`, over gRPC-Web so they work from any
 * browser. `token` is read on every call, so signing in or out takes effect
 * without rebuilding the clients.
 */
export function makeApi(url: string, token: () => string | null): Api {
  const auth: Interceptor = (next) => (req) => {
    const t = token();
    if (t) req.header.set("authorization", `Bearer ${t}`);
    return next(req);
  };
  const transport = createGrpcWebTransport({ baseUrl: url, interceptors: [auth, timeCalls] });
  return {
    node: createClient(NodeService, transport),
    auth: createClient(AuthService, transport),
    account: createClient(AccountService, transport),
    servers: createClient(ServerService, transport),
    channels: createClient(ChannelService, transport),
    messages: createClient(MessageService, transport),
    events: createClient(EventService, transport),
    admin: createClient(AdminService, transport),
    media: createClient(MediaService, transport),
    gifs: createClient(GifService, transport),
    roles: createClient(RoleService, transport),
    invites: createClient(InviteService, transport),
    join: createClient(JoinService, transport),
    dms: createClient(DirectMessageService, transport),
    friends: createClient(FriendService, transport),
    secure: createClient(SecureChannelService, transport),
    automod: createClient(AutoModService, transport),
    emojis: createClient(EmojiService, transport),
    calls: createClient(CallService, transport),
    webhooks: createClient(WebhookService, transport),
    agents: createClient(AgentService, transport),
    sso: createClient(SsoService, transport),
    shared: createClient(SharedChannelService, transport),
  };
}
