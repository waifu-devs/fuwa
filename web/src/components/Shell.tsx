import { Outlet, useParams, useRouterState } from "@tanstack/react-router";
import { AnimatePresence, motion } from "motion/react";
import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { AnnouncementBanner } from "@/components/AnnouncementBanner";
import { ChannelSidebar } from "@/components/ChannelSidebar";
import { InstanceSidebar } from "@/components/InstanceSidebar";
import { EASE_OUT } from "@/components/motion";
import { Rail } from "@/components/Rail";
import { useInstance } from "@/fuwa/hooks";
import { onCommand } from "@/lib/ui";
import { useMediaQuery } from "@/lib/use-media-query";

/**
 * The app frame: the server rail and a sidebar on the left, the page on the
 * right. On phones the left side is its own screen that the chat slides over,
 * like Discord's mobile app.
 */

type Layout = {
  /** Phone-sized screen. */
  compact: boolean;
  navOpen: boolean;
  setNavOpen: (open: boolean) => void;
  membersOpen: boolean;
  setMembersOpen: (open: boolean) => void;
};

const LayoutContext = createContext<Layout | null>(null);

export function useLayout(): Layout {
  const layout = useContext(LayoutContext);
  if (!layout) throw new Error("useLayout outside Shell");
  return layout;
}

const LAST_PATH = "fuwa:last-path";

export function rememberedPath(): string | null {
  try {
    return localStorage.getItem(LAST_PATH);
  } catch {
    return null;
  }
}

export function Shell() {
  const params = useParams({ strict: false }) as { instance?: string; server?: string; channel?: string; code?: string };
  const pathname = useRouterState({ select: (s) => s.location.pathname });
  const compact = !useMediaQuery("(min-width: 768px)");
  const wide = useMediaQuery("(min-width: 1280px)");
  const [navOpen, setNavOpen] = useState(!params.channel && !params.code);
  const [membersOpen, setMembersOpen] = useState(wide);

  // Opening a channel on a phone shows the chat; leaving one shows the list. Signed out (the session
  // ended, or an admin turned the account off), it shows the page asking to sign in; an invite shows itself.
  const inst = useInstance(params.instance);
  const signedOut = inst?.connection === "signed-out";
  // An instance this browser doesn't know yet has nothing to list, only the page asking to connect.
  const known = !!inst;
  const page = !!params.channel || !!params.code || !known;
  useEffect(() => {
    setNavOpen(!page && !signedOut);
  }, [page, params.server, signedOut]);
  useEffect(() => setMembersOpen(wide), [wide]);
  // The members shortcut works while a channel is open.
  const inChannel = !!params.channel;
  useEffect(() => (inChannel ? onCommand("toggleMembers", () => setMembersOpen((open) => !open)) : undefined), [inChannel]);
  // An invite isn't a place to come back to, so it isn't remembered.
  const invite = !!params.code;
  useEffect(() => {
    if (invite) return;
    try {
      localStorage.setItem(LAST_PATH, pathname);
    } catch {
      // Not remembered; the app still opens on the first server next time.
    }
  }, [pathname, invite]);

  const layout = useMemo(
    () => ({ compact, navOpen, setNavOpen, membersOpen, setMembersOpen }),
    [compact, navOpen, membersOpen],
  );

  const side = params.instance && known ? (
    params.server ? (
      <ChannelSidebar key={`${params.instance}/${params.server}`} instanceKey={params.instance} serverId={params.server} />
    ) : (
      <InstanceSidebar key={params.instance} instanceKey={params.instance} />
    )
  ) : null;

  return (
    <LayoutContext.Provider value={layout}>
      <div className="flex h-full flex-col">
        <AnnouncementBanner instanceKey={params.instance} />
        <div className="flex min-h-0 flex-1 overflow-hidden">
          {compact ? (
            <CompactFrame navOpen={navOpen} nav={<Nav side={side} />} />
          ) : (
            <>
              <Nav side={side} />
              <main className="surface-chat relative flex min-w-0 flex-1 flex-col">
                <Page />
              </main>
            </>
          )}
        </div>
      </div>
    </LayoutContext.Provider>
  );
}

function Nav({ side }: { side: ReactNode }) {
  return (
    <div className="flex h-full shrink-0">
      <Rail />
      <AnimatePresence mode="popLayout" initial={false}>
        {side && (
          <motion.aside
            key="side"
            initial={{ opacity: 0, x: -12 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0, x: -12 }}
            transition={{ duration: 0.2 }}
            className="surface-side flex h-full w-[calc(100vw-72px)] flex-col border-r md:w-64"
          >
            {side}
          </motion.aside>
        )}
      </AnimatePresence>
    </div>
  );
}

/** The page, rising in softly when you go to another server or instance. */
function Page() {
  const params = useParams({ strict: false }) as { instance?: string; server?: string };
  return (
    <motion.div
      key={`${params.instance}/${params.server ?? ""}`}
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ duration: 0.35, ease: EASE_OUT }}
      className="relative flex min-h-0 flex-1 flex-col"
    >
      <Outlet />
    </motion.div>
  );
}

/** Phones: the list is underneath, the chat slides in over it from the right. */
function CompactFrame({ navOpen, nav }: { navOpen: boolean; nav: ReactNode }) {
  return (
    <div className="relative h-full w-full overflow-hidden">
      <div className="absolute inset-0 flex">{nav}</div>
      <motion.main
        className="surface-chat absolute inset-0 flex flex-col shadow-[-12px_0_40px_-20px_rgb(0_0_0/0.6)]"
        initial={false}
        animate={{ x: navOpen ? "100%" : "0%" }}
        transition={{ type: "spring", stiffness: 380, damping: 38 }}
        aria-hidden={navOpen}
        inert={navOpen}
      >
        <Page />
      </motion.main>
    </div>
  );
}
