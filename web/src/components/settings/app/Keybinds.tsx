import { ChevronDownIcon, InfoIcon, PlusIcon, RotateCcwIcon, Trash2Icon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { type I18n, useI18n } from "@/i18n/react";
import {
  ACTIONS,
  GROUPS,
  actionById,
  actionName,
  bindingOf,
  comboOf,
  groupName,
  keycaps,
  modifiersOf,
  normalize,
  problemWith,
  type KeyAction,
} from "@/lib/keybinds";
import { getPrefs, setPrefs, usePrefs, type CustomKeybind } from "@/lib/prefs";
import { cn } from "@/lib/utils";
import { Keycaps } from "./common";

/** Where the Keybinds page's own rows sit for settings search. */
export const keybindSettings = (t: I18n["t"]) => [
  { id: "custom-keybinds", label: t("appsettings.keybinds.custom"), keywords: "add shortcut" },
  ...ACTIONS.map((a) => ({ id: `key-${a.id}`, label: actionName(t, a), keywords: "shortcut hotkey" })),
];

type Recording = { kind: "action"; id: string } | { kind: "custom"; id: string } | null;

let draftIds = 0;

export function Keybinds() {
  const { t } = useI18n();
  const p = usePrefs((x) => x);
  const [recording, setRecording] = useState<Recording>(null);
  const [draft, setDraft] = useState<CustomKeybind | null>(null);
  const changedAny = Object.keys(p.keybinds).length > 0 || p.customKeybinds.length > 0;

  function saveCustom(bind: CustomKeybind) {
    setPrefs((x) => {
      const exists = x.customKeybinds.some((c) => c.id === bind.id);
      return { customKeybinds: exists ? x.customKeybinds.map((c) => (c.id === bind.id ? bind : c)) : [...x.customKeybinds, bind] };
    });
  }

  return (
    <div className="flex flex-col gap-8">
      <p className="flex gap-2 rounded-xl bg-muted/60 px-3 py-2.5 text-sm text-muted-foreground">
        <InfoIcon className="mt-0.5 size-4 shrink-0" />
        <span>{t("appsettings.keybinds.about")}</span>
      </p>

      <section data-setting="custom-keybinds" className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-3">
          <div>
            <h3 className="font-extrabold">{t("appsettings.keybinds.custom")}</h3>
            <p className="text-sm text-muted-foreground">{t("appsettings.keybinds.customHint")}</p>
          </div>
          <Button
            type="button"
            size="sm"
            className="btn shrink-0 rounded-xl font-bold"
            disabled={!!draft}
            onClick={() => {
              const bind = { id: `new-${++draftIds}`, action: ACTIONS[0]!.id, combo: "" };
              setDraft(bind);
              setRecording({ kind: "custom", id: bind.id });
            }}
          >
            <PlusIcon /> {t("appsettings.keybinds.add")}
          </Button>
        </div>
        <div className="flex flex-col gap-2">
          <AnimatePresence initial={false} mode="popLayout">
            {[...p.customKeybinds, ...(draft ? [draft] : [])].map((bind) => (
              <CustomRow
                key={bind.id}
                bind={bind}
                recording={recording?.kind === "custom" && recording.id === bind.id}
                onRecording={(on) => {
                  setRecording(on ? { kind: "custom", id: bind.id } : null);
                  if (!on && draft?.id === bind.id) setDraft(null);
                }}
                onChange={(next) => {
                  if (draft?.id === bind.id) {
                    if (next.combo) {
                      saveCustom({ ...next, id: `custom-${Date.now().toString(36)}` });
                      setDraft(null);
                    } else setDraft(next);
                  } else saveCustom(next);
                }}
                onRemove={() => {
                  if (draft?.id === bind.id) setDraft(null);
                  else setPrefs((x) => ({ customKeybinds: x.customKeybinds.filter((c) => c.id !== bind.id) }));
                }}
              />
            ))}
          </AnimatePresence>
          {p.customKeybinds.length === 0 && !draft && <p className="rounded-xl border border-dashed px-3 py-3 text-sm text-muted-foreground">{t("appsettings.keybinds.noneYet")}</p>}
        </div>
      </section>

      {GROUPS.map((group) => (
        <section key={group} className="flex flex-col">
          <h3 className="mb-1 text-xs font-bold tracking-wide text-muted-foreground uppercase">{groupName(t, group)}</h3>
          {ACTIONS.filter((a) => a.group === group).map((action, n) => (
            <ActionRow
              key={action.id}
              action={action}
              index={n}
              recording={recording?.kind === "action" && recording.id === action.id}
              onRecording={(on) => setRecording(on ? { kind: "action", id: action.id } : null)}
            />
          ))}
        </section>
      ))}

      <AnimatePresence>
        {changedAny && (
          <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: 8 }} transition={SPRING}>
            <Button type="button" variant="outline" className="group rounded-xl" onClick={() => setPrefs({ keybinds: {}, customKeybinds: [] })}>
              <RotateCcwIcon className="transition-transform duration-500 group-hover:-rotate-[360deg]" /> {t("appsettings.keybinds.resetAll")}
            </Button>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function ActionRow({
  action,
  index,
  recording,
  onRecording,
}: {
  action: KeyAction;
  index: number;
  recording: boolean;
  onRecording: (on: boolean) => void;
}) {
  const { t } = useI18n();
  const p = usePrefs((x) => x);
  const combo = bindingOf(action, p);
  const changed = action.id in p.keybinds;
  const shake = useAnimationControls();
  const [problem, setProblem] = useState<string | null>(null);

  return (
    <motion.div
      data-setting={`key-${action.id}`}
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: index * 0.03 } }}
      className="border-b border-border/70 py-2.5 last:border-b-0"
    >
      <motion.div animate={shake} className="flex min-h-10 flex-wrap items-center gap-2">
        <span className="min-w-0 flex-1 text-sm font-bold">{actionName(t, action)}</span>
        <Recorder
          combo={combo}
          recording={recording}
          onRecording={(on) => {
            onRecording(on);
            if (on) setProblem(null);
          }}
          onRecord={(next) => {
            const why = problemWith(next, getPrefs(), { action: action.id }, t);
            if (why) {
              setProblem(why);
              void shake.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
              return false;
            }
            setProblem(null);
            // Recording the default again goes back to following the default.
            setPrefs((x) => ({ keybinds: next === action.combo ? without(x.keybinds, action.id) : { ...x.keybinds, [action.id]: next } }));
            return true;
          }}
        />
        <AnimatePresence initial={false} mode="popLayout">
          {changed && (
            <motion.span key="reset" initial={{ opacity: 0, scale: 0.6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.6 }} transition={SPRING}>
              <IconButton label={action.combo ? t("appsettings.keybinds.backTo", { keys: keycaps(action.combo).join(" ") }) : t("appsettings.keybinds.backToNone")} onClick={() => setPrefs((x) => ({ keybinds: without(x.keybinds, action.id) }))}>
                <RotateCcwIcon />
              </IconButton>
            </motion.span>
          )}
          {combo && (
            <motion.span key="remove" initial={{ opacity: 0, scale: 0.6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.6 }} transition={SPRING}>
              <IconButton label={t("appsettings.keybinds.removeShortcut")} onClick={() => setPrefs((x) => ({ keybinds: { ...x.keybinds, [action.id]: null } }))}>
                <XIcon />
              </IconButton>
            </motion.span>
          )}
        </AnimatePresence>
      </motion.div>
      <Problem text={problem} />
    </motion.div>
  );
}

