import { ArrowDownIcon, ArrowUpIcon, BotIcon, StarIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import type { AgentListing } from "@/gen/fuwa/v1/agent_pb";
import { discoverAgents, run, setFeaturedAgents } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction } from "@/fuwa/hooks";
import { AppBadge } from "@/components/AppBadge";
import { UserAvatar } from "@/components/Icons";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { T, useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * The instance's public agents, and which of them Browse features first,
 * in what order. Every change sends the whole list, like featured servers.
 */
export function FeaturedAgents({ instanceKey }: { instanceKey: string }) {
  const lang = useI18n();
  const { t } = lang;
  const [agents, setAgents] = useState<AgentListing[] | null>(null);
  const [featured, setFeatured] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const save = useAction(setFeaturedAgents);

  useEffect(() => {
    run(discoverAgents(instanceKey)).then(
      (r) => {
        setAgents(r.agents);
        setFeatured(r.featured);
      },
      (e: FuwaError) => setError(e.message),
    );
  }, [instanceKey]);

  async function change(ids: string[], said?: string) {
    const saved = await save.go(instanceKey, ids);
    if (!saved) return;
    setFeatured(saved);
    if (said) toast(said);
  }
  const swap = (place: number, by: number) => {
    const ids = [...featured];
    [ids[place], ids[place + by]] = [ids[place + by]!, ids[place]!];
    void change(ids);
  };

  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!agents)
    return (
      <div className="flex flex-col gap-3">
        {[0, 1, 2].map((n) => (
          <div key={n} className="shimmer h-16 rounded-2xl" />
        ))}
      </div>
    );

  // Featured first, in their order, as Browse shows them.
  const place = (a: AgentListing) => {
    const at = featured.indexOf(a.user!.id);
    return at < 0 ? featured.length : at;
  };
  const sorted = [...agents].sort((a, b) => place(a) - place(b));

  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-muted-foreground">{t("instancesettings.featuredAgents.intro")}</p>
      {agents.length === 0 && (
        <div className="flex flex-col items-center gap-2 rounded-2xl border border-dashed p-8 text-center">
          <BotIcon className="size-6 text-muted-foreground" />
          <p className="font-bold">{t("instancesettings.featuredAgents.none")}</p>
          <p className="text-sm text-muted-foreground">{t("instancesettings.featuredAgents.noneHint")}</p>
        </div>
      )}
      <ul className="flex flex-col gap-2">
        {sorted.map((a) => {
          const user = a.user!;
          const at = featured.indexOf(user.id);
          const on = at >= 0;
          return (
            <motion.li key={user.id} layout transition={SPRING} className={cn("flex flex-col gap-2 rounded-2xl border bg-background/50 p-3", on && "border-amber-500/40")}>
              <div className="flex items-center gap-3">
                <UserAvatar user={user} className="size-10" />
                <div className="min-w-0 flex-1">
                  <p className="flex items-center gap-1.5">
                    <span className="truncate font-bold">{displayName(user)}</span>
                    <AppBadge agent />
                  </p>
                  <p className="truncate text-xs text-muted-foreground">
                    @{user.username} · <T k="workspace.agents.servers" values={{ count: lang.number(a.servers) }} count={a.servers} />
                    {a.owner && <> · {t("workspace.agents.madeBy", { name: displayName(a.owner) })}</>}
                  </p>
                </div>
                <motion.span animate={{ rotate: on ? 72 : 0, scale: on ? 1.1 : 1 }} transition={SPRING} className={on ? "text-amber-500" : "text-muted-foreground"}>
                  <StarIcon className={cn("size-4", on && "fill-current")} />
                </motion.span>
                <Switch
                  checked={on}
                  disabled={save.pending}
                  aria-label={t("instancesettings.featuredAgents.feature")}
                  onCheckedChange={(next) =>
                    change(
                      next ? [...featured, user.id] : featured.filter((id) => id !== user.id),
                      t(next ? "instancesettings.featuredAgents.featuredNow" : "instancesettings.featuredAgents.unfeaturedNow", { agent: displayName(user) }),
                    )
                  }
                />
              </div>
              <AnimatePresence mode="popLayout" initial={false}>
                {on && featured.length > 1 && (
                  <motion.div {...SLIDE_IN} transition={SPRING} className="flex items-center gap-2 pl-13">
                    <span className="flex-1 text-xs text-muted-foreground tabular-nums">
                      {t("instancesettings.servers.featurePlace", { place: lang.number(at + 1), count: lang.number(featured.length) })}
                    </span>
                    <Button type="button" variant="ghost" size="sm" className="group rounded-xl" disabled={save.pending || at === 0} onClick={() => swap(at, -1)}>
                      <ArrowUpIcon className="transition-transform group-hover:-translate-y-0.5" /> {t("instancesettings.servers.featureEarlier")}
                    </Button>
                    <Button type="button" variant="ghost" size="sm" className="group rounded-xl" disabled={save.pending || at === featured.length - 1} onClick={() => swap(at, 1)}>
                      <ArrowDownIcon className="transition-transform group-hover:translate-y-0.5" /> {t("instancesettings.servers.featureLater")}
                    </Button>
                  </motion.div>
                )}
              </AnimatePresence>
            </motion.li>
          );
        })}
      </ul>
      {save.error && <p className="text-sm text-destructive first-letter:uppercase">{save.error}</p>}
    </div>
  );
}
