import * as DialogPrimitive from "@radix-ui/react-dialog";
import { useNavigate, useRouter, useRouterState } from "@tanstack/react-router";
import { ArrowDownIcon, ArrowUpIcon, CornerDownLeftIcon, EyeOffIcon, HashIcon, KeyboardIcon, SearchIcon, SparklesIcon, TvMinimalPlayIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Channel, Server } from "@/gen/fuwa/v1/types_pb";
import { markServerRead } from "@/fuwa/actions";
import { toggleCamera, toggleDeafen, toggleMute, toggleRecording, toggleScreen } from "@/calls/engine";
import { watchPushToTalk } from "@/calls/keys";
import { IncomingCalls } from "@/components/calls/IncomingCalls";
import { PopOuts, RecordingWatch } from "@/components/calls/Video";
import { store, useFuwa, type FuwaState } from "@/fuwa/store";
import { CHANNEL_ICON, openableChannels } from "@/components/ChannelSidebar";
import { ServerIcon } from "@/components/Icons";
import { Count, EASE_OUT, SPRING } from "@/components/motion";
import { Keycaps } from "@/components/settings/app/common";
import { fuzzy } from "@/lib/fuzzy";
import { ACTIONS, GROUPS, actionById, bindingOf, bindings, comboOf, composerKeys, normalize } from "@/lib/keybinds";
import { setDmNotificationTarget, setFriendsNotificationTarget, setNotificationTarget } from "@/lib/notify";
import { getPrefs, setPrefs, usePrefs } from "@/lib/prefs";
import { hidesPersonal, shownAddress } from "@/lib/streamer";
import {
  closeSettings,
  getUi,
  hideStreamerBanner,
  openSettings,
  runCommand,
  setShortcuts,
  setSwitcher,
  toast,
  useUi,
} from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * Everything that works from anywhere in the app: the keyboard shortcuts,
 * the shortcut sheet, the quick switcher and the little notes at the bottom.
 * Mounted once, around every page.
 */

type Here = { instance?: string; server?: string; channel?: string };

/** Where the app is now, read from the route. */
function useHere(): Here {
  const instance = useRouterState({ select: (s) => (s.matches.at(-1)?.params as Here | undefined)?.instance });
  const server = useRouterState({ select: (s) => (s.matches.at(-1)?.params as Here | undefined)?.server });
  const channel = useRouterState({ select: (s) => (s.matches.at(-1)?.params as Here | undefined)?.channel });
  return { instance, server, channel };
}

/** Every joined server, in the rail's order. */
function railServers(s: FuwaState) {
  return s.order.flatMap((key) => (s.instances[key]?.servers ?? []).map((server) => ({ instance: key, server })));
}

/** Every channel you can open, server by server in the rail's order. */
function allChannels(s: FuwaState) {
  return railServers(s).flatMap(({ instance, server }) =>
    openableChannels(s.instances[instance]?.channels[server.id] ?? []).map((channel) => ({ instance, server, channel })),
  );
}

const wrap = (n: number, length: number) => ((n % length) + length) % length;

const typing = (el: EventTarget | null) =>
  el instanceof HTMLElement && (el.isContentEditable || el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.tagName === "SELECT");

/** Actions that keep going while the keys are held, like walking down channels. */
const REPEATS = new Set(["previousServer", "nextServer", "previousChannel", "nextChannel", "previousUnread", "nextUnread"]);

