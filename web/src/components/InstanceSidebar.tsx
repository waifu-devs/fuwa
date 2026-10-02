import { Link } from "@tanstack/react-router";
import { CompassIcon, HashIcon, SettingsIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState, type Ref } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { useInstance } from "@/fuwa/hooks";
import { ConnDot, ServerIcon, connectionLabel } from "@/components/Icons";
import { Private, useAddress } from "@/components/Private";
import { useLayout } from "@/components/Shell";
import { SPRING, SwapText } from "@/components/motion";
import { UserPanel } from "@/components/UserPanel";
import { HostedBadge } from "@/components/HostedBadge";
import { InstanceSettingsDialog } from "@/components/settings/InstanceSettingsDialog";

/** The sidebar on an instance's home: what it is, and your servers there. */
export function InstanceSidebar({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const { compact, setNavOpen } = useLayout();
  const [settings, setSettings] = useState(false);
  const address = useAddress(instanceKey);
  if (!inst) return null;
  return (
    <>
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-4">
        <div className="min-w-0 flex-1">
          <p className="flex min-w-0 items-center gap-1 font-extrabold">
            <SwapText className="truncate align-bottom">{inst.node?.name ?? address}</SwapText>
            <HostedBadge url={inst.url} variant="mark" />
          </p>
          <p className="flex items-center gap-1.5 truncate text-xs text-muted-foreground">
            <ConnDot state={inst.connection} /> {connectionLabel(inst.connection)} · <Private text={instanceKey} />
          </p>
        </div>
        {inst.admin && (
          <button
            type="button"
            onClick={() => setSettings(true)}
            title="Instance settings"
            className="group grid size-8 shrink-0 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90"
          >
            <SettingsIcon className="size-4 transition-transform duration-500 ease-out group-hover:rotate-90" />
            <span className="sr-only">Instance settings</span>
          </button>
        )}
      </header>
      {inst.admin && <InstanceSettingsDialog open={settings} onOpenChange={setSettings} instanceKey={instanceKey} />}
      <div className="scroll-thin flex-1 overflow-y-auto p-2">
        <Link
          to="/$instance"
          params={{ instance: instanceKey }}
          activeOptions={{ exact: true }}
          onClick={() => compact && setNavOpen(false)}
          className="flex items-center gap-2 rounded-lg px-2 py-2 text-sm font-bold text-muted-foreground transition hover:bg-muted hover:text-foreground data-[status=active]:bg-primary/15 data-[status=active]:text-primary"
        >
          <CompassIcon className="size-4" /> Browse servers
        </Link>
        <AnimatePresence initial={false}>
          {inst.servers.length > 0 && (
            <motion.p
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="mt-4 mb-1 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase"
            >
              Your servers
            </motion.p>
          )}
        </AnimatePresence>
        <AnimatePresence mode="popLayout">
          {inst.servers.map((s, n) => (
            <ServerLink key={s.id} instanceKey={instanceKey} server={s} index={n} />
          ))}
        </AnimatePresence>
      </div>
      <UserPanel instanceKey={instanceKey} />
    </>
  );
}

function ServerLink({
  instanceKey,
  server,
  index,
  ref,
}: {
  instanceKey: string;
  server: Server;
  index: number;
  ref?: Ref<HTMLDivElement>;
}) {
  return (
    <motion.div
      ref={ref}
      layout="position"
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 12) * 0.03 } }}
      exit={{ opacity: 0, x: -10, transition: { duration: 0.15 } }}
      transition={SPRING}
    >
      <Link
        to="/$instance/$server"
        params={{ instance: instanceKey, server: server.id }}
        className="group flex items-center gap-2 rounded-lg px-2 py-1.5 text-sm transition hover:bg-muted"
      >
        <ServerIcon server={server} className="size-7 text-[0.65rem] group-hover:-rotate-6 group-hover:scale-110" />
        <span className="truncate font-bold transition-transform duration-300 group-hover:translate-x-0.5">{server.name}</span>
        <HashIcon className="ml-auto size-3.5 -translate-x-1 text-muted-foreground opacity-0 transition group-hover:translate-x-0 group-hover:opacity-100" />
      </Link>
    </motion.div>
  );
}
