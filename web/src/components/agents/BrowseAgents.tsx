import { Link } from "@tanstack/react-router";
import { ArrowUpRightIcon, BotIcon, ServerIcon, SparklesIcon, StarIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import type { AgentListing } from "@/gen/fuwa/v1/agent_pb";
import { discoverAgents, run } from "@/fuwa/actions";
import { useInstance } from "@/fuwa/hooks";
import { AddAgentMenu, useManagedServers } from "@/components/agents/AddAgent";
import { AppBadge } from "@/components/AppBadge";
import { UserAvatar } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { Count, Tilt } from "@/components/motion";
import { T, useI18n } from "@/i18n/react";
import { instanceHas } from "@/lib/compat";
import { displayName } from "@/lib/format";
import { cn } from "@/lib/utils";

const EASE = [0.22, 1, 0.36, 1] as const;

/**
 * Browse's agents: the instance's public ones, those its admins feature
 * first and big, then the rest, in the most servers first. Each adds to a
 * server you manage in a tap. Hidden where the instance has none.
 */
export function BrowseAgents({ instanceKey, query }: { instanceKey: string; query: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const here = instanceHas(inst?.node?.versions, "agent-discovery");
  const [found, setFound] = useState<{ agents: AgentListing[]; featured: string[] } | null>(null);
  const managed = useManagedServers(instanceKey).length > 0;

  useEffect(() => {
    if (!here || !inst?.me) return;
    run(discoverAgents(instanceKey)).then(setFound, () => setFound(null));
  }, [instanceKey, here, inst?.me]);

  if (!found?.agents.length) return null;
  const q = query.trim().toLowerCase().replace(/^@/, "");
  const matching = found.agents.filter((a) => {
    const user = a.user!;
    return !q || user.displayName.toLowerCase().includes(q) || user.username.includes(q) || a.bio.toLowerCase().includes(q);
  });
  const featured = new Set(found.featured);
  const shownFeatured = matching.filter((a) => featured.has(a.user!.id));
  const shown = matching.filter((a) => !featured.has(a.user!.id));

  return (
    <section className="mt-12">
      <div className="mb-4 flex items-center gap-2">
        <motion.span
          animate={{ rotate: [0, -10, 10, 0] }}
          transition={{ duration: 1.6, repeat: Infinity, repeatDelay: 3 }}
          className="grid size-8 place-items-center rounded-xl bg-violet-500/15 text-violet-500"
        >
          <BotIcon className="size-4" />
        </motion.span>
        <div>
          <h2 className="text-lg font-extrabold">{t("workspace.agents.browse")}</h2>
          <p className="text-sm text-muted-foreground">{managed ? t("workspace.agents.browseAbout") : t("workspace.agents.browseAboutNoServers")}</p>
        </div>
      </div>
      {matching.length === 0 && <p className="rounded-3xl border border-dashed p-6 text-center text-sm text-muted-foreground">{t("workspace.home.noMatch")}</p>}
      <AnimatePresence initial={false}>
        {shownFeatured.length > 0 && (
          <motion.div key="featured" layout initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -12 }} transition={{ duration: 0.35, ease: EASE }}>
            <p className="mb-3 flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
              <SparklesIcon className="size-3.5 text-violet-500" /> {t("workspace.home.featured")}
              <span className="font-normal tracking-normal normal-case">· {t("workspace.home.featuredAbout")}</span>
            </p>
            <div className="mb-6 grid gap-4 md:grid-cols-2">
              {shownFeatured.map((a, n) => (
                <motion.div
                  key={a.user!.id}
                  layout
                  initial={{ opacity: 0, y: 24, scale: 0.98 }}
                  animate={{ opacity: 1, y: 0, scale: 1 }}
                  transition={{ duration: 0.5, delay: Math.min(n, 6) * 0.07, ease: EASE }}
                  className={cn(n === 0 && shownFeatured.length % 2 === 1 && "md:col-span-2")}
                >
                  <AgentCard instanceKey={instanceKey} agent={a} featured canAdd={managed} />
                </motion.div>
              ))}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
      <motion.div layout className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <AnimatePresence>
          {shown.map((a, n) => (
            <motion.div
              key={a.user!.id}
              layout
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0, pointerEvents: "auto" }}
              exit={{ opacity: 0, scale: 0.95, pointerEvents: "none" }}
              transition={{ duration: 0.4, delay: Math.min(n, 8) * 0.05, ease: EASE, pointerEvents: { delay: 0 } }}
            >
              <AgentCard instanceKey={instanceKey} agent={a} canAdd={managed} />
            </motion.div>
          ))}
        </AnimatePresence>
      </motion.div>
    </section>
  );
}

