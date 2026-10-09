import { Link, useNavigate } from "@tanstack/react-router";
import { ArrowRightIcon, ChevronLeftIcon, PlusIcon, SearchIcon, SparklesIcon, StarIcon, TicketIcon, TvMinimalPlayIcon, UsersIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState, type FormEvent } from "react";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { discover, run } from "@/fuwa/actions";
import { useInstance } from "@/fuwa/hooks";
import { BuildLabel } from "@/components/BuildLabel";
import { ContinueAs } from "@/components/AccountSwitcher";
import { Connect } from "@/components/Connect";
import { DesktopDownload } from "@/components/DesktopDownload";
import { CreateServerDialog } from "@/components/dialogs/CreateServerDialog";
import { ConnDot, ServerIcon } from "@/components/Icons";
import { connectionLabel } from "@/components/icons-utils";
import { JoinButton } from "@/components/join/JoinButton";
import { ServerBanner } from "@/components/join/Banner";
import { ServerDoor } from "@/components/join/ServerDoor";
import { accentVars } from "@/lib/banner";
import { InlineMarkdown } from "@/components/Markdown";
import { Count, SwapText, Tilt } from "@/components/motion";
import { Private, useAddress, usePrivateField } from "@/components/Private";
import { useLayout } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { parseInvite } from "@/lib/invites";
import { HostedBadge } from "@/components/HostedBadge";
import { T, useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

/** An instance's front page: sign in if needed, then browse and create servers. */
export function InstanceHome({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const { compact, setNavOpen } = useLayout();

  if (!inst) return <UnknownInstance instanceKey={instanceKey} />;

  if (inst.connection === "signed-out" || (!inst.me && inst.node && inst.connection !== "connecting")) {
    return (
      <Centered>
        <div className="w-full max-w-md rounded-3xl border bg-card p-6 shadow-xl sm:p-8">
          {inst.problem && <p className="mb-4 rounded-2xl bg-muted p-3 text-sm">{inst.problem}</p>}
          <ContinueAs instanceKey={instanceKey} />
          <Connect initialUrl={inst.url} />
        </div>
      </Centered>
    );
  }

  return (
    <div className="scroll-thin h-full overflow-y-auto">
      {compact && (
        <button type="button" onClick={() => setNavOpen(true)} className="m-2 flex items-center gap-1 rounded-full px-3 py-2 text-sm font-bold text-muted-foreground hover:bg-muted">
          <ChevronLeftIcon className="size-4" /> {t("workspace.home.servers")}
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
  const { t } = useI18n();
  // A streamer mode link (/~name) only means something on the device that made it.
  if (instanceKey.startsWith("~"))
    return (
      <Centered>
        <div className="flex w-full max-w-md flex-col items-center gap-3 rounded-3xl border bg-card p-6 text-center shadow-xl sm:p-8">
          <span className="float grid size-14 place-items-center rounded-full bg-primary/15 text-primary">
            <TvMinimalPlayIcon className="size-7" />
          </span>
          <h1 className="text-xl font-extrabold">{t("workspace.home.hiddenTitle")}</h1>
          <p className="text-sm text-muted-foreground">{t("workspace.home.hiddenAbout")}</p>
          <Button asChild className="btn mt-1 rounded-xl font-bold">
            <Link to="/">{t("workspace.home.goHome")}</Link>
          </Button>
        </div>
      </Centered>
    );
  return (
    <Centered>
      <div className="w-full max-w-md rounded-3xl border bg-card p-6 shadow-xl sm:p-8">
        <h1 className="mb-1 text-xl font-extrabold">
          <T k="workspace.home.connectTo" values={{ address: <Private text={instanceKey.replaceAll("~", "/")} /> }} />
        </h1>
        <p className="mb-5 text-sm text-muted-foreground">{t("workspace.home.notSignedIn")}</p>
        <Connect initialUrl={instanceKey.replaceAll("~", "/")} />
      </div>
    </Centered>
  );
}

function Browse({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey)!;
  const [found, setFound] = useState<{ servers: Server[]; featured: string[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [creating, setCreating] = useState(false);

  useEffect(() => {
    if (!inst.me) return;
    run(discover(instanceKey)).then(setFound, (e) => setError(e.message));
  }, [instanceKey, inst.me]);

  const q = query.trim().toLowerCase();
  const matching = (found?.servers ?? []).filter((s) => !q || s.name.toLowerCase().includes(q) || s.description.toLowerCase().includes(q));
  // The instance sends featured servers first, in their order.
  const featured = new Set(found?.featured);
  const shownFeatured = matching.filter((s) => featured.has(s.id));
  const shown = matching.filter((s) => !featured.has(s.id));

  return (
    <div className="mx-auto max-w-5xl px-4 pt-6 pb-16 sm:px-8">
      <BrowseHero instanceKey={instanceKey} onCreate={() => setCreating(true)} />

      <HaveInvite instanceKey={instanceKey} />

      <div className="mt-8 mb-4 flex flex-col gap-3 sm:flex-row sm:items-center sm:justify-between">
        <h2 className="text-lg font-extrabold">{t("workspace.home.browse")}</h2>
        <div className="relative sm:w-72">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("workspace.home.search")} className="h-10 rounded-xl pl-9" />
        </div>
      </div>

      <BrowseResults instanceKey={instanceKey} loaded={found !== null} error={error} searching={!!q} featured={shownFeatured} shown={shown} />
      <CreateServerDialog open={creating} onOpenChange={setCreating} defaultInstance={instanceKey} />
    </div>
  );
}

/** The instance's welcome: how it's connected, its name, and making a server. */
function BrowseHero({ instanceKey, onCreate }: { instanceKey: string; onCreate: () => void }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey)!;
  const address = useAddress(instanceKey);
  return (
    <section className="relative overflow-hidden rounded-3xl border bg-card p-6 sm:p-10">
      <div aria-hidden className="dot-grid absolute inset-0 opacity-60" />
      <div aria-hidden className="absolute -top-20 -right-16 size-64 rounded-full bg-primary/25 blur-3xl" />
      <div className="relative flex flex-col gap-4 sm:flex-row sm:items-end sm:justify-between">
        <div>
          <p className="flex flex-wrap items-center gap-x-2 gap-y-1.5 text-sm font-bold text-muted-foreground">
            <ConnDot state={inst.connection} /> {connectionLabel(inst.connection)} · <Private text={instanceKey} /> · <BuildLabel node={inst.node} />
            <HostedBadge url={inst.url} className="sm:ml-1" />
          </p>
          <h1 className="mt-2 text-3xl font-extrabold tracking-tight sm:text-4xl">
            <T k="workspace.home.welcome" values={{ name: <SwapText className="gradient-text">{inst.node?.name ?? address}</SwapText> }} />
          </h1>
          <p className="mt-2 max-w-xl text-muted-foreground">{t("workspace.home.about")}</p>
        </div>
        <div className="flex shrink-0 flex-wrap gap-2">
          <DesktopDownload node={inst.node} url={inst.url} />
          <Button size="lg" onClick={onCreate} className="btn h-11 shrink-0 rounded-xl font-bold">
            <PlusIcon /> {t("workspace.home.create")}
          </Button>
        </div>
      </div>
    </section>
  );
}

/**
 * The servers in Browse that match the search, gliding in and out as it
 * changes: the ones the instance features first and big, then the rest.
 */
function BrowseResults({
  instanceKey,
  loaded,
  error,
  searching,
  featured,
  shown,
}: {
  instanceKey: string;
  loaded: boolean;
  error: string | null;
  searching: boolean;
  featured: Server[];
  shown: Server[];
}) {
  const { t } = useI18n();
  if (error) return <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>;
  if (!loaded)
    return (
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {[0, 1, 2].map((n) => (
          <div key={n} className="shimmer h-44 rounded-3xl" />
        ))}
      </div>
    );
  return (
    <>
      <AnimatePresence initial={false}>
        {featured.length > 0 && (
          <motion.section
            key="featured"
            layout
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -12 }}
            transition={{ duration: 0.35, ease: [0.22, 1, 0.36, 1] }}
          >
            <p className="mb-3 flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
              <SparklesIcon className="size-3.5 text-primary" /> {t("workspace.home.featured")}
              <span className="font-normal tracking-normal normal-case">· {t("workspace.home.featuredAbout")}</span>
            </p>
            <motion.div layout className="mb-8 grid gap-4 md:grid-cols-2">
              <AnimatePresence>
                {featured.map((s, n) => (
                  <motion.div
                    key={s.id}
                    layout
                    initial={{ opacity: 0, y: 24, scale: 0.98 }}
                    animate={{ opacity: 1, y: 0, scale: 1, pointerEvents: "auto" }}
                    exit={{ opacity: 0, scale: 0.95, pointerEvents: "none" }}
                    transition={{ duration: 0.5, delay: Math.min(n, 6) * 0.07, ease: [0.22, 1, 0.36, 1], pointerEvents: { delay: 0 } }}
                    // With an odd number, the first takes a whole row.
                    className={cn(n === 0 && featured.length % 2 === 1 && "md:col-span-2")}
                  >
                    <FeaturedCard instanceKey={instanceKey} server={s} wide={n === 0 && featured.length % 2 === 1} />
                  </motion.div>
                ))}
              </AnimatePresence>
            </motion.div>
          </motion.section>
        )}
      </AnimatePresence>
      {featured.length === 0 && shown.length === 0 && (
        <div className="grid place-items-center rounded-3xl border border-dashed p-10 text-center">
          <p className="font-bold">{searching ? t("workspace.home.noMatch") : t("workspace.home.noServers")}</p>
          <p className="mt-1 text-sm text-muted-foreground">{searching ? t("workspace.home.tryAnother") : t("workspace.home.makeFirst")}</p>
        </div>
      )}
      <motion.div layout className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <AnimatePresence>
          {shown.map((s, n) => (
            <motion.div
              key={s.id}
              layout
              initial={{ opacity: 0, y: 20 }}
              animate={{ opacity: 1, y: 0, pointerEvents: "auto" }}
              exit={{ opacity: 0, scale: 0.95, pointerEvents: "none" }}
              // A leaving card stops taking clicks at once, whatever its stagger.
              transition={{ duration: 0.4, delay: Math.min(n, 8) * 0.05, ease: [0.22, 1, 0.36, 1], pointerEvents: { delay: 0 } }}
            >
              <ServerCard instanceKey={instanceKey} server={s} />
            </motion.div>
          ))}
        </AnimatePresence>
      </motion.div>
    </>
  );
}

/** Servers that stay out of Browse are joined by invite: paste a link (from any fuwa server) or a code. */
function HaveInvite({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [text, setText] = useState("");
  const [shake, setShake] = useState(0);
  const [bad, setBad] = useState(false);
  const privateField = usePrivateField();
  function submit(e: FormEvent) {
    e.preventDefault();
    const found = parseInvite(text, instanceKey);
    if (!found) {
      setBad(true);
      return setShake((n) => n + 1);
    }
    navigate({ to: "/$instance/invite/$code", params: { instance: found.instance, code: found.code } });
  }
  return (
    <form onSubmit={submit} className="mt-4 flex flex-col gap-3 rounded-3xl border bg-card/70 p-4 sm:flex-row sm:items-center">
      <span className="flex items-center gap-3 sm:w-64">
        <span className="grid size-10 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary">
          <TicketIcon className="size-5 -rotate-12" />
        </span>
        <span>
          <span className="block font-extrabold">{t("workspace.home.invite.title")}</span>
          <span className="block text-xs text-muted-foreground">{bad ? t("workspace.home.invite.bad") : t("workspace.home.invite.hint")}</span>
        </span>
      </span>
      <div key={shake} className={cn("flex flex-1 gap-2", shake > 0 && "shake")}>
        <Input
          value={text}
          onChange={(e) => {
            setText(e.target.value);
            setBad(false);
          }}
          placeholder="https://chat.example.com/invite/…"
          aria-label={t("workspace.home.invite.label")}
          aria-invalid={bad}
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          className={cn("h-10 flex-1 rounded-xl", privateField)}
        />
        <Button type="submit" disabled={!text.trim()} className="group h-10 rounded-xl font-bold">
          {t("workspace.home.invite.open")} <ArrowRightIcon className="transition-transform group-hover:translate-x-0.5" />
        </Button>
      </div>
    </form>
  );
}

function ServerCard({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const { t } = useI18n();
  const navigate = useNavigate();
  return (
    <Tilt className="card-pop tilt h-full overflow-hidden rounded-3xl border bg-card">
      <ServerBanner server={server} pan={false} className="h-20" />
      <div style={accentVars(server)} className="relative -mt-9 flex h-[calc(100%-2.75rem)] flex-col gap-3 p-5 pt-0">
        <div className="flex items-end gap-3">
          <ServerIcon server={server} active className="size-14 text-lg ring-4 ring-card" />
          <div className="min-w-0">
            <p className="truncate text-lg font-extrabold">{server.name}</p>
            <p className="flex items-center gap-1 text-xs text-muted-foreground">
              <UsersIcon className="size-3.5" /> <T k="workspace.home.members" values={{ count: <Count value={Number(server.memberCount)} /> }} count={Number(server.memberCount)} />
            </p>
          </div>
        </div>
        <p className="line-clamp-3 flex-1 text-sm text-muted-foreground">
          {server.description ? <InlineMarkdown>{server.description}</InlineMarkdown> : t("workspace.home.noDescription")}
        </p>
        <ServerDoor server={server} />
        <JoinButton
          instanceKey={instanceKey}
          server={server}
          onOpen={(s) => navigate({ to: "/$instance/$server", params: { instance: instanceKey, server: s.id } })}
        />
      </div>
    </Tilt>
  );
}

/**
 * A server the instance's admins feature: a tall banner that pans, the
 * icon and name bigger, and more of the description than a plain card.
 */
function FeaturedCard({ instanceKey, server, wide }: { instanceKey: string; server: Server; wide: boolean }) {
  const { t } = useI18n();
  const navigate = useNavigate();
  return (
    <Tilt max={wide ? 3 : 5} className="card-pop tilt h-full overflow-hidden rounded-3xl border border-primary/30 bg-card shadow-lg shadow-primary/5">
      <ServerBanner server={server} className={wide ? "h-40 sm:h-52" : "h-36 sm:h-44"}>
        <span className="absolute top-3 left-3 flex items-center gap-1 rounded-full bg-background/80 px-2.5 py-1 text-xs font-extrabold text-primary backdrop-blur">
          <StarIcon className="size-3.5 fill-current" /> {t("workspace.home.featured")}
        </span>
      </ServerBanner>
      <div style={accentVars(server)} className="relative -mt-12 flex flex-col gap-3 p-5 pt-0 sm:p-6 sm:pt-0">
        <div className="flex items-end gap-4">
          <ServerIcon server={server} active className="size-20 shrink-0 text-2xl ring-4 ring-card" />
          <div className="min-w-0 pb-1">
            <p className="truncate text-2xl font-extrabold tracking-tight">{server.name}</p>
            <p className="flex items-center gap-1 text-sm text-muted-foreground">
              <UsersIcon className="size-4" /> <T k="workspace.home.members" values={{ count: <Count value={Number(server.memberCount)} /> }} count={Number(server.memberCount)} />
            </p>
          </div>
        </div>
        <p className={cn("text-muted-foreground", wide ? "line-clamp-3 sm:text-base" : "line-clamp-4 text-sm")}>
          {server.description ? <InlineMarkdown>{server.description}</InlineMarkdown> : t("workspace.home.noDescription")}
        </p>
        <div className={cn("flex flex-col gap-3", wide && "sm:flex-row sm:items-end sm:justify-between")}>
          <ServerDoor server={server} />
          <div className={cn(wide && "sm:ml-auto sm:w-56 sm:shrink-0")}>
            <JoinButton
              instanceKey={instanceKey}
              server={server}
              onOpen={(s) => navigate({ to: "/$instance/$server", params: { instance: instanceKey, server: s.id } })}
            />
          </div>
        </div>
      </div>
    </Tilt>
  );
}