export function AppOverlays() {
  const here = useHere();
  const navigate = useNavigate();
  const router = useRouter();
  const hereRef = useRef(here);
  hereRef.current = here;

  // Clicking a desktop notification opens its channel or conversation.
  useEffect(() => {
    setNotificationTarget((instance, server, channel) =>
      void navigate({ to: "/$instance/$server/$channel", params: { instance, server, channel } }),
    );
    setDmNotificationTarget((instance, conversation) => void navigate({ to: "/$instance/dm/$conversation", params: { instance, conversation } }));
    setFriendsNotificationTarget((instance) => void navigate({ to: "/$instance/friends", params: { instance } }));
  }, [navigate]);

  // Streamer mode swaps the address bar, and every link, between an instance's
  // host and its alias.
  const hidden = usePrefs(hidesPersonal);
  const wasHidden = useRef(hidden);
  useEffect(() => {
    const toggled = wasHidden.current !== hidden;
    wasHidden.current = hidden;
    // Handing the router its rewrite again makes it forget the addresses it built for links.
    if (toggled) router.update({ rewrite: { ...router.options.rewrite } } as never);
    const now = router.latestLocation;
    const shown = window.location.pathname + window.location.search + window.location.hash;
    if (router.buildLocation({ href: now.href } as never).publicHref === shown) return;
    // The same place with the mode in its history state, so the router writes the address again.
    void router.navigate({ href: now.href, replace: true, resetScroll: false, state: (s: object) => ({ ...s, streamer: hidden }) } as never);
  }, [hidden, router]);

  useEffect(() => {
    const go = (to: { instance: string; server: string; channel?: string }) =>
      to.channel
        ? navigate({ to: "/$instance/$server/$channel", params: { instance: to.instance, server: to.server, channel: to.channel } })
        : navigate({ to: "/$instance/$server", params: { instance: to.instance, server: to.server } });

    function stepServer(by: number) {
      const list = railServers(store.get());
      if (!list.length) return;
      const { instance, server } = hereRef.current;
      let at = list.findIndex((x) => x.instance === instance && x.server.id === server);
      // From an instance's home, the next server is its first one.
      if (at === -1) {
        const first = list.findIndex((x) => x.instance === instance);
        at = first === -1 ? (by > 0 ? -1 : 0) : by > 0 ? first - 1 : first;
      }
      const next = list[wrap(at + by, list.length)]!;
      void go({ instance: next.instance, server: next.server.id });
    }

    function stepChannel(by: number) {
      const { instance, server, channel } = hereRef.current;
      if (!instance || !server) return;
      const list = openableChannels(store.get().instances[instance]?.channels[server] ?? []);
      if (!list.length) return;
      const at = list.findIndex((c) => c.id === channel);
      const next = list[wrap(at === -1 ? (by > 0 ? 0 : -1) : at + by, list.length)]!;
      void go({ instance, server, channel: next.id });
    }

    function stepUnread(by: number) {
      const s = store.get();
      const list = allChannels(s);
      const { instance, channel } = hereRef.current;
      const at = list.findIndex((x) => x.instance === instance && x.channel.id === channel);
      for (let n = 1; n <= list.length; n++) {
        const x = list[wrap((at === -1 && by < 0 ? 0 : at) + by * n, list.length)]!;
        if ((s.instances[x.instance]?.unread[x.channel.id] ?? 0) > 0) return void go({ instance: x.instance, server: x.server.id, channel: x.channel.id });
      }
      toast("You're all caught up");
    }

    const run: Record<string, () => void> = {
      quickSwitcher: () => setSwitcher(!getUi().switcher),
      previousServer: () => stepServer(-1),
      nextServer: () => stepServer(1),
      previousChannel: () => stepChannel(-1),
      nextChannel: () => stepChannel(1),
      previousUnread: () => stepUnread(-1),
      nextUnread: () => stepUnread(1),
      markServerRead: () => {
        const { instance, server } = hereRef.current;
        if (!instance || !server) return;
        const name = store.get().instances[instance]?.servers.find((x) => x.id === server)?.name ?? "the server";
        toast(markServerRead(instance, server) > 0 ? `Marked ${name} as read` : `Nothing unread in ${name}`);
      },
      focusComposer: () => document.querySelector<HTMLTextAreaElement>("[data-composer]")?.focus(),
      toggleMembers: () => runCommand("toggleMembers"),
      toggleMute,
      toggleDeafen,
      toggleCamera: () => void toggleCamera(),
      toggleScreen: () => void toggleScreen(),
      toggleRecording,
      // Held down, not pressed: the call listens for it going down and up itself.
      pushToTalk: () => {},
      openSettings: () => (getUi().settings === null ? openSettings() : closeSettings()),
      shortcuts: () => setShortcuts(!getUi().shortcuts),
      toggleStreamer: () => {
        const on = !getPrefs().streamer;
        setPrefs({ streamer: on });
        if (on) hideStreamerBanner(false);
        toast(on ? "Streamer mode on" : "Streamer mode off");
      },
    };

    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const combo = comboOf(e);
      if (!combo) return;
      const id = bindings(getPrefs()).get(normalize(combo));
      const action = id && actionById(id);
      if (!action) return;
      if (e.repeat && !REPEATS.has(action.id)) return;
      if (typing(e.target) && !action.whileTyping) return;
      // Tab only jumps to the message box when nothing else has focus; otherwise it moves focus as usual.
      if (action.id === "focusComposer" && document.activeElement && document.activeElement !== document.body) return;
      // With a dialog or menu open, only the shortcuts that open or close app-wide screens work.
      const ui = getUi();
      if (document.querySelector('[role="dialog"], [role="menu"]')) {
        const allowed =
          action.id === "toggleStreamer" ||
          action.id === "toggleMute" ||
          action.id === "toggleDeafen" ||
          action.id === "toggleCamera" ||
          action.id === "toggleScreen" ||
          action.id === "toggleRecording" ||
          (action.id === "openSettings" && ui.settings !== null) ||
          (action.id === "shortcuts" && ui.shortcuts) ||
          (action.id === "quickSwitcher" && ui.switcher);
        if (!allowed) return;
      }
      e.preventDefault();
      run[action.id]?.();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [navigate]);

  useEffect(watchPushToTalk, []);

  return (
    <>
      <ShortcutSheet />
      <QuickSwitcher here={here} />
      <IncomingCalls />
      <PopOuts />
      <RecordingWatch />
      <Toaster />
    </>
  );
}

// ───────────────────────── The shortcut sheet ─────────────────────────

function ShortcutSheet() {
  const open = useUi((u) => u.shortcuts);
  const p = usePrefs((x) => x);
  let row = 0;
  return (
    <DialogPrimitive.Root open={open} onOpenChange={setShortcuts}>
      <AnimatePresence>
        {open && (
          <DialogPrimitive.Portal forceMount>
            <DialogPrimitive.Overlay asChild forceMount>
              <motion.div
                className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.2 }}
              />
            </DialogPrimitive.Overlay>
            <DialogPrimitive.Content asChild forceMount aria-describedby={undefined}>
              <motion.div
                initial={{ y: "100%" }}
                animate={{ y: 0 }}
                exit={{ y: "100%", transition: { duration: 0.22, ease: EASE_OUT } }}
                transition={{ type: "spring", stiffness: 420, damping: 38 }}
                className="scroll-thin fixed inset-x-0 bottom-0 z-50 mx-auto flex max-h-[85vh] w-full max-w-4xl flex-col overflow-y-auto rounded-t-3xl border border-b-0 bg-popover text-popover-foreground shadow-2xl outline-none"
              >
                <span aria-hidden className="mx-auto mt-2.5 h-1.5 w-10 shrink-0 rounded-full bg-muted-foreground/30" />
                <header className="flex items-center gap-3 px-5 pt-3 pb-2 sm:px-8">
                  <motion.span
                    initial={{ rotate: -20, scale: 0.6 }}
                    animate={{ rotate: 0, scale: 1 }}
                    transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.08 }}
                    className="grid size-10 place-items-center rounded-xl bg-primary text-primary-foreground"
                  >
                    <KeyboardIcon className="size-5" />
                  </motion.span>
                  <div className="min-w-0 flex-1">
                    <DialogPrimitive.Title className="text-lg font-extrabold">Keyboard shortcuts</DialogPrimitive.Title>
                    <p className="text-xs text-muted-foreground">They work in the browser and the app. Change any of them in settings.</p>
                  </div>
                  <DialogPrimitive.Close className="group grid size-9 place-items-center rounded-full text-muted-foreground transition hover:bg-muted hover:text-foreground" aria-label="Close">
                    <XIcon className="size-5 transition-transform duration-300 group-hover:rotate-90" />
                  </DialogPrimitive.Close>
                </header>
                <div className="grid gap-x-10 gap-y-6 px-5 py-4 sm:grid-cols-2 sm:px-8">
                  {GROUPS.map((group) => (
                    <section key={group} className="flex flex-col">
                      <h3 className="mb-1 text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">{group}</h3>
                      {ACTIONS.filter((a) => a.group === group).map((action) => {
                        const combo = bindingOf(action, p);
                        const extra = p.customKeybinds.filter((c) => c.action === action.id);
                        return (
                          <SheetRow key={action.id} index={row++} label={action.label}>
                            {combo ? <Keycaps combo={combo} /> : !extra.length && <span className="text-xs text-muted-foreground">Not set</span>}
                            {extra.map((c) => (
                              <Keycaps key={c.id} combo={c.combo} className="rounded-lg bg-primary/10 p-0.5" />
                            ))}
                          </SheetRow>
                        );
                      })}
                      {group === "Chat" &&
                        composerKeys(p.sendWith).map((k) => (
                          <SheetRow key={k.label} index={row++} label={k.label} muted>
                            <Keycaps combo={k.combo} />
                          </SheetRow>
                        ))}
                    </section>
                  ))}
                </div>
                <footer className="sticky bottom-0 flex flex-wrap items-center gap-3 border-t bg-popover/95 px-5 py-3 backdrop-blur sm:px-8">
                  <p className="min-w-0 flex-1 text-xs text-muted-foreground">
                    {bindingOf(actionById("shortcuts")!, p) ? (
                      <span className="inline-flex flex-wrap items-center gap-1">
                        Open this anytime with <Keycaps combo={bindingOf(actionById("shortcuts")!, p)!} />
                      </span>
                    ) : (
                      "Give this sheet a shortcut in Keybinds."
                    )}
                  </p>
                  <button
                    type="button"
                    onClick={() => openSettings("keybinds")}
                    className="group flex items-center gap-1.5 rounded-xl bg-primary px-3 py-2 text-sm font-bold text-primary-foreground transition hover:brightness-110 active:scale-95"
                  >
                    <SparklesIcon className="size-4 transition-transform group-hover:rotate-12" /> Change keybinds
                  </button>
                </footer>
              </motion.div>
            </DialogPrimitive.Content>
          </DialogPrimitive.Portal>
        )}
      </AnimatePresence>
    </DialogPrimitive.Root>
  );
}