/** One agent: who it is, what it does, who made it and how many servers it's in, and adding it. */
function AgentCard({ instanceKey, agent, featured = false, canAdd }: { instanceKey: string; agent: AgentListing; featured?: boolean; canAdd: boolean }) {
  const { t } = useI18n();
  const user = agent.user!;
  const [servers, setServers] = useState(agent.servers);
  return (
    <Tilt max={featured ? 3 : 5} className={cn("card-pop tilt flex h-full flex-col gap-3 rounded-3xl border bg-card p-5", featured && "border-violet-500/30 shadow-lg shadow-violet-500/5 sm:p-6")}>
      <div className="flex items-center gap-3">
        <UserAvatar user={user} className={cn("shrink-0", featured ? "size-16 text-xl" : "size-12 text-base")} />
        <div className="min-w-0 flex-1">
          <p className="flex items-center gap-1.5">
            <span className={cn("truncate font-extrabold", featured ? "text-xl" : "text-lg")}>{displayName(user)}</span>
            <AppBadge agent />
          </p>
          <p className="flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
            <span className="truncate">@{user.username}</span>
            <span className="flex items-center gap-1">
              <ServerIcon className="size-3" />
              <T k="workspace.agents.servers" values={{ count: <Count value={servers} /> }} count={servers} />
            </span>
          </p>
        </div>
        {featured && (
          <span className="flex shrink-0 items-center gap-1 self-start rounded-full bg-violet-500/15 px-2.5 py-1 text-xs font-extrabold text-violet-600 dark:text-violet-300">
            <StarIcon className="size-3.5 fill-current" /> {t("workspace.home.featured")}
          </span>
        )}
      </div>
      <p className={cn("flex-1 text-sm text-muted-foreground", featured ? "line-clamp-4" : "line-clamp-3")}>
        {agent.bio.trim() ? <InlineMarkdown>{agent.bio}</InlineMarkdown> : t("workspace.home.noDescription")}
      </p>
      {agent.owner && (
        <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
          <UserAvatar user={agent.owner} className="size-4 text-[0.5rem]" />
          <T k="workspace.agents.madeBy" values={{ name: <b className="text-foreground">{displayName(agent.owner)}</b> }} />
        </p>
      )}
      <div className="flex items-center gap-2">
        {canAdd && <AddAgentMenu instanceKey={instanceKey} agent={user} className="h-9 flex-1" onAdded={() => setServers((n) => n + 1)} />}
        <Link
          to="/$instance/agent/$username"
          params={{ instance: instanceKey, username: user.username }}
          aria-label={t("workspace.agents.open")}
          title={t("workspace.agents.open")}
          className={cn(
            "group grid h-9 place-items-center rounded-xl border px-3 text-sm font-bold text-muted-foreground transition hover:bg-muted hover:text-foreground",
            !canAdd && "flex-1",
          )}
        >
          <span className="flex items-center gap-1">
            {!canAdd && t("workspace.agents.open")}
            <ArrowUpRightIcon className="size-4 transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />
          </span>
        </Link>
      </div>
    </Tilt>
  );
}
