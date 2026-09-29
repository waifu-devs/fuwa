import { createClient, type Client, type Interceptor } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { AccountService } from "@/gen/fuwa/v1/account_pb";
import { AdminService } from "@/gen/fuwa/v1/admin_pb";
import { AuthService } from "@/gen/fuwa/v1/auth_pb";
import { ChannelService } from "@/gen/fuwa/v1/channel_pb";
import { EventService } from "@/gen/fuwa/v1/event_pb";
import { MessageService } from "@/gen/fuwa/v1/message_pb";
import { NodeService } from "@/gen/fuwa/v1/node_pb";
import { ServerService } from "@/gen/fuwa/v1/server_pb";

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
  const transport = createGrpcWebTransport({ baseUrl: url, interceptors: [auth] });
  return {
    node: createClient(NodeService, transport),
    auth: createClient(AuthService, transport),
    account: createClient(AccountService, transport),
    servers: createClient(ServerService, transport),
    channels: createClient(ChannelService, transport),
    messages: createClient(MessageService, transport),
    events: createClient(EventService, transport),
    admin: createClient(AdminService, transport),
  };
}
