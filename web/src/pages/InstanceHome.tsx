import { Link, useNavigate } from "@tanstack/react-router";
import { ArrowRightIcon, CheckIcon, ChevronLeftIcon, LoaderCircleIcon, PlusIcon, SearchIcon, UsersIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { discover, joinServer, run } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { Connect } from "@/components/Connect";
import { CreateServerDialog } from "@/components/dialogs/CreateServerDialog";
import { ConnDot, ServerIcon, connectionLabel } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { Tilt } from "@/components/motion";
import { useLayout } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";

/** An instance's front page: sign in if needed, then browse and create servers. */
export function InstanceHome({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const { compact, setNavOpen } = useLayout();

  if (!inst) return <UnknownInstance instanceKey={instanceKey} />;

  if (inst.connection === "signed-out" || (!inst.me && inst.node && inst.connection !== "connecting")) {
    return (
      <Centered>
        <div className="w-full max-w-md rounded-3xl border bg-card p-6 shadow-xl sm:p-8">
          {inst.problem && <p className="mb-4 rounded-2xl bg-muted p-3 text-sm">{inst.problem}</p>}
          <Connect initialUrl={inst.url} />
        </div>
      </Centered>
    );
  }

  return (
    <div className="scroll-thin h-full overflow-y-auto">
      {compact && (
        <button type="button" onClick={() => setNavOpen(true)} className="m-2 flex items-center gap-1 rounded-full px-3 py-2 text-sm font-bold text-muted-foreground hover:bg-muted">
          <ChevronLeftIcon className="size-4" /> Servers
        </button>
      )}
      <Browse instanceKey={instanceKey} />
    </div>
  );
}

function Centered({ children }: { children: React.ReactNode }) {
  return <div className="grid h-full place-items-center overflow-y-auto p-4">{children}</div>;
}

function UnknownInstance({ instanceKey }: { instanceKey: string }) {
  return (
    <Centered>
      <div className="w-full max-w-md rounded-3xl border bg-card p-6 shadow-xl sm:p-8">
        <h1 className="mb-1 text-xl font-extrabold">Connect to {instanceKey.replaceAll("~", "/")}?</h1>
        <p className="mb-5 text-sm text-muted-foreground">You aren't signed in to this fuwa server in this browser yet.</p>
        <Connect initialUrl={instanceKey.replaceAll("~", "/")} />
      </div>
    </Centered>
  );
}

function Browse({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey)!;
  const [servers, setServers] = useState<Server[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);
  const joinedIds = useMemo(() => new Set(inst.servers.map((s) => s.id)), [inst.servers]);

  useEffect(() => {
    if (!inst.me) return;
    run(discover(instanceKey)).then(setServers, (e) => setError(e.message));
  }, [instanceKey, inst.me]);

  const q = query.trim().toLowerCase();
  const shown = (servers ?? []).filter((s) => !q || s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q));

  return (
    <div className="mx-auto max-w-5xl px-4 pt-6 pb-16 sm:px-8">
      <section className="relative overflow-hidden rounded-3xl border bg-card p-6 sm:p-10">
        <div aria-hidden className="dot-grid absolute inset-0 opacity-60" />
        <div aria-hidden className="absolute -top-20 -right-16 size-64 rounded-full bg-primary/25 blur-3xl" />
        <div className="relative flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
          <div>
            <p className="flex items-center gap-2 text-sm font-bold text-muted-foreground">
              <ConnDot state={inst.connection} /> {connectionLabel(inst.connection)} · {instanceKey} · fuwa {inst.node?.version}
            </p>
            <h1 className="mt-2 text-3xl font-extrabold tracking-tight sm:text-4xl">
              Welcome to <span className="gradient-text">{inst.node?.name ?? instanceKey}</span>
            </h1>
            <p className="mt-2 max-w-xl text-muted-foreground">
              Find a community to join, or start your own. Everything here lives on this fuwa server.
            </p>
          </div>
          <Button size="lg" onClick={() => setCreating(true)} className="btn h-11 shrink-0 rounded-xl font-bold">
            <PlusIcon /> Create a server
          </Button>
        </div>
      </section>

      <div className="mt-8 mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <h2 className="text-lg font-extrabold">Browse servers</h2>
        <div className="relative sm:w-72">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search" className="h-10 rounded-xl pl-9" />
        </div>
      </div>

      {error ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>
      ) : servers === null ? (
        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {[0, 1, 2].map((n) => (
            <div key={n} className="shimmer h-44 rounded-3xl" />
          ))}
        </div>
      ) : shown.length === 0 ? (
        <div className="grid place-items-center rounded-3xl border border-dashed p-10 text-center">
          <p className="font-bold">{q ? "Nothing matches that." : "No public servers here yet."}</p>
          <p className="mt-1 text-sm text-muted-foreground">
            {q ? "Try another word." : "Make the first one, and turn on “Show in Browse” so people can find it."}
          </p>
        </div>
      ) : (
        <motion.div layout className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          <AnimatePresence>
            {shown.map((s, n) => (
              <motion.div
                key={s.id}
                layout
                initial={{ opacity: 0, y: 20 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, scale: 0.95 }}
                transition={{ duration: 0.4, delay: Math.min(n, 8) * 0.05, ease: [0.22, 1, 0.36, 1] }}
              >
                <ServerCard instanceKey={instanceKey} server={s} joined={joinedIds.has(s.id)} />
              </motion.div>
            ))}
          </AnimatePresence>
        </motion.div>
      )}
      <CreateServerDialog open={creating} onOpenChange={setCreating} defaultInstance={instanceKey} />
    </div>
  );
}

