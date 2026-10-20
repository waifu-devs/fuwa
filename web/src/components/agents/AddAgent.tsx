import { CheckIcon, LoaderCircleIcon, PlusIcon, ServerOffIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useMemo, useState } from "react";
import { Permission, type Server, type User } from "@/gen/fuwa/v1/types_pb";
import { addAgent, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { getInstance, useInstance } from "@/fuwa/hooks";
import type { InstanceState } from "@/fuwa/store";
import { ServerIcon } from "@/components/Icons";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { useI18n } from "@/i18n/react";
import { SPRING } from "@/lib/motion";
import { accessOf, has } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** The servers you manage on an instance, where you can add an agent. */
export const managedServers = (key: string): Server[] => managedIn(getInstance(key));

function managedIn(inst: InstanceState | undefined): Server[] {
  if (!inst?.me) return [];
  const me = inst.me;
  return inst.servers.filter((s) => {
    const member = inst.members[s.id]?.find((m) => m.user?.id === me.id);
    const access = accessOf(s.id, s.ownerId, inst.roles[s.id] ?? [], inst.channels[s.id] ?? [], me.id, member?.roleIds ?? []);
    return has(access, Permission.MANAGE_SERVER);
  });
}

/** `managedServers`, kept up to date as servers, roles and members change. */
export function useManagedServers(key: string): Server[] {
  const inst = useInstance(key);
  return useMemo(() => managedIn(inst), [inst]);
}

/**
 * Adding one agent to the servers you manage: which of them it's already
 * in (as far as this app has loaded their members), and the call.
 */
export function useAddAgent(instanceKey: string, agent: Pick<User, "id" | "username">, onAdded?: (server: Server) => void) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const servers = useManagedServers(instanceKey);
  const [adding, setAdding] = useState<string | null>(null);
  const [added, setAdded] = useState<ReadonlySet<string>>(new Set());
  const inServer = (s: Server) => added.has(s.id) || !!inst?.members[s.id]?.some((m) => m.user?.id === agent.id);

  async function add(server: Server) {
    if (adding || inServer(server)) return;
    setAdding(server.id);
    try {
      await run(addAgent(instanceKey, server.id, agent.username));
      setAdded((a) => new Set(a).add(server.id));
      toast(t("workspace.agents.addedTo", { server: server.name }));
      onAdded?.(server);
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setAdding(null);
  }

  return { servers, adding, inServer, add };
}

/**
 * "Add to server": a menu of the servers you manage, the ones it's in
 * already ticked. For the profile card and Browse's agent cards.
 */
export function AddAgentMenu({
  instanceKey,
  agent,
  className,
  onAdded,
}: {
  instanceKey: string;
  agent: Pick<User, "id" | "username">;
  className?: string;
  onAdded?: (server: Server) => void;
}) {
  const { t } = useI18n();
  const { servers, adding, inServer, add } = useAddAgent(instanceKey, agent, onAdded);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button type="button" className={cn("btn group rounded-xl font-bold", className)} disabled={!!adding}>
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span
              key={adding ? "adding" : "add"}
              initial={{ y: 10, opacity: 0 }}
              animate={{ y: 0, opacity: 1 }}
              exit={{ y: -10, opacity: 0 }}
              transition={SPRING}
              className="flex items-center gap-1.5"
            >
              {adding ? <LoaderCircleIcon className="animate-spin" /> : <PlusIcon className="transition-transform duration-300 group-hover:rotate-90" />}
              {t("workspace.agents.addToServer")}
            </motion.span>
          </AnimatePresence>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-72 w-64 overflow-y-auto">
        <DropdownMenuLabel className="text-xs text-muted-foreground">{t("workspace.agents.managed")}</DropdownMenuLabel>
        {servers.length === 0 && <p className="px-2 py-1.5 text-sm text-muted-foreground">{t("workspace.agents.noManaged")}</p>}
        {servers.map((s) => {
          const here = inServer(s);
          return (
            <DropdownMenuItem key={s.id} disabled={here} onSelect={() => void add(s)}>
              <ServerIcon server={s} className="size-6 text-[0.6rem]" /> <span className="truncate">{s.name}</span>
              {here && <CheckIcon className="ml-auto" />}
            </DropdownMenuItem>
          );
        })}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** The servers you manage as a list, each with its own Add: the agent invite page's main part. */
export function AddAgentList({
  instanceKey,
  agent,
  onAdded,
}: {
  instanceKey: string;
  agent: Pick<User, "id" | "username">;
  onAdded?: (server: Server) => void;
}) {
  const { t } = useI18n();
  const { servers, adding, inServer, add } = useAddAgent(instanceKey, agent, onAdded);
  if (servers.length === 0)
    return (
      <div className="flex flex-col items-center gap-2 rounded-2xl border border-dashed p-5 text-center">
        <ServerOffIcon className="size-6 text-muted-foreground" />
        <p className="text-sm font-bold">{t("workspace.agents.noManagedTitle")}</p>
        <p className="text-xs text-muted-foreground">{t("workspace.agents.noManaged")}</p>
      </div>
    );
  return (
    <div className="flex flex-col gap-1.5">
      <p className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("workspace.agents.managed")}</p>
      <ul className="scroll-thin flex max-h-72 flex-col gap-1.5 overflow-y-auto">
        {servers.map((s, n) => {
          const here = inServer(s);
          return (
            <motion.li
              key={s.id}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ ...SPRING, delay: Math.min(n, 8) * 0.04 }}
              className="flex items-center gap-3 rounded-2xl border bg-background/50 p-2 pl-2.5"
            >
              <ServerIcon server={s} className="size-9 text-xs" />
              <span className="min-w-0 flex-1 truncate font-bold">{s.name}</span>
              <Button
                type="button"
                size="sm"
                variant={here ? "ghost" : "default"}
                disabled={here || !!adding}
                onClick={() => void add(s)}
                className={cn("rounded-xl font-bold", !here && "btn")}
              >
                <AnimatePresence mode="popLayout" initial={false}>
                  <motion.span
                    key={here ? "here" : adding === s.id ? "adding" : "add"}
                    initial={{ scale: 0.5, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    exit={{ scale: 0.5, opacity: 0 }}
                    transition={SPRING}
                    className="flex items-center gap-1"
                  >
                    {here ? <CheckIcon strokeWidth={3} /> : adding === s.id ? <LoaderCircleIcon className="animate-spin" /> : <PlusIcon />}
                    {here ? t("workspace.agents.here") : t("workspace.agents.add")}
                  </motion.span>
                </AnimatePresence>
              </Button>
            </motion.li>
          );
        })}
      </ul>
    </div>
  );
}
