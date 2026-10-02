import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { CompassIcon, GlobeIcon, PlusIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState, type ReactNode, type Ref } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { useFuwa, type InstanceState } from "@/fuwa/store";
import { useInstances } from "@/fuwa/hooks";
import { AddInstanceDialog } from "@/components/dialogs/AddInstanceDialog";
import { CreateServerDialog } from "@/components/dialogs/CreateServerDialog";
import { ConnDot, FuwaMark, ServerIcon } from "@/components/Icons";
import { AppliedButton } from "@/components/join/Applied";
import { Count, SPRING } from "@/components/motion";
import { Private, useAddress } from "@/components/Private";
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
import { effectiveNotifications, useNow } from "@/lib/notifications";
import { hostedByUs } from "@/lib/hosted";

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

      <AnimatePresence initial={false} mode="popLayout">
        {instances.map((inst) => (
          <Pop key={inst.key}>
            <InstanceGroup inst={inst} params={params} />
          </Pop>
        ))}
      </AnimatePresence>

      <motion.div layout="position" transition={SPRING} className="flex flex-col items-center gap-2">
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
              <CompassIcon /> Browse servers on {inst.node?.name ?? <Private text={inst.key} />}
            </DropdownMenuItem>
          ))}
          <DropdownMenuItem onSelect={() => setConnecting(true)}>
            <GlobeIcon /> Connect to another fuwa server
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      </motion.div>

      <CreateServerDialog open={creating} onOpenChange={setCreating} defaultInstance={params.instance} />
      <AddInstanceDialog open={connecting} onOpenChange={setConnecting} />
    </nav>
  );
}

function Divider() {
  return <span aria-hidden className="my-1 h-0.5 w-8 shrink-0 rounded-full bg-border" />;
}

/** Rail entries pop in when you join or add something, and shrink away when you leave. */
function Pop({ children, className, ref }: { children: ReactNode; className?: string; ref?: Ref<HTMLDivElement> }) {
  return (
    <motion.div
      ref={ref}
      layout="position"
      initial={{ opacity: 0, scale: 0.3 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.3 }}
      transition={SPRING}
      className={cn("flex w-full flex-col items-center gap-2", className)}
    >
      {children}
    </motion.div>
  );
}

function InstanceGroup({ inst, params }: { inst: InstanceState; params: { instance?: string; server?: string } }) {
  const here = params.instance === inst.key;
  const address = useAddress(inst.key);
  const label = inst.node?.name ?? address;
  const hosted = hostedByUs(inst.url) ? " · Hosted by Waifu Devs" : "";
  // Unread direct messages show on the instance they're on.
  const dms = useFuwa((s) => Object.values(s.instances[inst.key]?.dms.unread ?? {}).reduce((sum, n) => sum + n, 0));
  return (
    <>
      <Divider />
      <RailItem
        label={inst.node ? `${label} · ${address}${hosted}` : label}
        active={here && !params.server}
        unread={dms > 0}
        to="/$instance"
        params={{ instance: inst.key }}
        small
      >
        <span
          className={cn(
            "server-icon relative grid size-9 place-items-center bg-card text-[0.7rem] font-extrabold text-muted-foreground",
            inst.connection !== "live" && "opacity-70",
          )}
          data-active={here && !params.server}
        >
          {inst.node ? initials(inst.node.name) : <GlobeIcon className="size-4" />}
          <ConnDot state={inst.connection} className="absolute -right-0.5 -bottom-0.5 ring-2 ring-[color-mix(in_srgb,var(--background)_75%,black)]" />
          <AnimatePresence>
            {dms > 0 && (
              <motion.span
                title="Unread direct messages"
                initial={{ scale: 0 }}
                animate={{ scale: 1 }}
                exit={{ scale: 0 }}
                transition={{ type: "spring", stiffness: 600, damping: 18 }}
                className="absolute -top-1.5 -right-1.5 grid h-4 min-w-4 place-items-center rounded-full bg-destructive px-1 text-[0.6rem] font-extrabold text-white ring-[3px] ring-[color-mix(in_srgb,var(--background)_75%,black)]"
              >
                <Count value={dms} max={99} />
              </motion.span>
            )}
          </AnimatePresence>
        </span>
      </RailItem>
      <AnimatePresence initial={false} mode="popLayout">
        {inst.servers.map((server) => (
          <Pop key={server.id}>
            <ServerButton inst={inst} server={server} active={here && params.server === server.id} />
          </Pop>
        ))}
        {Object.values(inst.applied)
          .filter((a) => !inst.servers.some((s) => s.id === a.server.id))
          .map((a) => (
            <Pop key={`applied-${a.server.id}`}>
              <AppliedButton inst={inst} applied={a} />
            </Pop>
          ))}
      </AnimatePresence>
    </>
  );
}

function ServerButton({ inst, server, active }: { inst: InstanceState; server: Server; active: boolean }) {
  const now = useNow();
  const unread = useFuwa((s) => {
    const i = s.instances[inst.key];
    if (!i) return 0;
    let n = 0;
    for (const c of i.channels[server.id] ?? []) {
      if (i.unread[c.id] && !effectiveNotifications(i, server.id, c.id, now).muted) n += i.unread[c.id]!;
    }
    return n;
  });
  return (
    <RailItem label={server.name} active={active} unread={unread > 0} to="/$instance/$server" params={{ instance: inst.key, server: server.id }}>
      <span className="relative">
        <ServerIcon server={server} active={active} />
        <AnimatePresence>
          {unread > 0 && !active && (
            <motion.span
              initial={{ scale: 0 }}
              animate={{ scale: 1 }}
              exit={{ scale: 0 }}
              transition={{ type: "spring", stiffness: 600, damping: 18 }}
              className="absolute -right-1 -bottom-1 grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1 text-[0.65rem] font-extrabold text-white ring-[3px] ring-[color-mix(in_srgb,var(--background)_75%,black)]"
            >
              <Count value={unread} max={99} />
            </motion.span>
          )}
        </AnimatePresence>
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