function ServerCard({ instanceKey, server, joined }: { instanceKey: string; server: Server; joined: boolean }) {
  const navigate = useNavigate();
  const join = useAction(joinServer);
  const [justJoined, setJustJoined] = useState(false);
  async function onJoin() {
    const s = await join.go(instanceKey, server.id);
    if (!s) return;
    setJustJoined(true);
    setTimeout(() => navigate({ to: "/$instance/$server", params: { instance: instanceKey, server: s.id } }), 550);
  }
  return (
    <Tilt className="card-pop tilt h-full rounded-3xl border bg-card">
      <div className="flex h-full flex-col gap-3 p-5">
        <div className="flex items-center gap-3">
          <ServerIcon server={server} active className="size-14 text-lg" />
          <div className="min-w-0">
            <p className="truncate text-lg font-extrabold">{server.name}</p>
            <p className="flex items-center gap-1 text-xs text-muted-foreground">
              <UsersIcon className="size-3.5" /> {Number(server.memberCount).toLocaleString()} {server.memberCount === 1n ? "member" : "members"}
            </p>
          </div>
        </div>
        <p className="line-clamp-3 flex-1 text-sm text-muted-foreground">
          {server.description ? <InlineMarkdown>{server.description}</InlineMarkdown> : "No description yet."}
        </p>
        {join.error && <p className="text-xs text-destructive first-letter:uppercase">{join.error}</p>}
        {joined && !justJoined ? (
          <Button asChild variant="outline" className="rounded-xl font-bold">
            <Link to="/$instance/$server" params={{ instance: instanceKey, server: server.id }}>
              Open <ArrowRightIcon />
            </Link>
          </Button>
        ) : (
          <Button onClick={onJoin} disabled={join.pending || justJoined} className="btn rounded-xl font-bold">
            <AnimatePresence mode="wait" initial={false}>
              <motion.span
                key={justJoined ? "done" : join.pending ? "busy" : "join"}
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, y: -6 }}
                className="flex items-center gap-2"
              >
                {justJoined ? <CheckIcon /> : join.pending ? <LoaderCircleIcon className="animate-spin" /> : null}
                {justJoined ? "Joined" : "Join"}
              </motion.span>
            </AnimatePresence>
          </Button>
        )}
      </div>
    </Tilt>
  );
}
