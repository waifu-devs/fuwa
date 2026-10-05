import * as DialogPrimitive from "@radix-ui/react-dialog";
import { ArrowLeftIcon, ChevronRightIcon, CornerDownRightIcon, SearchIcon, SearchXIcon, XIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { createContext, useCallback, useContext, useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { Count, EASE_OUT, SPRING } from "@/components/motion";
import { useI18n } from "@/i18n/react";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

export type SettingsSection = {
  id: string;
  label: string;
  icon: LucideIcon;
  /** A line under the section's heading. */
  description?: ReactNode;
  danger?: boolean;
  /** More words search should find this section by. */
  keywords?: string;
  /** Single settings on this section that search can jump to; each matches a `Setting` with the same id. */
  settings?: SettingEntry[];
  /** Something waiting there, such as applications to review. */
  badge?: number;
};

export type SettingEntry = { id: string; label: string; keywords?: string };

/** Sections under a heading. A group without one (such as signing out or deleting) sits apart at the end. */
export type SettingsGroup = { label?: string; sections: SettingsSection[] };

/**
 * Unsaved edits hold the screen: closing it (or, for edits that belong to one
 * section, leaving that section) shakes the save bar instead. Save bars
 * register themselves through this, so screens don't wire it by hand.
 */
type Guard = {
  setDirty: (id: string, scope: GuardScope | null) => void;
  nudge: number;
};
export type GuardScope = "section" | "screen";

const GuardContext = createContext<Guard | null>(null);

/** Marks the screen as holding unsaved edits; returns a counter that ticks when someone tries to leave. */
export function useUnsavedGuard(dirty: boolean, scope: GuardScope = "section") {
  const guard = useContext(GuardContext);
  const id = useId();
  const setDirty = guard?.setDirty;
  useEffect(() => {
    setDirty?.(id, dirty ? scope : null);
    return () => setDirty?.(id, null);
  }, [setDirty, id, dirty, scope]);
  return guard?.nudge ?? 0;
}

/**
 * Settings that take over the whole window, like Discord's: a side menu of
 * sections and the chosen one beside it. The app behind recedes as it opens.
 * On phones the menu is its own page and a section slides in over it.
 */
export function SettingsScreen({
  open,
  onOpenChange,
  title,
  subtitle,
  groups,
  section,
  onSectionChange,
  openToSection = false,
  children,
  footer,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  subtitle?: string;
  groups: SettingsGroup[];
  section: string;
  onSectionChange: (id: string) => void;
  /** On phones, open straight to `section` instead of the menu. */
  openToSection?: boolean;
  children: ReactNode;
  /** Shown under every section, such as a save bar for edits across sections. */
  footer?: ReactNode;
}) {
  const { t } = useI18n();
  const wide = useMediaQuery("(min-width: 768px)");
  const [menu, setMenu] = useState(true);
  const [query, setQuery] = useState("");
  const [glow, setGlow] = useState<{ id: string; n: number } | null>(null);
  const mainRef = useRef<HTMLElement>(null);
  const [dirty, setDirtyState] = useState<Record<string, GuardScope>>({});
  const [nudge, setNudge] = useState(0);

  useEffect(() => {
    if (open) {
      setMenu(!openToSection);
      setNudge(0);
      setQuery("");
    }
  }, [open, openToSection]);

  // A setting picked from search: once its section is on screen, scroll to it and let it glow.
  useEffect(() => {
    if (!glow) return;
    let frame = 0;
    const started = performance.now();
    const find = () => {
      const el = mainRef.current?.querySelector<HTMLElement>(`[data-setting="${CSS.escape(glow.id)}"]`);
      if (el) {
        el.scrollIntoView({ behavior: "smooth", block: "center" });
        el.classList.remove("found");
        void el.offsetWidth;
        el.classList.add("found");
        return;
      }
      if (performance.now() - started < 1500) frame = requestAnimationFrame(find);
    };
    frame = requestAnimationFrame(find);
    return () => cancelAnimationFrame(frame);
  }, [glow]);

  // The app behind shrinks back while settings are open.
  useEffect(() => {
    if (!open) return;
    document.body.dataset.settingsOpen = "";
    return () => {
      delete document.body.dataset.settingsOpen;
    };
  }, [open]);

  const setDirty = useCallback((id: string, scope: GuardScope | null) => {
    setDirtyState((current) => {
      if ((current[id] ?? null) === scope) return current;
      const next = { ...current };
      if (scope) next[id] = scope;
      else delete next[id];
      return next;
    });
  }, []);
  const guard = useMemo(() => ({ setDirty, nudge }), [setDirty, nudge]);
  const scopes = Object.values(dirty);
  const holdsScreen = scopes.length > 0;
  const holdsSection = scopes.includes("section");

  const hold = () => setNudge((n) => n + 1);
  function attemptClose() {
    if (holdsScreen) hold();
    else onOpenChange(false);
  }
  function choose(id: string, setting?: string) {
    if (id !== section && holdsSection) return hold();
    onSectionChange(id);
    setMenu(false);
    if (setting) setGlow({ id: setting, n: Date.now() });
  }

  const all = groups.flatMap((g) => g.sections);
  const current = all.find((s) => s.id === section) ?? all[0];
  const showMenu = wide || menu;
  const showSection = wide || !menu;

  return (
    <DialogPrimitive.Root open={open} onOpenChange={(next) => (next ? onOpenChange(true) : attemptClose())}>
      <AnimatePresence>
        {open && (
          <DialogPrimitive.Portal forceMount>
            <DialogPrimitive.Content
              asChild
              forceMount
              aria-describedby={undefined}
              onEscapeKeyDown={(e) => {
                // Escape clears a search before it closes settings.
                if (query) {
                  e.preventDefault();
                  setQuery("");
                }
              }}
            >
              <motion.div
                initial={{ opacity: 0, scale: 1.04 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 1.04 }}
                transition={{ duration: 0.28, ease: EASE_OUT }}
                className="fixed inset-0 z-50 flex bg-background text-foreground outline-none"
              >
                <GuardContext.Provider value={guard}>
                  {showMenu && (
                    <Menu
                      title={title}
                      subtitle={subtitle}
                      groups={groups}
                      section={wide ? current?.id : undefined}
                      onChoose={choose}
                      onClose={attemptClose}
                      wide={wide}
                      query={query}
                      onQuery={setQuery}
                    />
                  )}
                  {showSection && current && (
                    <main ref={mainRef} className="scroll-thin relative flex min-w-0 flex-[1_1_52rem] flex-col overflow-y-auto">
                      {!wide && (
                        <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-2 border-b bg-background/85 px-2 backdrop-blur">
                          <button
                            type="button"
                            onClick={() => (holdsSection ? hold() : setMenu(true))}
                            aria-label={t("settings.screen.allSettings")}
                            className="grid size-10 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted hover:text-foreground"
                          >
                            <ArrowLeftIcon className="size-5" />
                          </button>
                          <p className="min-w-0 flex-1 truncate font-extrabold">{current.label}</p>
                          <CloseButton onClose={attemptClose} compact />
                        </header>
                      )}
                      <div className="flex w-full max-w-[60rem] flex-1">
                        <div className="flex min-w-0 flex-1 flex-col px-4 pt-6 pb-4 sm:px-10 md:pt-16">
                          <AnimatePresence mode="wait" initial={false}>
                            <motion.div
                              key={current.id}
                              initial={{ opacity: 0, y: 14 }}
                              animate={{ opacity: 1, y: 0 }}
                              exit={{ opacity: 0, y: -8, transition: { duration: 0.12 } }}
                              transition={SPRING}
                              className="flex flex-col"
                            >
                              {wide && (
                                <div className="mb-6">
                                  <h2 className={cn("text-2xl font-extrabold tracking-tight", current.danger && "text-destructive")}>{current.label}</h2>
                                  {current.description && <p className="mt-1 text-sm text-muted-foreground">{current.description}</p>}
                                </div>
                              )}
                              {!wide && current.description && <p className="mb-4 text-sm text-muted-foreground">{current.description}</p>}
                              {children}
                            </motion.div>
                          </AnimatePresence>
                          <span className="flex-1" />
                          {footer}
                        </div>
                        {wide && (
                          <div className="sticky top-0 shrink-0 self-start pt-16 pr-6">
                            <CloseButton onClose={attemptClose} />
                          </div>
                        )}
                      </div>
                    </main>
                  )}
                </GuardContext.Provider>
              </motion.div>
            </DialogPrimitive.Content>
          </DialogPrimitive.Portal>
        )}
      </AnimatePresence>
    </DialogPrimitive.Root>
  );
}

function Menu({
  title,
  subtitle,
  groups,
  section,
  onChoose,
  onClose,
  wide,
  query,
  onQuery,
}: {
  title: string;
  subtitle?: string;
  groups: SettingsGroup[];
  section: string | undefined;
  onChoose: (id: string, setting?: string) => void;
  onClose: () => void;
  wide: boolean;
  query: string;
  onQuery: (query: string) => void;
}) {
  const { t } = useI18n();
  const highlight = useId();
  const results = useMemo(() => search(groups, query), [groups, query]);
  let n = 0;
  return (
    <aside
      className={cn(
        "surface-side scroll-thin flex shrink-0 justify-end overflow-y-auto",
        wide ? "flex-[1_0_15rem] border-r" : "w-full flex-col justify-start",
      )}
    >
      {!wide && (
        <header className="flex h-14 shrink-0 items-center gap-2 border-b px-4">
          <div className="min-w-0 flex-1">
            <DialogPrimitive.Title className="truncate font-extrabold">{title}</DialogPrimitive.Title>
            {subtitle && <p className="truncate text-xs text-muted-foreground">{subtitle}</p>}
          </div>
          <CloseButton onClose={onClose} compact />
        </header>
      )}
      <nav aria-label={t("settings.screen.title")} className={cn("flex flex-col gap-5", wide ? "w-60 py-16 pr-3 pl-5" : "p-3")}>
        {wide && (
          <div className="px-2">
            <DialogPrimitive.Title className="truncate text-lg font-extrabold">{title}</DialogPrimitive.Title>
            {subtitle && <p className="truncate text-xs text-muted-foreground">{subtitle}</p>}
          </div>
        )}
        <SearchBox
          query={query}
          onQuery={onQuery}
          onPick={() => {
            const first = results?.[0];
            if (first) onChoose(first.section.id, first.settings[0]?.id);
          }}
        />
        {results ? (
          <Results results={results} query={query} onChoose={onChoose} wide={wide} />
        ) : (
          groups.map((group, g) => (
          <div key={group.label ?? g} className={cn("flex flex-col gap-0.5", g > 0 && wide && "border-t border-border/70 pt-3")}>
            {group.label && <p className="mb-1 px-2 text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">{group.label}</p>}
            {group.sections.map((s) => {
              const active = s.id === section;
              const delay = n++ * 0.03;
              // Like Discord: plain rows on the desktop menu, icons only on
              // actions that stand apart; icons and chevrons on phones.
              const trailing = wide && !group.label;
              return (
                <motion.button
                  key={s.id}
                  type="button"
                  onClick={() => onChoose(s.id)}
                  aria-current={active ? "page" : undefined}
                  initial={{ opacity: 0, x: -10 }}
                  animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: 0.05 + delay } }}
                  whileTap={{ scale: 0.97 }}
                  className={cn(
                    "group relative flex items-center gap-2.5 rounded-lg text-left font-bold transition-colors",
                    wide ? "px-2.5 py-1.5 text-sm" : "px-3 py-3 text-base",
                    active
                      ? s.danger
                        ? "text-destructive"
                        : "text-primary"
                      : s.danger
                        ? "text-destructive/80 hover:bg-destructive/10 hover:text-destructive"
                        : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
                  )}
                >
                  {active && (
                    <motion.span
                      layoutId={`settings-section-${highlight}`}
                      transition={SPRING}
                      className={cn("absolute inset-0 rounded-lg", s.danger ? "bg-destructive/12" : "bg-primary/15")}
                    />
                  )}
                  {!wide && <s.icon className={cn(ICON, "size-5")} />}
                  <span className="relative truncate transition-transform duration-200 group-hover:translate-x-0.5">{s.label}</span>
                  <AnimatePresence initial={false}>
                    {!!s.badge && (
                      <motion.span
                        initial={{ scale: 0 }}
                        animate={{ scale: 1 }}
                        exit={{ scale: 0 }}
                        transition={{ type: "spring", stiffness: 600, damping: 18 }}
                        className="relative ml-auto grid h-5 min-w-5 shrink-0 place-items-center rounded-full bg-destructive px-1.5 text-[0.65rem] font-extrabold text-white"
                      >
                        <Count value={s.badge} max={99} />
                      </motion.span>
                    )}
                  </AnimatePresence>
                  {trailing && <s.icon className={cn(ICON, "ml-auto")} />}
                  {!wide && <ChevronRightIcon className="relative ml-auto size-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />}
                </motion.button>
              );
            })}
          </div>
          ))
        )}
      </nav>
    </aside>
  );
}

