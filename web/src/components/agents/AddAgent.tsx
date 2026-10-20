import { CheckIcon, LoaderCircleIcon, PlusIcon, ServerOffIcon, ShieldIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useMemo, useState } from "react";
import { Permission, type Role, type Server, type User } from "@/gen/fuwa/v1/types_pb";
import { addAgent, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { getInstance, useInstance } from "@/fuwa/hooks";
import type { InstanceState } from "@/fuwa/store";
import { ServerIcon } from "@/components/Icons";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { T, useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { above, accessOf, cssColor, has, sortRoles } from "@/lib/permissions";
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

/** The roles you could give in a server: below your highest, never @everyone, and only with Manage roles. */
function assignableIn(inst: InstanceState | undefined, server: Server): Role[] {
  if (!inst?.me) return [];
  const me = inst.me;
  const member = inst.members[server.id]?.find((m) => m.user?.id === me.id);
  const roles = inst.roles[server.id] ?? [];
  const access = accessOf(server.id, server.ownerId, roles, inst.channels[server.id] ?? [], me.id, member?.roleIds ?? []);
  if (!has(access, Permission.MANAGE_ROLES)) return [];
  return sortRoles(roles.filter((r) => r.id !== server.id && above(access, r.position)));
}

/** `managedServers`, kept up to date as servers, roles and members change. */
export function useManagedServers(key: string): Server[] {
  const inst = useInstance(key);
  return useMemo(() => managedIn(inst), [inst]);
}

/**
 * Adding one agent to the servers you manage: which of them it's already
 * in (as far as this app has loaded their members), and the call. Where you
 * could give it roles, `request` asks which first (render `dialog`).
 */
export function useAddAgent(
  instanceKey: string,
  agent: Pick<User, "id" | "username" | "displayName">,
  onAdded?: (server: Server) => void,
  options: { asking?: Server; onAsk?: (server: Server) => void; onDone?: () => void } = {},
) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const servers = useManagedServers(instanceKey);
  const [adding, setAdding] = useState<string | null>(null);
  const [added, setAdded] = useState<ReadonlySet<string>>(new Set());
  const [asking, setAsking] = useState<Server | null>(options.asking ?? null);
  const inServer = (s: Server) => added.has(s.id) || !!inst?.members[s.id]?.some((m) => m.user?.id === agent.id);

  async function add(server: Server, roleIds: string[] = []) {
    if (adding || inServer(server)) return;
    setAdding(server.id);
    try {
      await run(addAgent(instanceKey, server.id, agent.username, roleIds));
      setAdded((a) => new Set(a).add(server.id));
      toast(t("workspace.agents.addedTo", { server: server.name }));
      onAdded?.(server);
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setAdding(null);
  }

  function request(server: Server) {
    if (adding || inServer(server)) return;
    if (assignableIn(inst, server).length === 0) void add(server);
    else if (options.onAsk) options.onAsk(server);
    else setAsking(server);
  }

  const dialog = (
    <RolesDialog
      agent={agent}
      server={asking}
      roles={asking ? assignableIn(inst, asking) : []}
      onClose={() => {
        setAsking(null);
        options.onDone?.();
      }}
      onAdd={(roleIds) => {
        const server = asking!;
        setAsking(null);
        void add(server, roleIds).finally(options.onDone);
      }}
    />
  );

  return { servers, adding, inServer, request, dialog };
}

/** Which roles the agent joins with, before it's added: none ticked, so it starts with what everyone has. */
function RolesDialog({
  agent,
  server,
  roles,
  onClose,
  onAdd,
}: {
  agent: Pick<User, "displayName" | "username">;
  server: Server | null;
  roles: Role[];
  onClose: () => void;
  onAdd: (roleIds: string[]) => void;
}) {
  const { t } = useI18n();
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const toggle = (id: string) =>
    setPicked((p) => {
      const next = new Set(p);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  return (
    <Dialog
      open={!!server}
      onOpenChange={(open) => {
        if (!open) {
          onClose();
          setPicked(new Set());
        }
      }}
    >
      <DialogContent>
        <DialogHeader
          title={<T k="workspace.agents.rolesTitle" values={{ agent: displayName(agent as User), server: server?.name ?? "" }} />}
          description={t("workspace.agents.rolesAbout")}
        />
        <ul className="scroll-thin flex max-h-72 flex-col gap-1.5 overflow-y-auto">
          {roles.map((role) => {
            const on = picked.has(role.id);
            return (
              <li key={role.id}>
                <button
                  type="button"
                  aria-pressed={on}
                  onClick={() => toggle(role.id)}
                  className={cn(
                    "flex w-full items-center gap-3 rounded-2xl border px-3 py-2 text-left transition",
                    on ? "border-primary/50 bg-primary/10" : "hover:bg-muted",
                  )}
                >
                  <ShieldIcon className="size-4 shrink-0" style={role.color !== undefined ? { color: cssColor(role.color) } : undefined} />
                  <span className="min-w-0 flex-1 truncate font-bold">{role.name}</span>
                  <span className={cn("grid size-5 place-items-center rounded-md border transition", on && "border-primary bg-primary text-primary-foreground")}>
                    {on && <CheckIcon className="size-3.5" strokeWidth={3} />}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
        <div className="mt-5 flex justify-end gap-2">
          <Button type="button" variant="ghost" className="rounded-xl font-bold" onClick={onClose}>
            {t("workspace.agents.cancel")}
          </Button>
          <Button type="button" className="btn rounded-xl font-bold" onClick={() => onAdd([...picked])}>
            <PlusIcon /> {picked.size ? t("workspace.agents.addWithRoles", { count: picked.size }) : t("workspace.agents.addWithoutRoles")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
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
  onAsk,
}: {
  instanceKey: string;
  agent: Pick<User, "id" | "username" | "displayName">;
  className?: string;
  onAdded?: (server: Server) => void;
  /** Asks for roles somewhere else (the profile card closes first), instead of in a dialog of its own. */
  onAsk?: (server: Server) => void;
}) {
  const { t } = useI18n();
  const { servers, adding, inServer, request, dialog } = useAddAgent(instanceKey, agent, onAdded, { onAsk });
  return (
    <>
      {dialog}
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
              <DropdownMenuItem key={s.id} disabled={here} onSelect={() => request(s)}>
                <ServerIcon server={s} className="size-6 text-[0.6rem]" /> <span className="truncate">{s.name}</span>
                {here && <CheckIcon className="ml-auto" />}
              </DropdownMenuItem>
            );
          })}
        </DropdownMenuContent>
      </DropdownMenu>
    </>
  );
}

/** The servers you manage as a list, each with its own Add: the agent invite page's main part. */
export function AddAgentList({
  instanceKey,
  agent,
  onAdded,
}: {
  instanceKey: string;
  agent: Pick<User, "id" | "username" | "displayName">;
  onAdded?: (server: Server) => void;
}) {
  const { t } = useI18n();
  const { servers, adding, inServer, request, dialog } = useAddAgent(instanceKey, agent, onAdded);
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
      {dialog}
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
                onClick={() => request(s)}
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

/** The roles dialog alone, for one server: what the profile card opens once it has closed. */
export function AddAgentRoles({
  instanceKey,
  agent,
  server,
  onDone,
}: {
  instanceKey: string;
  agent: Pick<User, "id" | "username" | "displayName">;
  server: Server;
  onDone: () => void;
}) {
  return useAddAgent(instanceKey, agent, undefined, { asking: server, onDone }).dialog;
}