function SheetRow({ label, index, muted = false, children }: { label: string; index: number; muted?: boolean; children: ReactNode }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: 0.06 + Math.min(index, 20) * 0.018 } }}
      className="flex min-h-10 items-center gap-3 border-b border-border/60 py-1.5 last:border-b-0"
    >
      <span className={cn("min-w-0 flex-1 text-sm", muted ? "text-muted-foreground" : "font-bold")}>{label}</span>
      <span className="flex shrink-0 flex-wrap justify-end gap-1.5">{children}</span>
    </motion.div>
  );
}

// ───────────────────────── The quick switcher ─────────────────────────

type Item = {
  id: string;
  kind: "channel" | "server";
  instance: string;
  server: Server;
  channel?: Channel;
  name: string;
  /** Where it lives, as shown under or beside the name. */
  where: string;
  unread: number;
  hits: number[];
  score: number;
};

function QuickSwitcher({ here }: { here: Here }) {
  const open = useUi((u) => u.switcher);
  return (
    <DialogPrimitive.Root open={open} onOpenChange={setSwitcher}>
      <AnimatePresence>
        {open && (
          <DialogPrimitive.Portal forceMount>
            <DialogPrimitive.Overlay asChild forceMount>
              <motion.div
                className="fixed inset-0 z-50 bg-black/45 backdrop-blur-[3px]"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.18 }}
              />
            </DialogPrimitive.Overlay>
            <div className="pointer-events-none fixed inset-0 z-50 flex justify-center px-3 pt-[12vh]">
              <DialogPrimitive.Content asChild forceMount aria-describedby={undefined}>
                <motion.div
                  initial={{ opacity: 0, scale: 0.94, y: -12 }}
                  animate={{ opacity: 1, scale: 1, y: 0 }}
                  exit={{ opacity: 0, scale: 0.96, y: -8, transition: { duration: 0.14 } }}
                  transition={SPRING}
                  className="pointer-events-auto flex max-h-[70vh] w-full max-w-xl flex-col self-start overflow-hidden rounded-2xl border bg-popover text-popover-foreground shadow-2xl outline-none"
                >
                  <DialogPrimitive.Title className="sr-only">Find a server or channel</DialogPrimitive.Title>
                  <Switcher here={here} />
                </motion.div>
              </DialogPrimitive.Content>
            </div>
          </DialogPrimitive.Portal>
        )}
      </AnimatePresence>
    </DialogPrimitive.Root>
  );
}

