import { Code } from "@connectrpc/connect";
import { Link, useNavigate } from "@tanstack/react-router";
import type { Effect } from "effect";
import { ChevronLeftIcon, GlobeIcon, HashIcon, Link2OffIcon, LoaderCircleIcon, UsersIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { lookUpInvite, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { Account } from "@/components/Connect";
import { ServerIcon, UserAvatar } from "@/components/Icons";
import { JoinButton } from "@/components/join/JoinButton";
import { ServerDoor } from "@/components/join/ServerDoor";
import { InlineMarkdown } from "@/components/Markdown";
import { Count, EASE_OUT } from "@/components/motion";
import { Private } from "@/components/Private";
import { useLayout } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { displayName } from "@/lib/format";
import { expiresAt, timeLeft } from "@/lib/invites";
import { allowPicturesFrom } from "@/lib/shown";
import { loadSaved, normalizeUrl } from "@/fuwa/saved";
import { store } from "@/fuwa/store";
import { HostedBadge } from "@/components/HostedBadge";

type Found = Effect.Effect.Success<ReturnType<typeof lookUpInvite>>;

/**
 * Where an invite link lands: the server it's for, who sent it, and one
 * button to join. People without an account on that fuwa server sign in or
 * make one right here, then go straight in.
 */
export function InvitePage({ instanceKey, code }: { instanceKey: string; code: string }) {
  const navigate = useNavigate();
  const { compact, setNavOpen } = useLayout();
  const [found, setFound] = useState<Found | null>(null);
  const [problem, setProblem] = useState<FuwaError | null>(null);
  // Signing in here can save the instance under a slightly different address; the button follows it.
  const [key, setKey] = useState(instanceKey);
  const [justSignedIn, setJustSignedIn] = useState(false);
  useEffect(() => {
    setKey(instanceKey);
    setJustSignedIn(false);
  }, [instanceKey]);
  const inst = useInstance(key);
  // From the key alone, so signing in partway (which adds the instance) doesn't look the invite up again.
  const address = instanceKey.replaceAll("~", "/");
  // Looking an invite up talks to its instance, which learns your IP address.
  // One you never added asks first, so a link alone can't make you visit it.
  const [confirmed, setConfirmed] = useState(() => knownInstance(address));

  useEffect(() => {
    if (!confirmed) return;
    allowPicturesFrom(address);
    let cancelled = false;
    setFound(null);
    setProblem(null);
    run(lookUpInvite(address, code)).then(
      (f) => !cancelled && setFound(f),
      (err: FuwaError) => !cancelled && setProblem(err),
    );
    return () => {
      cancelled = true;
    };
  }, [address, code, confirmed]);

  const signedOut = !inst || inst.connection === "signed-out" || (!inst.me && !!inst.node && inst.connection !== "connecting");

  function open() {
    if (!found) return;
    const channel = found.channelName ? found.invite.channelId : "";
    if (channel) navigate({ to: "/$instance/$server/$channel", params: { instance: key, server: found.server.id, channel } });
    else navigate({ to: "/$instance/$server", params: { instance: key, server: found.server.id } });
  }

  return (
    <div className="scroll-thin relative isolate h-full overflow-y-auto">
      <div aria-hidden className="dot-grid absolute inset-0 -z-10 opacity-60" />
      <motion.div
        aria-hidden
        animate={{ scale: [1, 1.12, 1], opacity: [0.5, 0.7, 0.5] }}
        transition={{ duration: 8, repeat: Infinity, ease: "easeInOut" }}
        className="absolute top-1/4 left-1/2 -z-10 size-80 -translate-x-1/2 rounded-full bg-primary/25 blur-3xl"
      />
      {compact && (
        <button type="button" onClick={() => setNavOpen(true)} className="m-2 flex items-center gap-1 rounded-full px-3 py-2 text-sm font-bold text-muted-foreground hover:bg-muted">
          <ChevronLeftIcon className="size-4" /> Servers
        </button>
      )}
      <div className="grid min-h-[calc(100%-3.5rem)] place-items-center p-4 sm:min-h-full">
        <AnimatePresence mode="wait">
          {!confirmed ? (
            <Elsewhere key="elsewhere" address={address} onContinue={() => setConfirmed(true)} />
          ) : problem ? (
            <Broken key="broken" problem={problem} />
          ) : !found ? (
            <motion.div key="loading" exit={{ opacity: 0, scale: 0.97 }} className="w-full max-w-md rounded-3xl border bg-card p-8 shadow-xl">
              <div className="flex flex-col items-center gap-3">
                <div className="shimmer size-20 rounded-3xl" />
                <div className="shimmer h-6 w-1/2 rounded-lg" />
                <div className="shimmer h-4 w-1/3 rounded-lg" />
                <div className="shimmer mt-4 h-11 w-full rounded-xl" />
              </div>
            </motion.div>
          ) : (
            <motion.div
              key="invite"
              initial={{ opacity: 0, y: 24, scale: 0.96 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              transition={{ duration: 0.5, ease: EASE_OUT }}
              className="w-full max-w-md overflow-hidden rounded-3xl border bg-card shadow-2xl"
            >
              <div className="relative flex flex-col items-center gap-2 px-6 pt-8 pb-6 text-center sm:px-8">
                <div aria-hidden className="absolute inset-x-0 top-0 h-24 bg-gradient-to-b from-primary/20 to-transparent" />
                {found.inviter && (
                  <motion.p
                    initial={{ opacity: 0, y: -6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ delay: 0.15 }}
                    className="relative flex items-center gap-1.5 text-sm text-muted-foreground"
                  >
                    <UserAvatar user={found.inviter} className="size-5 text-[0.55rem]" />
                    <b className="text-foreground">{displayName(found.inviter)}</b> invited you to join
                  </motion.p>
                )}
                <motion.div
                  initial={{ scale: 0.5, rotate: -14 }}
                  animate={{ scale: 1, rotate: 0 }}
                  transition={{ type: "spring", stiffness: 380, damping: 13, delay: 0.1 }}
                  className="relative mt-2"
                >
                  <ServerIcon server={found.server} active className="float size-20 text-2xl shadow-[0_14px_30px_-10px_color-mix(in_srgb,var(--primary)_60%,transparent)]" />
                </motion.div>
                <h1 className="relative mt-1 text-2xl font-extrabold tracking-tight">{found.server.name}</h1>
                <p className="relative flex flex-wrap items-center justify-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
                  <span className="flex items-center gap-1">
                    <UsersIcon className="size-3.5" /> <Count value={Number(found.server.memberCount)} />{" "}
                    {found.server.memberCount === 1n ? "member" : "members"}
                  </span>
                  {found.channelName && (
                    <span className="flex items-center gap-0.5 font-bold text-foreground">
                      <HashIcon className="size-3.5" />
                      {found.channelName}
                    </span>
                  )}
                </p>
                {found.server.description.trim() && (
                  <p className="relative line-clamp-3 text-sm break-words text-muted-foreground">
                    <InlineMarkdown>{found.server.description}</InlineMarkdown>
                  </p>
                )}
                <ServerDoor server={found.server} className="relative justify-center" />
                {!signedOut && (
                  <p className="relative flex flex-wrap items-center justify-center gap-x-1 gap-y-1.5 text-xs text-muted-foreground">
                    <span>
                      on <b>{found.node.name}</b> · <Private text={instanceKey} />
                    </span>
                    <HostedBadge url={found.url} className="ml-1" />
                  </p>
                )}
              </div>
              <div className="border-t bg-background/40 p-6 sm:p-8">
                {signedOut ? (
                  <div className="flex flex-col gap-3">
                    <p className="text-center text-sm text-muted-foreground">Sign in, or make an account on this fuwa server, to join.</p>
                    <Account
                      url={found.url}
                      node={found.node}
                      onDone={(k) => {
                        setKey(k);
                        setJustSignedIn(true);
                      }}
                      returnTo={window.location.pathname}
                    />
                  </div>
                ) : !inst?.me ? (
                  <div className="grid h-11 place-items-center">
                    <LoaderCircleIcon className="size-5 animate-spin text-muted-foreground" />
                  </div>
                ) : (
                  <JoinButton
                    instanceKey={key}
                    server={found.server}
                    inviteCode={code}
                    auto={justSignedIn}
                    size="lg"
                    openLabel="You're already in. Open it"
                    onOpen={open}
                  />
                )}
                {(() => {
                  const until = expiresAt(found.invite);
                  return until ? (
                    <p className="mt-3 text-center text-xs text-muted-foreground">This invite expires in {timeLeft(until.getTime() - Date.now())}.</p>
                  ) : null;
                })()}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}

/** An invite that doesn't lead anywhere anymore, or a server that can't be reached. */
/** Whether `address` is this instance or one this browser already added. */
function knownInstance(address: string): boolean {
  let origin: string;
  try {
    origin = new URL(normalizeUrl(address)).origin;
  } catch {
    return false;
  }
  const origins = [location.origin, ...loadSaved().map((i) => i.url), ...Object.values(store.get().instances).map((i) => i.url)];
  return origins.some((url) => {
    try {
      return new URL(url).origin === origin;
    } catch {
      return false;
    }
  });
}

/** Asks before opening an invite on an instance this browser has never talked to. */
function Elsewhere({ address, onContinue }: { address: string; onContinue: () => void }) {
  let host = address;
  try {
    host = new URL(normalizeUrl(address)).host;
  } catch {
    // Shown as typed.
  }
  return (
    <motion.div
      initial={{ opacity: 0, y: 16, scale: 0.96 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.97 }}
      transition={{ duration: 0.45, ease: EASE_OUT }}
      className="flex w-full max-w-md flex-col items-center gap-3 rounded-3xl border bg-card p-8 text-center shadow-xl"
    >
      <motion.span
        initial={{ scale: 0.5, rotate: -20 }}
        animate={{ scale: 1, rotate: 0 }}
        transition={{ type: "spring", stiffness: 380, damping: 12, delay: 0.1 }}
        className="grid size-16 place-items-center rounded-full bg-primary/15 text-primary"
      >
        <motion.span animate={{ rotate: 360 }} transition={{ duration: 18, repeat: Infinity, ease: "linear" }}>
          <GlobeIcon className="size-7" />
        </motion.span>
      </motion.span>
      <h1 className="text-xl font-extrabold">Open this invite on another fuwa?</h1>
      <p className="text-sm text-muted-foreground">
        This invite is for a server on <b className="text-foreground [overflow-wrap:anywhere]">{host}</b>, which you haven't added yet. Opening it
        connects to that instance, so it will see your IP address. Only continue if you trust whoever sent the link.
      </p>
      <div className="mt-1 flex w-full flex-col gap-2 sm:flex-row-reverse">
        <motion.div whileHover={{ scale: 1.02 }} whileTap={{ scale: 0.97 }} className="flex-1">
          <Button onClick={onContinue} className="btn w-full rounded-xl font-bold">
            Continue to {host}
          </Button>
        </motion.div>
        <Button asChild variant="ghost" className="flex-1 rounded-xl font-bold">
          <Link to="/">Go back</Link>
        </Button>
      </div>
    </motion.div>
  );
}

function Broken({ problem }: { problem: FuwaError }) {
  const gone = problem.code === Code.NotFound;
  return (
    <motion.div
      initial={{ opacity: 0, y: 16, scale: 0.96 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.97 }}
      transition={{ duration: 0.45, ease: EASE_OUT }}
      className="flex w-full max-w-md flex-col items-center gap-3 rounded-3xl border bg-card p-8 text-center shadow-xl"
    >
      <motion.span
        initial={{ rotate: 0 }}
        animate={{ rotate: [0, -14, 12, -6, 0] }}
        transition={{ duration: 0.7, delay: 0.25 }}
        className="grid size-16 place-items-center rounded-full bg-muted text-muted-foreground"
      >
        <Link2OffIcon className="size-7" />
      </motion.span>
      <h1 className="text-xl font-extrabold">{gone ? "This invite doesn't work anymore" : "Couldn't open this invite"}</h1>
      <p className="text-sm text-muted-foreground first-letter:uppercase">
        {gone ? "It may have expired, been used up, or been taken back. Ask whoever sent it for a new one." : problem.message}
      </p>
      <Button asChild className="btn mt-1 rounded-xl font-bold">
        <Link to="/">Go to your servers</Link>
      </Button>
    </motion.div>
  );
}
