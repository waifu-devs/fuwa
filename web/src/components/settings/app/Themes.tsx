import {
  ArrowLeftIcon,
  CheckIcon,
  CopyIcon,
  DownloadIcon,
  EllipsisIcon,
  FileUpIcon,
  PaintbrushIcon,
  PencilIcon,
  PlusIcon,
  SparklesIcon,
  Trash2Icon,
  TriangleAlertIcon,
  WandSparklesIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useRef, useState, type DragEvent, type MouseEvent, type ReactNode } from "react";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import { keepBackground, run, uploadPicture } from "@/fuwa/actions";
import { SPRING } from "@/components/motion";
import { BackdropForm, Labeled } from "@/components/settings/app/BackdropForm";
import { ThemePreview } from "@/components/settings/app/ThemePreview";
import { Toggle, WithPreview } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { DEFAULT_BACKDROP, type Backdrop } from "@/lib/backdrop";
import { activeTheme, deleteCustomTheme, getPrefs, saveCustomTheme, usePrefs } from "@/lib/prefs";
import { shownPicture } from "@/lib/shown";
import {
  blobToDataUrl,
  DESCRIPTION_MAX,
  fileName,
  MAX_CUSTOM_THEMES,
  NAME_MAX,
  parseThemeFile,
  themeFrom,
  ThemeFileError,
  toFile,
  type CustomTheme,
} from "@/lib/theme-file";
import { clickPoint, switchTheme } from "@/lib/theme-switch";
import { BUILTIN_THEMES, deriveTokens, HEX, RADIUS_MAX, RADIUS_MIN, SEEDS, seedsOf, TOKEN_LABELS, TOKENS, type Theme, type Token } from "@/lib/themes";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * Custom themes: make one from any theme, tune its colors, corners and
 * backdrop with a live preview, and pass it around as a file. They're app
 * settings, so they live on this device and work on every instance.
 */
export function Themes({ instanceKey }: { instanceKey?: string }) {
  const [draft, setDraft] = useState<{ theme: CustomTheme; isNew: boolean } | null>(null);
  return (
    <AnimatePresence mode="wait" initial={false}>
      {draft ? (
        <motion.div key="editor" initial={{ opacity: 0, x: 24 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 24 }} transition={SPRING}>
          <Editor instanceKey={instanceKey} start={draft.theme} isNew={draft.isNew} onClose={() => setDraft(null)} />
        </motion.div>
      ) : (
        <motion.div key="list" initial={{ opacity: 0, x: -24 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -24 }} transition={SPRING}>
          <Library instanceKey={instanceKey} onEdit={(theme, isNew) => setDraft({ theme, isNew })} />
        </motion.div>
      )}
    </AnimatePresence>
  );
}

// ───────────────────────── Your themes ─────────────────────────

