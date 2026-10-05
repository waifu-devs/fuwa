import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { CompassIcon, FolderMinusIcon, FolderPlusIcon, GlobeIcon, PlusIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useMemo, useRef, useState, type ReactNode, type Ref } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { useFuwa } from "@/fuwa/store";
import { useRailInstances, type RailInstance } from "@/fuwa/hooks";
import { AddInstanceDialog } from "@/components/dialogs/AddInstanceDialog";
import { CreateServerDialog } from "@/components/dialogs/CreateServerDialog";
import { ConnDot, FuwaMark, ServerIcon } from "@/components/Icons";
import { AppliedButton } from "@/components/join/Applied";
import { Count, SPRING } from "@/components/motion";
import { Private, useAddress } from "@/components/Private";
import { useLayout } from "@/components/Shell";
import { useContextMenu } from "@/components/ContextMenu";
import { serverMenu } from "@/components/menus/server";
import type { MenuIcon } from "@/lib/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { initials } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";
import { effectiveNotifications, useNow } from "@/lib/notifications";
import { hostedByUs } from "@/lib/hosted";
import { arrangeServers, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { FolderBlock, FolderDialog, RailMenu, type RailMenuFor, type RailMenuTarget } from "@/components/RailFolder";
import { landingId, useRailArrange, useRailGlide } from "@/hooks/use-rail-arrange";
import { editFolder, folderLabel, folderOf, keyOf, moveRail, railLayout, stepRail, type RailFolder, type RailLayout } from "@/lib/rail";
import { isFolderOpen, setFolderOpen, useFoldersVersion } from "@/lib/rail-open";
import { reportUsage } from "@/lib/reports";
import { toast } from "@/lib/ui";

/**
 * The far-left column: every server you're in, grouped by the fuwa instance
 * it lives on, so hosted and self-hosted servers sit side by side.
 */
export function Rail() {
  const { t } = useI18n();
  const instances = useRailInstances();
  const params = useParams({ strict: false }) as { instance?: string; server?: string };
  const [creating, setCreating] = useState(false);
  const [connecting, setConnecting] = useState(false);
  const navigate = useNavigate();
  const signedIn = instances.filter((i) => i.me);
  // Instances below one whose servers were rearranged or a folder opened glide to their new place.
  useFoldersVersion();
  useFuwa((s) => s.order.map((k) => s.instances[k]?.rail?.length ?? -1).join());

  return (
    <nav
      aria-label={t("workspace.rail.label")}
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
                aria-label={t("workspace.rail.add")}
                className="server-icon group grid size-12 place-items-center bg-card text-primary transition-colors hover:bg-primary hover:text-primary-foreground"
              >
                <PlusIcon className="size-6 transition-transform duration-300 group-hover:rotate-90" />
              </button>
            </DropdownMenuTrigger>
          </TooltipTrigger>
          <TooltipContent side="right">{t("workspace.rail.add")}</TooltipContent>
        </Tooltip>
        <DropdownMenuContent side="right" align="start" className="w-64">
          <DropdownMenuItem disabled={signedIn.length === 0} onSelect={() => setCreating(true)}>
            <PlusIcon /> {t("workspace.rail.create")}
          </DropdownMenuItem>
          {signedIn.map((inst) => (
            <DropdownMenuItem
              key={inst.key}
              onSelect={() => navigate({ to: "/$instance", params: { instance: inst.key } })}
            >
              <CompassIcon /> <T k="workspace.rail.browseOn" values={{ instance: inst.node?.name ?? <Private text={inst.key} /> }} />
            </DropdownMenuItem>
          ))}
          <DropdownMenuItem onSelect={() => setConnecting(true)}>
            <GlobeIcon /> {t("workspace.rail.connect")}
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
function Pop({
  children,
  className,
  ref,
  still = false,
  delay = 0,
  glide = true,
}: {
  children: ReactNode;
  className?: string;
  ref?: Ref<HTMLDivElement>;
  /** Shows up in place, without popping in: a server just dragged here. */
  still?: boolean;
  delay?: number;
  /** Moves to new places by itself; off where `useRailGlide` moves things. */
  glide?: boolean;
}) {
  return (
    <motion.div
      ref={ref}
      layout={glide ? "position" : false}
      initial={still ? false : { opacity: 0, scale: 0.3 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.3 }}
      transition={{ ...SPRING, delay }}
      className={cn("flex w-full flex-col items-center gap-2", className)}
    >
      {children}
    </motion.div>
  );
}

/** Unread messages in some of an instance's servers, skipping muted channels and the server on screen. */
function useUnread(key: string, serverIds: string[], skip?: string) {
  const now = useNow();
  return useFuwa((s) => {
    const i = s.instances[key];
    if (!i) return 0;
    let n = 0;
    for (const id of serverIds) {
      if (id === skip) continue;
      for (const c of i.channels[id] ?? []) {
        if (i.unread[c.id] && !effectiveNotifications(i, id, c.id, now).muted) n += i.unread[c.id]!;
      }
    }
    return n;
  });
}

function InstanceGroup({ inst, params }: { inst: RailInstance; params: { instance?: string; server?: string } }) {
  const { t } = useI18n();
  const here = params.instance === inst.key;
  const address = useAddress(inst.key);
  const label = inst.node?.name ?? address;
  const hosted = hostedByUs(inst.url);
  // Unread direct messages show on the instance they're on.
  const dms = useFuwa((s) => Object.values(s.instances[inst.key]?.dms.unread ?? {}).reduce((sum, n) => sum + n, 0));
  return (
    <>
      <Divider />
      <RailItem
        label={
          inst.node
            ? hosted
              ? t("workspace.rail.instanceHosted", { name: label, address, hosted: t("shell.hosted.label") })
              : t("workspace.rail.instance", { name: label, address })
            : label
        }
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
                title={t("workspace.rail.unreadDms")}
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
      <ArrangedServers inst={inst} active={here ? params.server : undefined} />
    </>
  );
}

/**
 * An instance's servers in the order you arranged them, folders and all:
 * drag them around, drop one on another to make a folder, or move the one
 * in focus with Alt+↑ and Alt+↓.
 */
function ArrangedServers({ inst, active }: { inst: RailInstance; active?: string }) {
  const saved = useFuwa((s) => s.instances[inst.key]?.rail ?? null);
  const ids = useMemo(() => inst.servers.map((s) => s.id), [inst.servers]);
  const layout = useMemo(() => railLayout(saved, ids), [saved, ids]);
  const byId = useMemo(() => new Map(inst.servers.map((s) => [s.id, s])), [inst.servers]);
  const { t, number } = useI18n();
  const container = useRef<HTMLDivElement>(null);
  const [menu, setMenu] = useState<RailMenuTarget | null>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [said, setSaid] = useState("");
  const isOpen = (folder: string) => isFolderOpen(inst.key, folder);

  const arrange = (next: RailLayout, what: string) => {
    reportUsage(what);
    run(arrangeServers(inst.key, next)).catch((err: FuwaError) => toast(err.message));
  };
  useRailGlide(container);
  useRailArrange({
    container,
    enabled: !!inst.me && inst.connection === "live",
    layout,
    onArrange: (next, drop) => arrange(next, drop.kind === "combine" ? "rail.folder_create" : "rail.arrange"),
  });

  const nameOf = (id: string) => {
    const entry = layout.find((e) => e.kind === "folder" && e.folder.id === id);
    return entry?.kind === "folder" ? folderLabel(t, entry.folder, byId) : (byId.get(id)?.name ?? "");
  };
  /** Says where something went, for screen readers. */
  const announce = (next: RailLayout, id: string) => {
    const top = next.findIndex((e) => keyOf(e) === id);
    const folder = next.find((e) => e.kind === "folder" && e.folder.servers.includes(id));
    if (folder?.kind === "folder") {
      setSaid(
        t("workspace.rail.movedInFolder", {
          name: nameOf(id),
          position: number(folder.folder.servers.indexOf(id) + 1),
          count: number(folder.folder.servers.length),
          folder: folderLabel(t, folder.folder, byId),
        }),
      );
    } else {
      setSaid(t("workspace.rail.moved", { name: nameOf(id), position: number(top + 1), count: number(next.length) }));
    }
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown") || !inst.me) return;
    const row = (e.target as Element).closest<HTMLElement>("[data-rail]");
    if (!row) return;
    e.preventDefault();
    const id = row.dataset.id!;
    const next = stepRail(layout, id, e.key === "ArrowUp" ? -1 : 1, isOpen);
    if (!next) return;
    arrange(next, "rail.arrange_keys");
    announce(next, id);
    // Keeps focus on it, wherever it went.
    requestAnimationFrame(() => {
      const there = container.current?.querySelector<HTMLElement>(`[data-rail][data-id="${CSS.escape(id)}"]`);
      there?.querySelector<HTMLElement>("a, button")?.focus();
    });
  };

  const openMenu = (e: React.MouseEvent, target: RailMenuFor) => {
    if (!inst.me) return;
    e.preventDefault();
    // From the keyboard there's no pointer: the menu opens by the element.
    const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const x = e.clientX || r.right;
    const y = e.clientY || r.top;
    setMenu({ ...target, x, y });
    setMenuOpen(true);
  };

  const newFolder = (id: string) => {
    const next = folderOf(layout, id);
    arrange(next, "rail.folder_create");
    const made = next.find((e) => e.kind === "folder" && e.folder.servers[0] === id && e.folder.servers.length === 1);
    if (made?.kind === "folder") setFolderOpen(inst.key, made.folder.id, true);
  };
  const leaveFolder = (id: string) => {
    const at = layout.findIndex((e) => e.kind === "folder" && e.folder.servers.includes(id));
    const after = layout[at + 1];
    arrange(moveRail(layout, { kind: "server", id, folder: "", before: after ? keyOf(after) : null }), "rail.arrange");
  };

  const server = (id: string, folder: string, n = 0) => {
    const s = byId.get(id);
    if (!s) return null;
    return (
      <Pop key={id} still={landingId() === id} glide={false} delay={folder ? n * 0.03 : 0}>
        <div
          data-rail="server"
          data-rail-unit={folder ? undefined : ""}
          data-id={id}
          data-folder={folder}
          data-rail-lifted={landingId() === id ? "" : undefined}
          className="w-full"
        >
          <ServerButton
            inst={inst}
            server={s}
            active={active === id}
            folder={
              inst.me
                ? folder
                  ? { label: t("workspace.rail.takeOut"), icon: FolderMinusIcon, onSelect: () => leaveFolder(id) }
                  : { label: t("workspace.rail.newFolder"), icon: FolderPlusIcon, onSelect: () => newFolder(id) }
                : undefined
            }
          />
        </div>
      </Pop>
    );
  };
  const editingFolder = layout.find((e) => e.kind === "folder" && e.folder.id === editing);

  return (
    <div ref={container} onKeyDown={onKeyDown} className="relative flex w-full flex-col items-center gap-2">
      <AnimatePresence initial={false} mode="popLayout">
        {layout.map((e) =>
          e.kind === "server" ? (
            server(e.id, "")
          ) : (
            <Pop key={e.folder.id} still={landingId() === e.folder.id} glide={false}>
              <Folder
                inst={inst}
                folder={e.folder}
                servers={byId}
                active={active}
                onMenu={(ev) => openMenu(ev, { kind: "folder", id: e.folder.id })}
              >
                {e.folder.servers.map((id, n) => server(id, e.folder.id, n))}
              </Folder>
            </Pop>
          ),
        )}
        {Object.values(inst.applied)
          .filter((a) => !inst.servers.some((s) => s.id === a.server.id))
          .map((a) => (
            <Pop key={`applied-${a.server.id}`} glide={false}>
              <AppliedButton inst={inst} applied={a} />
            </Pop>
          ))}
      </AnimatePresence>
      <span aria-live="polite" className="sr-only">
        {said}
      </span>
      <RailMenu
        target={menu}
        open={menuOpen}
        onClose={() => setMenuOpen(false)}
        onToggle={(id) => setFolderOpen(inst.key, id, !isOpen(id))}
        onEdit={setEditing}
        onDissolve={(id) => arrange(editFolder(layout, id, null), "rail.folder_dissolve")}
        onNewFolder={newFolder}
        onLeaveFolder={leaveFolder}
      />
      <FolderDialog
        folder={editingFolder?.kind === "folder" ? editingFolder.folder : null}
        servers={byId}
        onOpenChange={(o) => !o && setEditing(null)}
        onSave={(name, color) => {
          if (editing) arrange(editFolder(layout, editing, { name, color }), "rail.folder_edit");
          setEditing(null);
        }}
      />
    </div>
  );
}

function Folder({
  inst,
  folder,
  servers,
  active,
  onMenu,
  children,
}: {
  inst: RailInstance;
  folder: RailFolder;
  servers: Map<string, Server>;
  active?: string;
  onMenu: (e: React.MouseEvent) => void;
  children: ReactNode;
}) {
  const unread = useUnread(inst.key, folder.servers, active);
  return (
    <FolderBlock
      instance={inst.key}
      folder={folder}
      servers={servers}
      unread={unread}
      activeInside={!!active && folder.servers.includes(active)}
      onMenu={onMenu}
    >
      {children}
    </FolderBlock>
  );
}

/** What a server's menu offers for your folders: put it in one, or take it out. */
type FolderItem = { label: string; icon: MenuIcon; onSelect: () => void };

function ServerButton({ inst, server, active, folder }: { inst: RailInstance; server: Server; active: boolean; folder?: FolderItem }) {
  const unread = useUnread(inst.key, [server.id]);
  const menu = useContextMenu("server", () => {
    const sections = serverMenu({ instanceKey: inst.key, server });
    if (!folder) return sections;
    // Your folders sit with the server's own settings, before its ID and Leave.
    const at = sections.findIndex((s) => s.id === "developer" || s.id === "danger");
    const entry = { id: "folder", items: [{ id: "folder", ...folder }] };
    return at === -1 ? [...sections, entry] : [...sections.slice(0, at), entry, ...sections.slice(at)];
  });
  return (
    <RailItem label={server.name} active={active} unread={unread > 0} to="/$instance/$server" params={{ instance: inst.key, server: server.id }} menu={menu}>
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
  menu,
}: {
  /** Right-click handlers, for a server's menu. */
  menu?: ReturnType<typeof useContextMenu>;
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
        className="absolute top-1/2 left-0 -mt-5 h-10 w-1 rounded-r-full bg-foreground"
        initial={false}
        animate={{ scaleY: height / 40, opacity: height ? 1 : 0 }}
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
            {...menu}
            className={cn(
              "rounded-[50%] outline-none focus-visible:ring-2 focus-visible:ring-ring data-[menu-open]:ring-2 data-[menu-open]:ring-primary/60",
              small && "my-0.5",
            )}
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
