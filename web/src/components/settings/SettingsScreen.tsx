import * as DialogPrimitive from "@radix-ui/react-dialog";
import { ArrowLeftIcon, ChevronRightIcon, XIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { createContext, useCallback, useContext, useEffect, useId, useMemo, useState, type ReactNode } from "react";
import { EASE_OUT, SPRING } from "@/components/motion";
import { useMediaQuery } from "@/lib/use-media-query";
import { cn } from "@/lib/utils";

export type SettingsSection = {
  id: string;
  label: string;
  icon: LucideIcon;
  /** A line under the section's heading. */
  description?: ReactNode;
  danger?: boolean;
};

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
  const wide = useMediaQuery("(min-width: 768px)");
  const [menu, setMenu] = useState(true);
  const [dirty, setDirtyState] = useState<Record<string, GuardScope>>({});
  const [nudge, setNudge] = useState(0);

  useEffect(() => {
    if (open) {
      setMenu(!openToSection);
      setNudge(0);
    }
  }, [open, openToSection]);

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
  function choose(id: string) {
    if (id !== section && holdsSection) return hold();
    onSectionChange(id);
    setMenu(false);
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
            <DialogPrimitive.Content asChild forceMount aria-describedby={undefined}>
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
                    />
                  )}
                  {showSection && current && (
                    <main className="scroll-thin relative flex min-w-0 flex-[1_1_52rem] flex-col overflow-y-auto">
                      {!wide && (
                        <header className="sticky top-0 z-20 flex h-14 shrink-0 items-center gap-2 border-b bg-background/85 px-2 backdrop-blur">
                          <button
                            type="button"
                            onClick={() => (holdsSection ? hold() : setMenu(true))}
                            aria-label="All settings"
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
}: {
  title: string;
  subtitle?: string;
  groups: SettingsGroup[];
  section: string | undefined;
  onChoose: (id: string) => void;
  onClose: () => void;
  wide: boolean;
}) {
  const highlight = useId();
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
      <nav aria-label="Settings" className={cn("flex flex-col gap-5", wide ? "w-60 py-16 pr-3 pl-5" : "p-3")}>
        {wide && (
          <div className="px-2">
            <DialogPrimitive.Title className="truncate text-lg font-extrabold">{title}</DialogPrimitive.Title>
            {subtitle && <p className="truncate text-xs text-muted-foreground">{subtitle}</p>}
          </div>
        )}
        {groups.map((group, g) => (
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
                  {trailing && <s.icon className={cn(ICON, "ml-auto")} />}
                  {!wide && <ChevronRightIcon className="relative ml-auto size-4 text-muted-foreground transition-transform group-hover:translate-x-0.5" />}
                </motion.button>
              );
            })}
          </div>
        ))}
      </nav>
    </aside>
  );
}

const ICON = "relative size-4 shrink-0 transition duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:-rotate-12 group-hover:scale-110";

function CloseButton({ onClose, compact = false }: { onClose: () => void; compact?: boolean }) {
  return (
    <button type="button" onClick={onClose} aria-label="Close settings" className="group flex shrink-0 flex-col items-center gap-1">
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