function Library({ instanceKey, onEdit }: { instanceKey?: string; onEdit: (theme: CustomTheme, isNew: boolean) => void }) {
  const custom = usePrefs((p) => p.customThemes);
  const current = usePrefs((p) => activeTheme(p).id);
  const [notes, setNotes] = useState<{ name: string; lines: string[] } | null>(null);
  const [importing, setImporting] = useState(false);
  const [dragging, setDragging] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const full = custom.length >= MAX_CUSTOM_THEMES;

  const create = (base: Theme) => onEdit(themeFrom(base, base.builtin ? `My ${base.name}` : `${base.name} copy`, "backdrop" in base ? (base as CustomTheme).backdrop : null), true);

  async function importFile(file: File | undefined) {
    if (!file || full) return;
    setImporting(true);
    setNotes(null);
    try {
      if (file.size > 20 * 1024 * 1024) throw new ThemeFileError("That file is too big to be a theme.");
      const { theme, picture, notes: lines } = parseThemeFile(await file.text());
      if (picture && theme.backdrop) {
        if (instanceKey) {
          const url = await run(uploadPicture(instanceKey, MediaPurpose.BACKGROUND, picture));
          theme.backdrop.image = (await run(keepBackground(instanceKey, url))).url;
        } else {
          lines.push("Its background picture needs an instance to live on; open one you're signed in to and import it again.");
        }
      }
      saveCustomTheme(theme);
      toast(`Imported ${theme.name}`);
      if (lines.length) setNotes({ name: theme.name, lines });
    } catch (err) {
      setNotes({ name: file.name, lines: [(err as Error).message] });
    } finally {
      setImporting(false);
    }
  }

  const drop = {
    onDragOver: (e: DragEvent) => {
      if (!e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      setDragging(true);
    },
    onDragLeave: (e: DragEvent) => !e.currentTarget.contains(e.relatedTarget as Node | null) && setDragging(false),
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      setDragging(false);
      void importFile(e.dataTransfer.files[0]);
    },
  };

  return (
    <div className="relative flex flex-col gap-8" {...drop}>
      <section className="flex flex-col gap-4">
        <div className="flex flex-wrap items-end justify-between gap-3">
          <div>
            <h3 className="font-extrabold">Your themes</h3>
            <p className="mt-0.5 text-sm text-muted-foreground">Made, tuned or imported here. They work on every instance you use on this device.</p>
          </div>
          <div className="flex gap-2">
            <Button type="button" variant="outline" size="sm" disabled={importing || full} onClick={() => input.current?.click()}>
              <FileUpIcon className="size-4" />
              {importing ? "Importing…" : "Import"}
            </Button>
            <Button type="button" size="sm" disabled={full} onClick={() => create(activeTheme())} className="group">
              <PlusIcon className="size-4 transition-transform group-hover:rotate-90" />
              New theme
            </Button>
          </div>
        </div>

        <AnimatePresence>
          {notes && (
            <motion.div
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={SPRING}
              className="overflow-hidden"
            >
              <div className="flex gap-3 rounded-2xl border border-amber-500/40 bg-amber-500/10 p-3 text-sm">
                <TriangleAlertIcon className="mt-0.5 size-4 shrink-0 text-amber-600 dark:text-amber-400" />
                <div className="min-w-0 flex-1">
                  <p className="font-bold">{notes.name}</p>
                  {notes.lines.map((line) => (
                    <p key={line} className="text-muted-foreground">
                      {line}
                    </p>
                  ))}
                </div>
                <button type="button" className="self-start text-xs font-bold text-muted-foreground hover:text-foreground" onClick={() => setNotes(null)}>
                  Got it
                </button>
              </div>
            </motion.div>
          )}
        </AnimatePresence>

        {custom.length === 0 ? (
          <Empty onCreate={() => create(activeTheme())} />
        ) : (
          <div className="grid gap-3 sm:grid-cols-2">
            <AnimatePresence initial={false}>
              {custom.map((theme, n) => (
                <ThemeCard
                  key={theme.id}
                  theme={theme}
                  index={n}
                  active={theme.id === current}
                  onUse={(e) => switchTheme({ theme: theme.id, followSystem: false }, clickPoint(e))}
                  onEdit={() => onEdit(theme, false)}
                  onDuplicate={() => !full && create(theme)}
                  onExport={() => void exportTheme(theme)}
                  onDelete={() => deleteCustomTheme(theme.id)}
                />
              ))}
            </AnimatePresence>
          </div>
        )}
        {full && <p className="text-xs text-muted-foreground">That's {MAX_CUSTOM_THEMES} themes, as many as fuwa keeps. Delete one to add another.</p>}
      </section>

      <section className="flex flex-col gap-3">
        <div>
          <h3 className="font-extrabold">Start from a built-in</h3>
          <p className="mt-0.5 text-sm text-muted-foreground">Make your own version of one of the Waifu Devs themes.</p>
        </div>
        <div className="flex flex-wrap gap-2">
          {BUILTIN_THEMES.map((theme) => {
            const t = theme.variant.tokens;
            return (
              <motion.button
                key={theme.id}
                type="button"
                whileHover={{ y: -2 }}
                whileTap={{ scale: 0.95 }}
                disabled={full}
                onClick={() => create(theme)}
                style={{ background: t.background, color: t.foreground, borderColor: t.border }}
                className="group flex items-center gap-2 rounded-full border py-1.5 pr-3 pl-1.5 text-sm font-bold"
              >
                <span className="grid size-6 place-items-center rounded-full" style={{ background: t.primary, color: t["primary-foreground"] }}>
                  <PaintbrushIcon className="size-3.5 transition-transform group-hover:-rotate-12" />
                </span>
                {theme.name}
              </motion.button>
            );
          })}
        </div>
      </section>

      <AnimatePresence>
        {dragging && (
          <motion.div
            initial={{ opacity: 0, scale: 0.98 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.98 }}
            className="pointer-events-none absolute -inset-3 grid place-items-center rounded-3xl border-2 border-dashed border-primary bg-background/85 backdrop-blur-sm"
          >
            <span className="flex flex-col items-center gap-2 font-extrabold text-primary">
              <motion.span animate={{ y: [0, -5, 0] }} transition={{ duration: 0.9, repeat: Infinity }}>
                <FileUpIcon className="size-8" />
              </motion.span>
              Drop a theme file to import it
            </span>
          </motion.div>
        )}
      </AnimatePresence>
      <input ref={input} type="file" accept=".json,application/json" hidden onChange={(e) => (void importFile(e.target.files?.[0]), (e.target.value = ""))} />
    </div>
  );
}

function Empty({ onCreate }: { onCreate: () => void }) {
  return (
    <motion.button
      type="button"
      onClick={onCreate}
      whileHover="hover"
      whileTap={{ scale: 0.98 }}
      className="group flex flex-col items-center gap-3 rounded-3xl border-2 border-dashed px-6 py-10 text-center transition-colors hover:border-primary/50"
    >
      <motion.span
        variants={{ hover: { rotate: [0, -12, 10, 0], scale: 1.1 } }}
        transition={{ duration: 0.6 }}
        className="grid size-14 place-items-center rounded-2xl bg-primary/15 text-primary"
      >
        <WandSparklesIcon className="size-7" />
      </motion.span>
      <span className="font-extrabold">Make fuwa yours</span>
      <span className="max-w-sm text-sm text-muted-foreground">Start from the theme you're using, pick your colors, add a background picture and an effect. Or drop a theme file here.</span>
    </motion.button>
  );
}

function ThemeCard({
  theme,
  index,
  active,
  onUse,
  onEdit,
  onDuplicate,
  onExport,
  onDelete,
}: {
  theme: CustomTheme;
  index: number;
  active: boolean;
  onUse: (e: MouseEvent) => void;
  onEdit: () => void;
  onDuplicate: () => void;
  onExport: () => void;
  onDelete: () => void;
}) {
  const t = theme.variant.tokens;
  const picture = shownPicture(theme.backdrop?.image);
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 10, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { ...SPRING, delay: index * 0.03 } }}
      exit={{ opacity: 0, scale: 0.9 }}
      whileHover={{ y: -3 }}
      style={{ background: t.background, color: t.foreground, borderColor: active ? t.primary : t.border }}
      className={cn("relative overflow-hidden rounded-2xl border-2", active && "shadow-lg")}
    >
      {picture && <img src={picture} alt="" className="absolute inset-0 size-full object-cover opacity-30" />}
      <button type="button" onClick={onUse} className="relative flex w-full flex-col gap-3 p-4 text-left" aria-pressed={active}>
        <span className="flex items-center gap-2">
          {[t.primary, t.card, t["muted-foreground"], t.border].map((c, i) => (
            <span key={i} className="size-5 rounded-full border" style={{ background: c, borderColor: t.border }} />
          ))}
          {theme.backdrop && theme.backdrop.effect !== "none" && <SparklesIcon className="size-4" style={{ color: t.primary }} />}
        </span>
        <span className="pr-8">
          <span className="block truncate font-extrabold">{theme.name}</span>
          <span className="block truncate text-xs" style={{ color: t["muted-foreground"] }}>
            {theme.description || (active ? "In use" : "Click to use it")}
          </span>
        </span>
      </button>
      <div className="absolute top-3 right-3 flex items-center gap-1">
        {active && (
          <motion.span layoutId="custom-theme-check" transition={SPRING} className="grid size-6 place-items-center rounded-full" style={{ background: t.primary, color: t["primary-foreground"] }}>
            <CheckIcon className="size-4" />
          </motion.span>
        )}
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button type="button" aria-label={`More for ${theme.name}`} className="grid size-7 place-items-center rounded-full transition-colors hover:bg-black/10">
              <EllipsisIcon className="size-4" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onSelect={onEdit}>
              <PencilIcon className="size-4" /> Edit
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={onDuplicate}>
              <CopyIcon className="size-4" /> Duplicate
            </DropdownMenuItem>
            <DropdownMenuItem onSelect={onExport}>
              <DownloadIcon className="size-4" /> Export file
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem variant="destructive" onSelect={onDelete}>
              <Trash2Icon className="size-4" /> Delete
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </motion.div>
  );
}