function Switcher({ here }: { here: Here }) {
  const s = useFuwa((x) => x);
  const hidden = usePrefs(hidesPersonal);
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const list = useRef<HTMLDivElement>(null);

  const items = useMemo(() => {
    const where = (key: string) => s.instances[key]?.node?.name ?? shownAddress(key);
    const mode = query.startsWith("#") ? "channel" : query.startsWith("*") ? "server" : null;
    const q = mode ? query.slice(1) : query;
    const out: Item[] = [];
    if (mode !== "server")
      for (const { instance, server, channel } of allChannels(s)) {
        if (instance === here.instance && channel.id === here.channel && !q) continue;
        const match = fuzzy(q, `${channel.name} ${server.name}`);
        if (!match) continue;
        const inName = match.hits.every((h) => h < channel.name.length);
        out.push({
          id: `c:${instance}:${channel.id}`,
          kind: "channel",
          instance,
          server,
          channel,
          name: channel.name,
          where: `${server.name} · ${where(instance)}`,
          unread: s.instances[instance]?.unread[channel.id] ?? 0,
          hits: match.hits.filter((h) => h < channel.name.length),
          score: match.score + (inName ? 4 : 0) + (instance === here.instance && server.id === here.server ? 1 : 0),
        });
      }
    if (mode !== "channel")
      for (const { instance, server } of railServers(s)) {
        const match = fuzzy(q, server.name);
        if (!match) continue;
        out.push({
          id: `s:${instance}:${server.id}`,
          kind: "server",
          instance,
          server,
          name: server.name,
          where: where(instance),
          unread: openableChannels(s.instances[instance]?.channels[server.id] ?? []).reduce((n, c) => n + (s.instances[instance]?.unread[c.id] ?? 0), 0),
          hits: match.hits,
          score: match.score + (q ? 0 : -1),
        });
      }
    // With nothing typed: unread channels first, then this server's, then servers.
    if (!q)
      return out
        .sort((a, b) => Number(b.kind === "channel" && b.unread > 0) - Number(a.kind === "channel" && a.unread > 0) || b.unread - a.unread || b.score - a.score)
        .slice(0, 40);
    return out.sort((a, b) => b.score - a.score || b.unread - a.unread).slice(0, 40);
    // Streamer mode (`hidden`) changes how places read.
  }, [s, query, here.instance, here.server, here.channel, hidden]);

  useEffect(() => setActive(0), [query]);
  useEffect(() => {
    list.current?.querySelector(`[data-index="${active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [active]);

  function pick(item: Item | undefined) {
    if (!item) return;
    setSwitcher(false);
    if (item.channel) void navigate({ to: "/$instance/$server/$channel", params: { instance: item.instance, server: item.server.id, channel: item.channel.id } });
    else void navigate({ to: "/$instance/$server", params: { instance: item.instance, server: item.server.id } });
  }

  return (
    <>
      <label className="flex items-center gap-3 border-b px-4">
        <SearchIcon className="size-5 shrink-0 text-muted-foreground" />
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" || e.key === "ArrowUp") {
              e.preventDefault();
              if (items.length) setActive((n) => wrap(n + (e.key === "ArrowDown" ? 1 : -1), items.length));
            } else if (e.key === "Enter") {
              e.preventDefault();
              pick(items[active]);
            }
          }}
          placeholder="Where to?"
          aria-label="Find a server or channel"
          aria-controls="switcher-results"
          aria-activedescendant={items[active] ? `switcher-${active}` : undefined}
          className="h-14 min-w-0 flex-1 bg-transparent text-lg outline-none placeholder:text-muted-foreground"
        />
      </label>
      <div ref={list} id="switcher-results" role="listbox" className="scroll-thin flex-1 overflow-y-auto p-2">
        {!query && items.length > 0 && (
          <p className="px-2 pt-1 pb-1.5 text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">
            {items[0]?.unread && items[0].kind === "channel" ? "Unread first" : "Jump to"}
          </p>
        )}
        {items.length === 0 ? (
          <motion.p initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} className="px-3 py-8 text-center text-sm text-muted-foreground">
            {query ? `Nothing called “${query}”` : "Join a server and its channels show up here."}
          </motion.p>
        ) : (
          items.map((item, n) => (
            <SwitcherRow key={item.id} item={item} index={n} active={n === active} onHover={() => setActive(n)} onPick={() => pick(item)} />
          ))
        )}
      </div>
      <footer className="flex flex-wrap items-center gap-x-4 gap-y-1 border-t bg-muted/40 px-4 py-2 text-[0.7rem] text-muted-foreground">
        <span className="flex items-center gap-1">
          <ArrowUpIcon className="size-3" />
          <ArrowDownIcon className="size-3" /> move
        </span>
        <span className="flex items-center gap-1">
          <CornerDownLeftIcon className="size-3" /> go
        </span>
        <span>
          <b className="text-foreground">#</b> channels only
        </span>
        <span>
          <b className="text-foreground">*</b> servers only
        </span>
      </footer>
    </>
  );
}

function SwitcherRow({ item, index, active, onHover, onPick }: { item: Item; index: number; active: boolean; onHover: () => void; onPick: () => void }) {
  const Icon = item.channel ? (CHANNEL_ICON[item.channel.type] ?? HashIcon) : null;
  return (
    <motion.button
      type="button"
      id={`switcher-${index}`}
      role="option"
      aria-selected={active}
      data-index={index}
      onPointerMove={onHover}
      onClick={onPick}
      initial={{ opacity: 0, x: -6 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 10) * 0.015 } }}
      className={cn("relative flex w-full items-center gap-3 rounded-xl px-2.5 py-2 text-left", active ? "text-foreground" : "text-muted-foreground")}
    >
      {active && <motion.span layoutId="switcher-active" transition={{ type: "spring", stiffness: 700, damping: 45 }} className="absolute inset-0 rounded-xl bg-primary/12" />}
      {Icon ? (
        <span className="relative grid size-8 shrink-0 place-items-center rounded-lg bg-muted">
          <Icon className="size-4" />
        </span>
      ) : (
        <ServerIcon server={item.server} className="relative size-8 text-[0.65rem]" />
      )}
      <span className="relative min-w-0 flex-1">
        <span className={cn("block truncate", (active || item.unread > 0) && "font-bold text-foreground")}>
          <Highlight text={item.name} hits={item.hits} />
        </span>
        <span className="block truncate text-xs text-muted-foreground">{item.where}</span>
      </span>
      {item.unread > 0 && (
        <span className="relative grid h-5 min-w-5 place-items-center rounded-full bg-destructive px-1.5 text-[0.7rem] font-extrabold text-white">
          <Count value={item.unread} max={99} />
        </span>
      )}
      <motion.span initial={false} animate={{ opacity: active ? 1 : 0, x: active ? 0 : -6 }} className="relative text-muted-foreground">
        <CornerDownLeftIcon className="size-4" />
      </motion.span>
    </motion.button>
  );
}

function Highlight({ text, hits }: { text: string; hits: number[] }) {
  if (!hits.length) return <>{text}</>;
  const set = new Set(hits);
  return (
    <>
      {Array.from(text).map((ch, n) =>
        set.has(n) ? (
          <b key={n} className="text-primary">
            {ch}
          </b>
        ) : (
          <span key={n}>{ch}</span>
        ),
      )}
    </>
  );
}

// ───────────────────────── Notes and banners ─────────────────────────

function Toaster() {
  const toasts = useUi((u) => u.toasts);
  return (
    <div aria-live="polite" className="pointer-events-none fixed inset-x-0 bottom-28 z-[60] flex flex-col items-center gap-2 px-4">
      <AnimatePresence initial={false}>
        {toasts.map((t) => (
          <motion.div
            key={t.id}
            layout
            initial={{ opacity: 0, y: 24, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.95, transition: { duration: 0.15 } }}
            transition={{ type: "spring", stiffness: 500, damping: 30 }}
            className="rounded-full bg-foreground px-4 py-2 text-sm font-bold text-background shadow-xl"
          >
            {t.text}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}

/** A strip across the top while streamer mode is on, so it's never on by surprise. */
export function StreamerBanner() {
  const on = usePrefs((p) => p.streamer);
  const hiddenBanner = useUi((u) => u.streamerBannerHidden);
  const show = on && !hiddenBanner;
  return (
    <AnimatePresence initial={false}>
      {show && (
        <motion.div
          initial={{ height: 0 }}
          animate={{ height: "auto" }}
          exit={{ height: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 40 }}
          className="shrink-0 overflow-hidden"
        >
          <div className="streamer-banner flex items-center justify-center gap-3 px-3 py-1.5 text-sm font-bold text-primary-foreground">
            <motion.span initial={{ scale: 0, rotate: -30 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.1 }}>
              <TvMinimalPlayIcon className="size-4" />
            </motion.span>
            <span className="truncate">Streamer mode is on</span>
            <span className="flex shrink-0 items-center gap-1">
              <button
                type="button"
                onClick={() => hideStreamerBanner()}
                className="flex items-center gap-1 rounded-full px-2.5 py-0.5 text-xs transition hover:bg-white/20 active:scale-95"
              >
                <EyeOffIcon className="size-3.5" /> Hide
              </button>
              <button
                type="button"
                onClick={() => setPrefs({ streamer: false })}
                className="rounded-full bg-white/25 px-2.5 py-0.5 text-xs transition hover:bg-white/35 active:scale-95"
              >
                Turn off
              </button>
            </span>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
