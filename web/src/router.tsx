import {
  createRootRoute,
  createRoute,
  createRouter,
  Navigate,
  Outlet,
  useParams,
} from "@tanstack/react-router";
import { useState } from "react";
import { useInstanceOrder } from "@/fuwa/hooks";
import { aliasToKey, keyToAlias } from "@/lib/streamer";
import { Shell } from "@/components/Shell";
import { rememberedPath } from "@/lib/last-path";
import { AppOverlays, StreamerBanner } from "@/components/Shortcuts";
import { instanceKey } from "@/fuwa/saved";
import { DmView } from "@/components/dm/DmView";
import { InstanceHome } from "@/pages/InstanceHome";
import { FriendsPage } from "@/pages/FriendsPage";
import { InvitePage } from "@/pages/InvitePage";
import { LinkedCallback } from "@/pages/LinkedCallback";
import { SsoDone } from "@/pages/SsoDone";
import { ProviderDone } from "@/pages/ProviderDone";
import { ChannelPage, ServerIndex } from "@/pages/ServerPages";
import { Welcome } from "@/pages/Welcome";
import { lazyComponent } from "@/components/lazy";
import { useUi } from "@/lib/ui";
import { reportThrown, setRouteSource } from "@/lib/reports";

const LegalPage = lazyComponent(() => import("@/pages/Legal").then((m) => m.Legal));
const SettingsScreens = lazyComponent(() => import("@/components/settings/UserSettings").then((m) => m.UserSettings));

/** Settings load from their own file the first time they open (or once the app is idle). */
function UserSettings() {
  const open = useUi((u) => u.settings !== null);
  const [opened, setOpened] = useState(open);
  if (open && !opened) setOpened(true);
  return opened ? <SettingsScreens /> : null;
}

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

/** Waifu Devs' terms and privacy policy, on its own instances (pages/Legal). */
const terms = createRoute({ getParentRoute: () => root, path: "terms", component: () => <LegalPage page="terms" /> });
const privacy = createRoute({ getParentRoute: () => root, path: "privacy", component: () => <LegalPage page="privacy" /> });

/** Where waifu.dev sends people back to after signing in with a linked account. */
const linkedCallback = createRoute({ getParentRoute: () => root, path: "auth/waifu/callback", component: LinkedCallback });

/** Where an instance sends people back to after single sign-on (its own, or a server's). */
const ssoDone = createRoute({ getParentRoute: () => root, path: "auth/sso/done", component: SsoDone });

/** Where an instance sends people back to after signing in with Google, X or Twitch, or linking one. */
const providerDone = createRoute({ getParentRoute: () => root, path: "auth/provider/done", component: ProviderDone });

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

/** An encrypted conversation with someone on the instance. */
const dm = createRoute({
  getParentRoute: () => instance,
  path: "dm/$conversation",
  component: function DmRoute() {
    const { instance: key, conversation } = useParams({ from: "/$instance/dm/$conversation" });
    return <DmView key={`${key}/${conversation}`} instanceKey={key} conversationId={conversation} />;
  },
});

/** Your friends on an instance, with requests and blocks. */
const friends = createRoute({
  getParentRoute: () => instance,
  path: "friends",
  component: function FriendsRoute() {
    const { instance: key } = useParams({ from: "/$instance/friends" });
    return <FriendsPage key={key} instanceKey={key} />;
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
  terms,
  privacy,
  linkedCallback,
  ssoDone,
  providerDone,
  instance.addChildren([instanceIndex, invite, dm, friends, server.addChildren([serverIndex, channel])]),
]);

export const router = createRouter({
  routeTree,
  defaultPreload: false,
  // Streamer mode shows instances by a local alias instead of their address.
  rewrite: { input: ({ url }) => aliasToKey(url), output: ({ url }) => keyToAlias(url) },
  // A page that failed to draw, for anonymous reports.
  defaultOnCatch: (error) => reportThrown(error, "render"),
});

// Reports say which page by its pattern ("/$instance/$server/$channel"), never the address itself.
setRouteSource(() => router.state.matches.at(-1)?.routeId);

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