type Result = { section: SettingsSection; settings: SettingEntry[] };

const text = (value: ReactNode) => (typeof value === "string" ? value : "");

/** Sections and single settings whose words contain every word typed, or null with nothing typed. */
function search(groups: SettingsGroup[], query: string): Result[] | null {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return null;
  const hits = (haystack: string) => words.every((w) => haystack.toLowerCase().includes(w));
  const out: Result[] = [];
  for (const section of groups.flatMap((g) => g.sections)) {
    const own = `${section.label} ${text(section.description)} ${section.keywords ?? ""}`;
    const settings = (section.settings ?? []).filter((s) => hits(`${s.label} ${s.keywords ?? ""} ${section.label}`));
    if (settings.length || hits(own)) out.push({ section, settings });
  }
  return out;
}

function SearchBox({ query, onQuery, onPick }: { query: string; onQuery: (q: string) => void; onPick: () => void }) {
  const { t } = useI18n();
  return (
    <label className="group relative block">
      <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground transition group-focus-within:scale-110 group-focus-within:text-primary" />
      <input
        type="search"
        value={query}
        onChange={(e) => onQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            onPick();
          }
        }}
        placeholder={t("settings.screen.search")}
        aria-label={t("settings.screen.search")}
        className="h-9 w-full rounded-lg border border-transparent bg-muted/70 pr-8 pl-8 text-sm outline-none transition placeholder:text-muted-foreground focus:border-primary/50 focus:bg-background focus:ring-4 focus:ring-primary/10 [&::-webkit-search-cancel-button]:hidden"
      />
      <AnimatePresence>
        {query && (
          <motion.button
            type="button"
            aria-label={t("settings.screen.clearSearch")}
            onClick={() => onQuery("")}
            initial={{ opacity: 0, scale: 0.5, rotate: -90 }}
            animate={{ opacity: 1, scale: 1, rotate: 0 }}
            exit={{ opacity: 0, scale: 0.5, rotate: 90 }}
            transition={SPRING}
            className="absolute top-1/2 right-1.5 grid size-6 -translate-y-1/2 place-items-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground"
          >
            <XIcon className="size-3.5" />
          </motion.button>
        )}
      </AnimatePresence>
    </label>
  );
}

