import { Code } from "@connectrpc/connect";
import { Link } from "@tanstack/react-router";
import { BotIcon, CheckIcon, ChevronLeftIcon, CopyIcon, LoaderCircleIcon, ServerIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import type { AgentListing } from "@/gen/fuwa/v1/agent_pb";
import type { Node } from "@/gen/fuwa/v1/types_pb";
import { lookUpAgent, lookUpAgentHere, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { AddAgentList } from "@/components/agents/AddAgent";
import { AppBadge } from "@/components/AppBadge";
import { Account } from "@/components/Connect";
import { HostedBadge } from "@/components/HostedBadge";
import { UserAvatar } from "@/components/Icons";
import { Markdown } from "@/components/Markdown";
import { Count } from "@/components/motion";
import { Private } from "@/components/Private";
import { useLayout } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { T, useI18n } from "@/i18n/react";
import { agentLink } from "@/lib/agent-links";
import { displayName } from "@/lib/format";
import { EASE_OUT } from "@/lib/motion";
import { allowPicturesFrom } from "@/lib/shown";
import { toast } from "@/lib/ui";
import { Elsewhere, knownInstance } from "./InvitePage";

type Found = { url: string; node: Node | undefined; agent: AgentListing };

/**
 * Where an agent's invite link lands (<instance>/agent/<username>): who the
 * agent is and who made it, then the servers you manage there, each with
 * its own Add. People not signed in to that instance sign in right here.
 */
export function AgentPage({ instanceKey, username }: { instanceKey: string; username: string }) {
  const { t } = useI18n();
  const { compact, setNavOpen } = useLayout();
  const [found, setFound] = useState<Found | null>(null);
  const [problem, setProblem] = useState<FuwaError | null>(null);
  // Signing in here can save the instance under a slightly different address; the list follows it.
  const [signedIn, setSignedIn] = useState<{ on: string; key: string } | null>(null);
  const key = signedIn?.on === instanceKey ? signedIn.key : instanceKey;
  const inst = useInstance(key);
  const address = instanceKey.replaceAll("~", "/");
  // Like an invite: an instance you never added asks first, so a link alone can't make you visit it.
  const [confirmed, setConfirmed] = useState(() => knownInstance(address));
  const me = inst?.me?.id;

  useEffect(() => {
    if (!confirmed) return;
    allowPicturesFrom(address);
    let cancelled = false;
    setFound(null);
    setProblem(null);
    run(lookUpAgent(address, username)).then(
      (f) => !cancelled && setFound(f),
      async (err: FuwaError) => {
        // A private agent shows only to its owner, signed in.
        if (err.code === Code.NotFound && me) {
          try {
            const agent = await run(lookUpAgentHere(key, { username }));
            if (!cancelled) setFound({ url: inst?.url ?? address, node: inst?.node ?? undefined, agent });
            return;
          } catch {
            // Gone for them too.
          }
        }
        if (!cancelled) setProblem(err);
      },
    );
    return () => {
      cancelled = true;
    };
    // `inst` is read for its address only; following it would look the agent up on every change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [address, username, confirmed, me, key]);

  const signedOut = !inst || inst.connection === "signed-out" || (!inst.me && !!inst.node && inst.connection !== "connecting");

  return (
    <div className="scroll-thin relative isolate h-full overflow-y-auto">
      <div aria-hidden className="dot-grid absolute inset-0 -z-10 opacity-60" />
      <motion.div
        aria-hidden
        animate={{ scale: [1, 1.12, 1], opacity: [0.5, 0.7, 0.5] }}
        transition={{ duration: 8, repeat: Infinity, ease: "easeInOut" }}
        className="absolute top-1/4 left-1/2 -z-10 size-80 -translate-x-1/2 rounded-full bg-violet-500/25 blur-3xl"
      />
      {compact && (
        <button type="button" onClick={() => setNavOpen(true)} className="m-2 flex items-center gap-1 rounded-full px-3 py-2 text-sm font-bold text-muted-foreground hover:bg-muted">
          <ChevronLeftIcon className="size-4" /> {t("workspace.invitePage.servers")}
        </button>
      )}
      <div className="grid min-h-[calc(100%-3.5rem)] place-items-center p-4 sm:min-h-full">
        <AnimatePresence mode="wait">
          {!confirmed ? (
            <Elsewhere key="elsewhere" agent address={address} onContinue={() => setConfirmed(true)} />
          ) : problem ? (
            <Missing key="missing" problem={problem} />
          ) : !found ? (
            <motion.div key="loading" exit={{ opacity: 0, scale: 0.97 }} className="w-full max-w-md rounded-3xl border bg-card p-8 shadow-xl">
              <div className="flex flex-col items-center gap-3">
                <div className="shimmer size-20 rounded-full" />
                <div className="shimmer h-6 w-1/2 rounded-lg" />
                <div className="shimmer h-4 w-1/3 rounded-lg" />
                <div className="shimmer mt-4 h-11 w-full rounded-xl" />
              </div>
            </motion.div>
          ) : (
            <motion.div
              key="agent"
              initial={{ opacity: 0, y: 24, scale: 0.96 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              transition={{ duration: 0.5, ease: EASE_OUT }}
              className="w-full max-w-md overflow-hidden rounded-3xl border bg-card shadow-2xl"
            >
              <AgentHeader found={found} instanceKey={instanceKey} signedOut={signedOut} />
              <div className="border-t bg-background/40 p-6 sm:p-8">
                {signedOut ? (
                  <div className="flex flex-col gap-3">
                    <p className="text-center text-sm text-muted-foreground">{t("workspace.agentPage.signIn")}</p>
                    <Account url={found.url} node={found.node!} onDone={(k) => setSignedIn({ on: instanceKey, key: k })} returnTo={window.location.pathname} />
                  </div>
                ) : !inst?.me ? (
                  <div className="grid h-11 place-items-center">
                    <LoaderCircleIcon className="size-5 animate-spin text-muted-foreground" />
                  </div>
                ) : (
                  <AddAgentList
                    instanceKey={key}
                    agent={found.agent.user!}
                    onAdded={() => setFound((f) => (f ? { ...f, agent: { ...f.agent, servers: f.agent.servers + 1 } } : f))}
                  />
                )}
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}

/** The agent: its picture, name, what it does, who made it, how many servers it's in, and its link to pass on. */
function AgentHeader({ found, instanceKey, signedOut }: { found: Found; instanceKey: string; signedOut: boolean }) {
  const { t } = useI18n();
  const { agent } = found;
  const user = agent.user!;
  const [copied, setCopied] = useState(false);

  async function copy() {
    try {
      await navigator.clipboard.writeText(agentLink(found.node?.publicUrl || found.url, user.username));
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      toast(t("workspace.agents.copyFailed"));
    }
  }

  return (
    <div className="relative flex flex-col items-center gap-2 px-6 pt-8 pb-6 text-center sm:px-8">
      <motion.div
        initial={{ scale: 0.5, rotate: -14, y: 12 }}
        animate={{ scale: 1, rotate: 0, y: 0 }}
        transition={{ type: "spring", stiffness: 380, damping: 13, delay: 0.1 }}
        className="relative"
      >
        <UserAvatar user={user} className="float size-20 text-2xl ring-4 ring-violet-500/30" />
        <span className="absolute -right-1 -bottom-1 grid size-7 place-items-center rounded-full border-4 border-card bg-violet-500 text-white">
          <BotIcon className="size-3.5" />
        </span>
      </motion.div>
      <h1 className="mt-1 flex items-center gap-2 text-2xl font-extrabold tracking-tight">
        <span className="truncate">{displayName(user)}</span>
        <AppBadge agent />
      </h1>
      <p className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
        <span>@{user.username}</span>
        <span className="flex items-center gap-1">
          <ServerIcon className="size-3.5" />
          <T k="workspace.agents.servers" values={{ count: <Count value={agent.servers} /> }} count={agent.servers} />
        </span>
      </p>
      {agent.owner && (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <UserAvatar user={agent.owner} className="size-5 text-[0.55rem]" />
          <T k="workspace.agents.madeBy" values={{ name: <b className="text-foreground">{displayName(agent.owner)}</b> }} />
        </p>
      )}
      {agent.bio.trim() && <Markdown className="mt-1 line-clamp-6 max-w-full text-left text-sm break-words text-muted-foreground">{agent.bio}</Markdown>}
      {!agent.public && <p className="rounded-full bg-muted px-2.5 py-0.5 text-xs font-bold text-muted-foreground">{t("workspace.agentPage.private")}</p>}
      <div className="mt-1 flex flex-wrap items-center justify-center gap-2">
        <Button type="button" variant="ghost" size="sm" onClick={() => void copy()} className="rounded-xl font-bold">
          {copied ? <CheckIcon strokeWidth={3} /> : <CopyIcon />} {copied ? t("workspace.agents.copied") : t("workspace.agents.copyLink")}
        </Button>
      </div>
      {!signedOut && found.node && (
        <p className="flex flex-wrap items-center justify-center gap-x-1 gap-y-1.5 text-xs text-muted-foreground">
          <span>
            <T k="workspace.invitePage.on" values={{ name: <b>{found.node.name}</b>, address: <Private text={instanceKey} /> }} />
          </span>
          <HostedBadge url={found.url} className="ml-1" />
        </p>
      )}
    </div>
  );
}

/** A link to an agent that isn't there anymore (or never was public), or an instance that can't be reached. */
function Missing({ problem }: { problem: FuwaError }) {
  const { t } = useI18n();
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
        animate={{ rotate: [0, -14, 12, -6, 0] }}
        transition={{ duration: 0.7, delay: 0.25 }}
        className="grid size-16 place-items-center rounded-full bg-muted text-muted-foreground"
      >
        <BotIcon className="size-7" />
      </motion.span>
      <h1 className="text-xl font-extrabold">{gone ? t("workspace.agentPage.gone") : t("workspace.agentPage.failed")}</h1>
      <p className="text-sm text-muted-foreground first-letter:uppercase">{gone ? t("workspace.agentPage.goneAbout") : problem.message}</p>
      <Button asChild className="btn mt-1 rounded-xl font-bold">
        <Link to="/">{t("workspace.invitePage.broken.home")}</Link>
      </Button>
    </motion.div>
  );
}
