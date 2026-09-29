import {
  createRootRoute,
  createRoute,
  createRouter,
  Navigate,
  Outlet,
  useParams,
} from "@tanstack/react-router";
import { useInstanceOrder } from "@/fuwa/hooks";
import { rememberedPath, Shell } from "@/components/Shell";
import { InstanceHome } from "@/pages/InstanceHome";
import { ChannelPage, ServerIndex } from "@/pages/ServerPages";
import { Welcome } from "@/pages/Welcome";

/**
 * Addresses name the instance by its host, so links read like
 * /fuwa.waifu.dev/<server>/<channel> and work from any fuwa client.
 */

const root = createRootRoute({
  component: Outlet,
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

const instance = createRoute({ getParentRoute: () => root, path: "$instance", component: Shell });

const instanceIndex = createRoute({
  getParentRoute: () => instance,
  path: "/",
  component: function InstanceIndex() {
    const { instance: key } = useParams({ from: "/$instance/" });
    return <InstanceHome instanceKey={key} />;
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
  instance.addChildren([instanceIndex, server.addChildren([serverIndex, channel])]),
]);

export const router = createRouter({ routeTree, defaultPreload: false });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