function Results({
  results,
  query,
  onChoose,
  wide,
}: {
  results: Result[];
  query: string;
  onChoose: (id: string, setting?: string) => void;
  wide: boolean;
}) {
  const { t } = useI18n();
  if (results.length === 0)
    return (
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={SPRING}
        className="flex flex-col items-center gap-2 px-2 py-6 text-center text-sm text-muted-foreground"
      >
        <motion.span animate={{ rotate: [0, -12, 10, -6, 0] }} transition={{ duration: 0.6, delay: 0.1 }}>
          <SearchXIcon className="size-7" />
        </motion.span>
        {t("settings.screen.noMatches", { query })}
      </motion.div>
    );
  let n = 0;
  return (
    <div className="flex flex-col gap-0.5" aria-live="polite">
      <AnimatePresence initial={false} mode="popLayout">
        {results.map(({ section, settings }) => (
          <motion.div key={section.id} layout="position" initial={{ opacity: 0, x: -10 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -10 }} transition={SPRING}>
            <ResultRow delay={n++} onClick={() => onChoose(section.id)} wide={wide} danger={section.danger}>
              <section.icon className={cn(ICON, "size-4")} />
              <span className="truncate">{section.label}</span>
            </ResultRow>
            {settings.map((s) => (
              <ResultRow key={s.id} delay={n++} onClick={() => onChoose(section.id, s.id)} wide={wide} nested>
                <CornerDownRightIcon className="size-3.5 shrink-0 opacity-60" />
                <span className="truncate">{s.label}</span>
              </ResultRow>
            ))}
          </motion.div>
        ))}
      </AnimatePresence>
    </div>
  );
}