/** Saves a theme file, with its background picture inside so it works anywhere. */
async function exportTheme(theme: CustomTheme) {
  let picture: string | null = null;
  const src = shownPicture(theme.backdrop?.image);
  if (src) {
    try {
      const response = await fetch(src, { credentials: "omit", referrerPolicy: "no-referrer" });
      if (response.ok) picture = await blobToDataUrl(await response.blob());
    } catch {
      // Exported without its picture.
    }
  }
  const file = toFile(theme, theme.backdrop, picture);
  const url = URL.createObjectURL(new Blob([JSON.stringify(file, null, 2)], { type: "application/json" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = fileName(theme.name);
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
  toast(theme.backdrop?.image && !picture ? `Exported ${theme.name}, without its picture` : `Exported ${theme.name}`);
}

// ───────────────────────── Editor ─────────────────────────

/** WCAG contrast between two #rrggbb colors. */
function contrast(a: string, b: string) {
  const lum = (hex: string) => {
    const [r, g, bl] = [1, 3, 5].map((i) => {
      const c = parseInt(hex.slice(i, i + 2), 16) / 255;
      return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r! + 0.7152 * g! + 0.0722 * bl!;
  };
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

function Editor({ instanceKey, start, isNew, onClose }: { instanceKey?: string; start: CustomTheme; isNew: boolean; onClose: () => void }) {
  const [theme, setTheme] = useState(start);
  const [allColors, setAllColors] = useState(false);
  const appBackdrop = usePrefs((p) => p.backdrop);
  const t = theme.variant.tokens;
  const changed = isNew || JSON.stringify(theme) !== JSON.stringify(start);

  const setTokens = (patch: Partial<Record<Token, string>>) => setTheme((th) => ({ ...th, variant: { ...th.variant, tokens: { ...th.variant.tokens, ...patch } } }));

  /** A main color changed: the colors made from it follow, unless someone tuned them by hand. */
  function setSeed(key: (typeof SEEDS)[number], value: string) {
    setTheme((th) => {
      const tokens = th.variant.tokens;
      const before = deriveTokens(seedsOf(tokens));
      const after = deriveTokens({ ...seedsOf(tokens), [key]: value });
      const next = { ...tokens, [key]: value };
      for (const k of TOKENS) if (!(SEEDS as readonly string[]).includes(k) && tokens[k] === before[k]) next[k] = after[k];
      return { ...th, variant: { ...th.variant, tokens: next } };
    });
  }

  const setBackdrop = (patch: Partial<Backdrop>) => setTheme((th) => ({ ...th, backdrop: { ...(th.backdrop ?? DEFAULT_BACKDROP), ...patch } }));

  function save(use: boolean, e?: MouseEvent) {
    const saved = { ...theme, name: theme.name.trim() || "Untitled" };
    saveCustomTheme(saved);
    if (use || getPrefs().theme === saved.id) switchTheme({ theme: saved.id, followSystem: false }, e && clickPoint(e));
    toast(isNew ? `Made ${saved.name}` : `Saved ${saved.name}`);
    onClose();
  }

  const readable = contrast(t.foreground, t.background);
  const onPrimary = contrast(t["primary-foreground"], t.primary);

  return (
    <WithPreview
      preview={
        <div className="flex flex-col gap-3">
          <ThemePreview theme={theme} backdrop={theme.backdrop ?? appBackdrop} />
          <Readability label="Text" ratio={readable} />
          <Readability label="Text on primary" ratio={onPrimary} />
        </div>
      }
    >
      <div className="flex flex-col gap-7 pb-24">
        <div className="flex items-center gap-2">
          <Button type="button" variant="ghost" size="sm" onClick={onClose} className="group -ml-2">
            <ArrowLeftIcon className="size-4 transition-transform group-hover:-translate-x-0.5" />
            Your themes
          </Button>
        </div>

        <div className="grid gap-3 sm:grid-cols-[1fr_1.5fr]">
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-bold tracking-wide text-muted-foreground uppercase">Name</span>
            <Input value={theme.name} maxLength={NAME_MAX} onChange={(e) => setTheme((th) => ({ ...th, name: e.target.value }))} />
          </label>
          <label className="flex flex-col gap-1.5">
            <span className="text-xs font-bold tracking-wide text-muted-foreground uppercase">Description</span>
            <Input
              value={theme.description ?? ""}
              maxLength={DESCRIPTION_MAX}
              placeholder="A few words about it"
              onChange={(e) => setTheme((th) => ({ ...th, description: e.target.value || null }))}
            />
          </label>
        </div>

        <Group title="Colors" hint="The main colors; the rest are made from them.">
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-3">
            {SEEDS.map((key) => (
              <ColorField key={key} label={TOKEN_LABELS[key]} value={t[key]} onChange={(v) => setSeed(key, v)} />
            ))}
          </div>
          <button type="button" onClick={() => setAllColors((v) => !v)} className="self-start text-xs font-bold text-primary hover:underline">
            {allColors ? "Hide the other colors" : "Fine-tune every color"}
          </button>
          <AnimatePresence initial={false}>
            {allColors && (
              <motion.div initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING} className="overflow-hidden">
                <div className="grid grid-cols-2 gap-2 pt-1 sm:grid-cols-4">
                  {TOKENS.filter((k) => !(SEEDS as readonly string[]).includes(k)).map((key) => (
                    <ColorField key={key} label={TOKEN_LABELS[key]} value={t[key]} onChange={(v) => setTokens({ [key]: v })} />
                  ))}
                </div>
              </motion.div>
            )}
          </AnimatePresence>
        </Group>

        <Group title="Corners" hint="How round buttons, cards and fields are.">
          <Labeled label="Corners" shown={`${theme.variant.radius.toFixed(2)}rem`}>
            <Slider
              label="Corners"
              value={theme.variant.radius}
              min={RADIUS_MIN}
              max={RADIUS_MAX}
              step={0.05}
              format={(n) => `${n.toFixed(2)}rem`}
              onChange={(radius) => setTheme((th) => ({ ...th, variant: { ...th.variant, radius } }))}
              className="pt-6"
            />
          </Labeled>
        </Group>

        <Group title="Backdrop" hint="A picture and an effect that come with this theme.">
          <Toggle
            checked={!!theme.backdrop}
            onChange={(on) => setTheme((th) => ({ ...th, backdrop: on ? { ...appBackdrop } : null }))}
            label="Bring its own backdrop"
            hint="Off, it uses your Background setting like any theme."
          />
          <AnimatePresence initial={false}>
            {theme.backdrop && (
              <motion.div initial={{ opacity: 0, y: -6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING} className="overflow-hidden pt-2">
                <BackdropForm instanceKey={instanceKey} value={theme.backdrop} onChange={setBackdrop} />
              </motion.div>
            )}
          </AnimatePresence>
        </Group>
      </div>

      <motion.div
        initial={{ y: 24, opacity: 0 }}
        animate={{ y: 0, opacity: 1 }}
        transition={SPRING}
        className="sticky bottom-4 z-10 flex items-center justify-end gap-2 rounded-2xl border bg-popover/95 p-3 shadow-xl backdrop-blur"
      >
        <span className="mr-auto hidden text-sm text-muted-foreground sm:inline">{changed ? (isNew ? "A new theme" : "Unsaved changes") : "No changes"}</span>
        <Button type="button" variant="ghost" onClick={onClose}>
          Cancel
        </Button>
        <Button type="button" variant="outline" disabled={!changed} onClick={() => save(false)}>
          Save
        </Button>
        <Button type="button" onClick={(e) => save(true, e)} className="group">
          <SparklesIcon className="size-4 transition-transform group-hover:rotate-12" />
          Save and use
        </Button>
      </motion.div>
    </WithPreview>
  );
}

function Group({ title, hint, children }: { title: string; hint: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-3 border-t pt-6">
      <div>
        <h3 className="font-extrabold">{title}</h3>
        <p className="mt-0.5 text-sm text-muted-foreground">{hint}</p>
      </div>
      {children}
    </section>
  );
}

/** A color: the swatch opens the system picker, the text takes a #rrggbb. */
function ColorField({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  const [text, setText] = useState(value);
  const [editing, setEditing] = useState(false);
  const shown = editing ? text : value;
  return (
    <div className="group flex items-center gap-2 rounded-xl border p-1.5 pr-2 transition-colors focus-within:border-primary/60 hover:border-primary/30">
      <span className="relative size-8 shrink-0 overflow-hidden rounded-lg border shadow-inner transition-transform group-hover:scale-105" style={{ background: value }}>
        <input type="color" value={value} onChange={(e) => onChange(e.target.value)} className="absolute inset-0 size-full cursor-pointer opacity-0" aria-label={label} />
      </span>
      <span className="min-w-0">
        <span className="block truncate text-[0.65rem] font-bold text-muted-foreground">{label}</span>
        <input
          value={shown}
          spellCheck={false}
          onFocus={() => (setText(value), setEditing(true))}
          onBlur={() => setEditing(false)}
          onChange={(e) => {
            const v = e.target.value.trim();
            setText(v);
            const hex = v.startsWith("#") ? v : `#${v}`;
            if (HEX.test(hex)) onChange(hex.toLowerCase());
          }}
          aria-label={`${label}, as #rrggbb`}
          className="w-full bg-transparent font-mono text-xs outline-none"
        />
      </span>
    </div>
  );
}

/** How readable a pair of colors is, by the WCAG contrast ratio. */
function Readability({ label, ratio }: { label: string; ratio: number }) {
  const good = ratio >= 4.5;
  const ok = ratio >= 3;
  return (
    <div className="flex items-center justify-between rounded-xl border px-3 py-2 text-xs">
      <span className="font-bold">{label}</span>
      <motion.span
        key={good ? "good" : ok ? "ok" : "bad"}
        initial={{ scale: 0.7, opacity: 0 }}
        animate={{ scale: 1, opacity: 1 }}
        transition={SPRING}
        className={cn("rounded-full px-2 py-0.5 font-bold", good ? "bg-emerald-500/15 text-emerald-600 dark:text-emerald-400" : ok ? "bg-amber-500/15 text-amber-600 dark:text-amber-400" : "bg-destructive/15 text-destructive")}
      >
        {ratio.toFixed(1)}:1 {good ? "Easy to read" : ok ? "Large text only" : "Hard to read"}
      </motion.span>
    </div>
  );
}
