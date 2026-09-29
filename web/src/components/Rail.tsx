import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { CompassIcon, GlobeIcon, PlusIcon } from "lucide-react";
import { motion } from "motion/react";
import { useState, type ReactNode } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { useFuwa, type InstanceState } from "@/fuwa/store";
import { useInstances } from "@/fuwa/hooks";
import { AddInstanceDialog } from "@/components/dialogs/AddInstanceDialog";
import { CreateServerDialog } from "@/components/dialogs/CreateServerDialog";
import { ConnDot, FuwaMark, ServerIcon } from "@/components/Icons";
import { useLayout } from "@/components/Shell";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { initials } from "@/lib/format";
import { cn } from "@/lib/utils";

/**
 * The far-left column: every server you're in, grouped by the fuwa instance
 * it lives on, so hosted and self-hosted servers sit side by side.
 */
export function Rail() {
  const instances = useInstances();
  const params = useParams({ strict: false }) as { instance?: string; server?: string };
  const [creating, setCreating] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const navigate = useNavigate();
  const signedIn = instances.filter((i) => i.me);

  return (
    <nav
      aria-label="Servers"
      className="surface-rail scroll-none flex h-full w-[72px] shrink-0 flex-col items-center gap-2 overflow-y-auto py-3"
    >
      <RailItem label="fuwa" active={!params.instance} to="/">
        <span className="server-icon grid size-12 place-items-center bg-card" data-active={!params.instance}>
          <FuwaMark className="size-8 float" />
        </span>
      </RailItem>

      {instances.map((inst) => (
        <InstanceGroup key={inst.key} inst={inst} params={params} />
      ))}

      <Divider />
      <DropdownMenu>
        <Tooltip>
          <TooltipTrigger asChild>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                aria-label="Add a server"
                className="server-icon group grid size-12 place-items-center bg-card text-primary transition-colors hover:bg-primary hover:text-primary-foreground"
              >
                <PlusIcon className="size-6 transition-transform duration-300 group-hover:rotate-90" />
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="right">Add a server</TooltipContent>
        </Tooltip>
        <DropdownMenuContent side="right" align="start" className="w-64">
          <DropdownMenuItem disabled={signedIn.length === 0} onSelect={() => setCreating(true)}>
            <PlusIcon /> Create a server
          </DropdownMenuItem>
          {signedIn.map((inst) => (
            <DropdownMenuItem
              key={inst.key}
              onSelect={() => navigate({ to: "/$instance", params: { instance: inst.key } })}
            >
              <CompassIcon /> Browse servers on {inst.node?.name ?? inst.key}
            </DropdownMenuItem>
          ))}
          <DropdownMenuItem onSelect={() => setConnecting(true)}>
            <GlobeIcon /> Connect to another fuwa server
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <CreateServerDialog open={creating} onOpenChange={setCreating} defaultInstance={params.instance} />
      <AddInstanceDialog open={connecting} onOpenChange={setConnecting} />
    </nav>
  );
}

function Divider() {
  return <span aria-hidden className="my-1 h-0.5 w-8 shrink-0 rounded-full bg-border" />;
}

function InstanceGroup({ inst, params }: { inst: InstanceState; params: { instance?: string; server?: string } }) {
  const here = params.instance === inst.key;
  const label = inst.node?.name ?? inst.key;
  return (
    <>
      <Divider />
      <RailItem label={`${label} · ${inst.key}`} active={here && !params.server} to="/$instance" params={{ instance: inst.key }} small>
        <span
          className={cn(
            "server-icon relative grid size-9 place-items-center bg-card text-[0.7rem] font-extrabold text-muted-foreground",
            inst.connection !== "live" && "opacity-70",
          )}
          data-active={here && !params.server}
        >
          {inst.node ? initials(inst.node.name) : <GlobeIcon className="size-4" />}
          <ConnDot state={inst.connection} className="absolute -right-0.5 -bottom-0.5 ring-2 ring-[color-mix(in_srgb,var(--background)_75%,black)]" />
        </span>
      </RailItem>
      {inst.servers.map((server) => (
        <ServerButton key={server.id} inst={inst} server={server} active={here && params.server === server.id} />
      ))}
    </>
  );
}

function ServerButton({ inst, server, active }: { inst: InstanceState; server: Server; active: boolean }) {
  const unread = useFuwa((s) => {
    const i = s.instances[inst.key];
    if (!i) return 0;
    let n = 0;
    for (const c of i.channels[server.id] ?? []) n += i.unread[c.id] ?? 0;
    return n;
  });
  return (
    <RailItem label={server.name} active={active} unread={unread > 0} to="/$instance/$server" params={{ instance: inst.key, server: server.id }}>
      <span className="relative">
        <ServerIcon server={server} active={active} />
        {unread > 0 && !active && (
          <motion.span
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            transition={{ type: "spring", stiffness: 600, damping: 18 }}
            className="absolute -right-1 -bottom-1 grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1 text-[0.65rem] font-extrabold text-white ring-[3px] ring-[color-mix(in_srgb,var(--background)_75%,black)]"
          >
            {unread > 99 ? "99+" : unread}
          </motion.span>
        )}
      </span>
    </RailItem>
  );
}

/** A rail entry with Discord's left pill: a dot when unread, taller on hover, full when open. */
function RailItem({
  label,
  active,
  unread = false,
  small = false,
  children,
  to,
  params,
}: {
  label: string;
  active: boolean;
  unread?: boolean;
  small?: boolean;
  children: ReactNode;
  to: "/" | "/$instance" | "/$instance/$server";
  params?: Record<string, string>;
}) {
  const [hover, setHover] = useState(false);
  const { setNavOpen, compact } = useLayout();
  const height = active ? 40 : hover ? 20 : unread ? 8 : 0;
  return (
    <div className="relative flex w-full justify-center" onPointerEnter={(e) => e.pointerType === "mouse" && setHover(true)} onPointerLeave={() => setHover(false)}>
      <motion.span
        aria-hidden
        className="absolute top-1/2 left-0 w-1 -translate-y-1/2 rounded-r-full bg-foreground"
        initial={false}
        animate={{ height, opacity: height ? 1 : 0 }}
        transition={{ type: "spring", stiffness: 500, damping: 30 }}
      />
      <Tooltip>
        <TooltipTrigger asChild>
          <Link
            to={to}
            params={params as never}
            aria-label={label}
            aria-current={active ? "page" : undefined}
            onClick={() => compact && setNavOpen(true)}
            className={cn("rounded-[50%] outline-none focus-visible:ring-2 focus-visible:ring-ring", small && "my-0.5")}
          >
            {children}
          </Link>
        </TooltipTrigger>
        <TooltipContent side="right" className="font-bold">
          {label}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}