function CustomRow({
  bind,
  recording,
  onRecording,
  onChange,
  onRemove,
}: {
  bind: CustomKeybind;
  recording: boolean;
  onRecording: (on: boolean) => void;
  onChange: (bind: CustomKeybind) => void;
  onRemove: () => void;
}) {
  const { t } = useI18n();
  const shake = useAnimationControls();
  const [problem, setProblem] = useState<string | null>(null);
  const action = actionById(bind.action);
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: -8, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, x: 20, transition: { duration: 0.15 } }}
      transition={SPRING}
      className="rounded-xl border bg-card px-3 py-2"
    >
      <motion.div animate={shake} className="flex flex-wrap items-center gap-2">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" className="group flex min-w-0 flex-1 items-center gap-1.5 rounded-lg px-2 py-1.5 text-left text-sm font-bold transition hover:bg-muted">
              <span className="truncate">{action ? actionName(t, action) : t("appsettings.keybinds.pickAction")}</span>
              <ChevronDownIcon className="size-4 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-180" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-80 w-64 overflow-y-auto">
            {GROUPS.map((group, g) => (
              <div key={group}>
                {g > 0 && <DropdownMenuSeparator />}
                <DropdownMenuLabel className="text-xs text-muted-foreground">{groupName(t, group)}</DropdownMenuLabel>
                {ACTIONS.filter((a) => a.group === group).map((a) => (
                  <DropdownMenuItem key={a.id} onSelect={() => onChange({ ...bind, action: a.id })}>
                    {actionName(t, a)}
                  </DropdownMenuItem>
                ))}
              </div>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <Recorder
          combo={bind.combo || null}
          recording={recording}
          onRecording={(on) => {
            onRecording(on);
            if (on) setProblem(null);
          }}
          onRecord={(next) => {
            const why = problemWith(next, getPrefs(), { custom: bind.id }, t);
            if (why) {
              setProblem(why);
              void shake.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
              return false;
            }
            setProblem(null);
            onChange({ ...bind, combo: next });
            return true;
          }}
        />
        <IconButton label={t("appsettings.keybinds.removeKeybind")} danger onClick={onRemove}>
          <Trash2Icon />
        </IconButton>
      </motion.div>
      <Problem text={problem} />
    </motion.div>
  );
}

/**
 * A shortcut you can re-record: click it, then press the keys. Keys pop in
 * as they're held; Escape stops without changing anything.
 */
function Recorder({
  combo,
  recording,
  onRecording,
  onRecord,
}: {
  combo: string | null;
  recording: boolean;
  onRecording: (on: boolean) => void;
  /** Tries a combo; false keeps recording (it was refused). */
  onRecord: (combo: string) => boolean;
}) {
  const { t } = useI18n();
  const [held, setHeld] = useState<string[]>([]);
  const button = useRef<HTMLButtonElement>(null);
  const latest = useRef({ onRecord, onRecording });
  latest.current = { onRecord, onRecording };

  useEffect(() => {
    if (!recording) return;
    setHeld([]);
    button.current?.focus();
    // Capture on the window, ahead of everything else, so the keys only record.
    const down = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      const bare = !e.shiftKey && !e.ctrlKey && !e.altKey && !e.metaKey;
      if (e.key === "Escape" && bare) return latest.current.onRecording(false);
      setHeld(modifiersOf(e));
      const next = comboOf(e);
      if (!next) return;
      setHeld([]);
      if (latest.current.onRecord(normalize(next))) latest.current.onRecording(false);
    };
    const up = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopImmediatePropagation();
      setHeld(modifiersOf(e));
    };
    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
    };
  }, [recording]);

  return (
    <button
      ref={button}
      type="button"
      onClick={() => onRecording(!recording)}
      onBlur={() => recording && onRecording(false)}
      aria-label={recording ? t("appsettings.keybinds.recording") : combo ? t("appsettings.keybinds.change", { keys: keycaps(combo).join(" ") }) : t("appsettings.keybinds.set")}
      className={cn(
        "group flex h-10 min-w-36 items-center justify-center gap-1.5 rounded-xl border px-3 text-sm transition-colors",
        recording ? "recording border-primary bg-primary/10 text-primary" : "hover:border-primary/50 hover:bg-muted/60",
      )}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        {recording ? (
          held.length ? (
            <motion.span key="held" initial={{ opacity: 0 }} animate={{ opacity: 1 }} className="flex items-center gap-1">
              <Keycaps combo={held.join("+")} />
              <span className="animate-pulse font-bold">+ …</span>
            </motion.span>
          ) : (
            <motion.span key="press" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING} className="font-bold">
              {t("appsettings.keybinds.pressKeys")}
            </motion.span>
          )
        ) : combo ? (
          <motion.span key={combo} initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: 1 }} transition={SPRING}>
            <Keycaps combo={combo} />
          </motion.span>
        ) : (
          <motion.span key="none" initial={{ opacity: 0 }} animate={{ opacity: 1 }} className="text-muted-foreground group-hover:text-foreground">
            {t("appsettings.keybinds.notSet")}
          </motion.span>
        )}
      </AnimatePresence>
    </button>
  );
}

function Problem({ text }: { text: string | null }) {
  return (
    <AnimatePresence initial={false}>
      {text && (
        <motion.p
          key={text}
          initial={{ opacity: 0, height: 0, y: -4 }}
          animate={{ opacity: 1, height: "auto", y: 0 }}
          exit={{ opacity: 0, height: 0 }}
          transition={SPRING}
          className="overflow-hidden pt-1.5 text-xs font-bold text-destructive"
        >
          {text}
        </motion.p>
      )}
    </AnimatePresence>
  );
}

function IconButton({ label, onClick, danger = false, children }: { label: string; onClick: () => void; danger?: boolean; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      className={cn(
        "grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:scale-110 active:scale-90 [&_svg]:size-4",
        danger ? "hover:bg-destructive/15 hover:text-destructive" : "hover:bg-muted hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

function without<V>(record: Record<string, V>, key: string): Record<string, V> {
  const { [key]: _, ...rest } = record;
  return rest;
}