function ResultRow({
  children,
  onClick,
  delay,
  wide,
  nested = false,
  danger = false,
}: {
  children: ReactNode;
  onClick: () => void;
  delay: number;
  wide: boolean;
  nested?: boolean;
  danger?: boolean;
}) {
  return (
    <motion.button
      type="button"
      onClick={onClick}
      initial={{ opacity: 0, x: -8 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(delay, 12) * 0.02 } }}
      whileTap={{ scale: 0.97 }}
      className={cn(
        "group flex w-full items-center gap-2 rounded-lg text-left font-bold transition-colors hover:bg-muted/70",
        wide ? "px-2.5 py-1.5 text-sm" : "px-3 py-2.5 text-base",
        nested ? "pl-6 text-[0.8rem] font-normal text-muted-foreground hover:text-foreground" : danger ? "text-destructive/80" : "text-foreground",
      )}
    >
      {children}
    </motion.button>
  );
}

const ICON = "relative size-4 shrink-0 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:-rotate-12 group-hover:scale-110";

function CloseButton({ onClose, compact = false }: { onClose: () => void; compact?: boolean }) {
  const { t } = useI18n();
  return (
    <button type="button" onClick={onClose} aria-label={t("settings.screen.close")} className="group flex shrink-0 flex-col items-center gap-1">
      <span
        className={cn(
          "grid place-items-center rounded-full text-muted-foreground transition group-hover:bg-muted group-hover:text-foreground group-active:scale-90",
          compact ? "size-10" : "size-10 border-2 group-hover:border-foreground/40",
        )}
      >
        <XIcon className="size-5 transition-transform duration-300 group-hover:rotate-90" />
      </span>
      {!compact && <span className="text-[0.65rem] font-bold text-muted-foreground">ESC</span>}
    </button>
  );
}
