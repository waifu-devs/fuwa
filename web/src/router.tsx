import {
  createRootRoute,
  createRoute,
  createRouter,
  Navigate,
  Outlet,
  useParams,
} from "@tanstack/react-router";
import { useInstanceOrder } from "@/fuwa/hooks";
import { aliasToKey, keyToAlias } from "@/lib/streamer";
import { rememberedPath, Shell } from "@/components/Shell";
import { AppOverlays, StreamerBanner } from "@/components/Shortcuts";
import { UserSettings } from "@/components/settings/UserSettings";
import { instanceKey } from "@/fuwa/saved";
import { InstanceHome } from "@/pages/InstanceHome";
import { InvitePage } from "@/pages/InvitePage";
import { ChannelPage, ServerIndex } from "@/pages/ServerPages";
import { Welcome } from "@/pages/Welcome";

/**
 * Addresses name the instance by its host, so links read like
 * /fuwa.waifu.dev/<server>/<channel> and work from any fuwa client.
 */

/** Every page, with the app-wide screens (settings, shortcuts, the quick switcher) around it. */
function Root() {
  return (
    <div className="flex h-full flex-col">
      <StreamerBanner />
      <div className="relative min-h-0 flex-1">
        <Outlet />
      </div>
      <UserSettings />
      <AppOverlays />
    </div>
  );
}

const root = createRootRoute({
  component: Root,
  notFoundComponent: () => <Navigate to="/" replace />,
});

function Home() {
  const order = useInstanceOrder();
  if (order.length === 0) return <Welcome />;
  const last = rememberedPath();
  const lastKey = last?.split("/")[1];
  if (last && lastKey && order.includes(decodeURIComponent(lastKey))) return <Navigate to={last} replace />;
  return <Navigate to="/$instance" params={{ instance: order[0]! }} replace />;
}

const index = createRoute({ getParentRoute: () => root, path: "/", component: Home });

const connect = createRoute({
  getParentRoute: () => root,
  path: "/connect",
  validateSearch: (search: Record<string, unknown>) => ({
    url: typeof search.url === "string" ? search.url : undefined,
  }),
  component: function ConnectPage() {
    const { url } = connect.useSearch();
    return <Welcome initialUrl={url} />;
  },
});

/**
 * An invite link as the instance hands it out (/invite/<code>): this page is
 * served by that instance, so it's the one the invite is for.
 */
const inviteHere = createRoute({
  getParentRoute: () => root,
  path: "invite/$code",
  component: function InviteHere() {
    const { code } = inviteHere.useParams();
    return <Navigate to="/$instance/invite/$code" params={{ instance: instanceKey(window.location.origin), code }} replace />;
  },
});

const instance = createRoute({ getParentRoute: () => root, path: "$instance", component: Shell });

const instanceIndex = createRoute({
  getParentRoute: () => instance,
  path: "/",
  component: function InstanceIndex() {
    const { instance: key } = useParams({ from: "/$instance/" });
    return <InstanceHome instanceKey={key} />;
  },
});

const invite = createRoute({
  getParentRoute: () => instance,
  path: "invite/$code",
  component: function InviteRoute() {
    const { instance: key, code } = useParams({ from: "/$instance/invite/$code" });
    return <InvitePage key={`${key}/${code}`} instanceKey={key} code={code} />;
  },
});

const server = createRoute({ getParentRoute: () => instance, path: "$server" });

const serverIndex = createRoute({
  getParentRoute: () => server,
  path: "/",
  component: function ServerIndexPage() {
    const { instance: key, server: id } = useParams({ from: "/$instance/$server/" });
    return <ServerIndex key={`${key}/${id}`} instanceKey={key} serverId={id} />;
  },
});

const channel = createRoute({
  getParentRoute: () => server,
  path: "$channel",
  component: function ChannelRoute() {
    const { instance: key, server: id, channel: channelId } = useParams({ from: "/$instance/$server/$channel" });
    return <ChannelPage instanceKey={key} serverId={id} channelId={channelId} />;
  },
});

const routeTree = root.addChildren([
  index,
  connect,
  inviteHere,
  instance.addChildren([instanceIndex, invite, server.addChildren([serverIndex, channel])]),
]);

export const router = createRouter({
  routeTree,
  defaultPreload: false,
  // Streamer mode shows instances by a local alias instead of their address.
  rewrite: { input: ({ url }) => aliasToKey(url), output: ({ url }) => keyToAlias(url) },
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
