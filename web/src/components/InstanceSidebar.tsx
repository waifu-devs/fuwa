import { Link } from "@tanstack/react-router";
import { CompassIcon, HashIcon, SettingsIcon, UsersIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState, type Ref } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { useFuwa, type InstanceState } from "@/fuwa/store";
import { ConnDot, ServerIcon } from "@/components/Icons";
import { connectionLabel } from "@/components/icons-utils";
import { Private, useAddress } from "@/components/Private";
import { useLayout } from "@/components/Shell";
import { Count, SwapText } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { waitingForYou } from "@/lib/friends";
import { UserPanel } from "@/components/UserPanel";
import { CallPanel } from "@/components/calls/CallPanel";
import { DmList } from "@/components/dm/DmList";
import { HostedBadge } from "@/components/HostedBadge";
import { lazyComponent } from "@/components/lazy";
import { useI18n } from "@/i18n/react";

const InstanceSettingsDialog = lazyComponent(
  () => import("@/components/settings/InstanceSettingsDialog").then((m) => m.InstanceSettingsDialog),
  (p) => p.open,
);

/** What the sidebar draws, the same object until one of these changes (not for every message). */
type SidebarView = Pick<InstanceState, "node" | "url" | "connection" | "admin" | "servers">;
const views = new WeakMap<InstanceState["servers"], SidebarView>();
function sidebarView(i: InstanceState): SidebarView {
  const v = views.get(i.servers);
  if (v && v.node === i.node && v.url === i.url && v.connection === i.connection && v.admin === i.admin) return v;
  const next = { node: i.node, url: i.url, connection: i.connection, admin: i.admin, servers: i.servers };
  views.set(i.servers, next);
  return next;
}

/** The sidebar on an instance's home: what it is, your conversations and your servers there. */
export function InstanceSidebar({ instanceKey }: { instanceKey: string }) {
  const inst = useFuwa((s) => {
    const i = s.instances[instanceKey];
    return i && sidebarView(i);
  });
  const { compact, setNavOpen } = useLayout();
  const [settings, setSettings] = useState(false);
  const address = useAddress(instanceKey);
  const { t } = useI18n();
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
            title={t("workspace.instanceSidebar.settings")}
            className="group grid size-8 shrink-0 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90"
          >
            <SettingsIcon className="size-4 transition-transform duration-500 ease-out group-hover:rotate-90" />
            <span className="sr-only">{t("workspace.instanceSidebar.settings")}</span>
          </button>
        )}
      </header>
      {inst.admin && <InstanceSettingsDialog open={settings} onOpenChange={setSettings} instanceKey={instanceKey} />}
      <div className="scroll-thin flex-1 overflow-y-auto p-2">
        <FriendsLink instanceKey={instanceKey} onOpen={() => compact && setNavOpen(false)} />
        <Link
          to="/$instance"
          params={{ instance: instanceKey }}
          activeOptions={{ exact: true }}
          onClick={() => compact && setNavOpen(false)}
          className="flex items-center gap-2 rounded-lg px-2 py-2 text-sm font-bold text-muted-foreground transition hover:bg-muted hover:text-foreground data-[status=active]:bg-primary/15 data-[status=active]:text-primary"
        >
          <CompassIcon className="size-4" /> {t("workspace.instanceSidebar.browse")}
        </Link>
        <DmList instanceKey={instanceKey} />
        <AnimatePresence initial={false}>
          {inst.servers.length > 0 && (
            <motion.p
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              className="mt-4 mb-1 px-2 text-xs font-bold tracking-wide text-muted-foreground uppercase"
            >
              {t("workspace.instanceSidebar.yourServers")}
            </motion.p>
          )}
        </AnimatePresence>
        <AnimatePresence mode="popLayout">
          {inst.servers.map((s, n) => (
            <ServerLink key={s.id} instanceKey={instanceKey} server={s} index={n} />
          ))}
        </AnimatePresence>
      </div>
      <CallPanel />
      <UserPanel instanceKey={instanceKey} />
    </>
  );
}

/** Friends, with how many requests wait for your answer. Hidden on instances from before friends. */
function FriendsLink({ instanceKey, onOpen }: { instanceKey: string; onOpen: () => void }) {
  const status = useFuwa((s) => s.instances[instanceKey]?.friends.status ?? "off");
  const waiting = useFuwa((s) => waitingForYou(s.instances[instanceKey]?.friends.list ?? []));
  const { t } = useI18n();
  if (status === "off" || status === "unsupported") return null;
  return (
    <Link
      to="/$instance/friends"
      params={{ instance: instanceKey }}
      onClick={onOpen}
      className="group flex items-center gap-2 rounded-lg px-2 py-2 text-sm font-bold text-muted-foreground transition hover:bg-muted hover:text-foreground data-[status=active]:bg-primary/15 data-[status=active]:text-primary"
    >
      <UsersIcon className="size-4 transition-transform duration-300 group-hover:scale-110" /> {t("workspace.instanceSidebar.friends")}
      <AnimatePresence>
        {waiting > 0 && (
          <motion.span
            key="waiting"
            initial={{ scale: 0 }}
            animate={{ scale: 1 }}
            exit={{ scale: 0 }}
            transition={{ type: "spring", stiffness: 600, damping: 18 }}
            title={t("workspace.instanceSidebar.requestsWaiting", { count: waiting })}
            className="ml-auto grid h-5 min-w-5 place-items-center rounded-full bg-primary px-1.5 text-[0.7rem] font-extrabold text-primary-foreground"
          >
            <Count value={waiting} max={99} />
          </motion.span>
        )}
      </AnimatePresence>
    </Link>
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
