import { create } from "@bufbuild/protobuf";
import { ArrowDownIcon, ArrowUpIcon, CheckIcon, PlusIcon, Trash2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState } from "react";
import { BoardColumnSchema, BoardLabelSchema, type BoardColumn, type BoardLabel } from "@/gen/fuwa/v1/types_pb";
import { setBoard } from "@/fuwa/board";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { useI18n } from "@/i18n/react";
import { COLUMN_NAME_MAX, COLUMNS_MAX, colorCss, LABEL_NAME_MAX, LABELS_MAX, PALETTE } from "@/lib/board";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";
import { LabelChip } from "./BoardCard";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  serverId: string;
  channelId: string;
  columns: BoardColumn[];
  labels: BoardLabel[];
  /** Cards on the board in each column, which can't be removed while it has any. */
  counts: Record<string, number>;
};

/** A board's columns and labels, for people who can manage the channel. */
export function BoardSettingsDialog({ open, onOpenChange, ...rest }: Props) {
  const { t } = useI18n();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent wide>
        <DialogHeader title={t("chattools.board.settings")} description={t("chattools.board.settingsAbout")} />
        {/* Mounted only while open, so each time starts from the board as it is. */}
        <Form onOpenChange={onOpenChange} {...rest} />
      </DialogContent>
    </Dialog>
  );
}

/** A row being edited: new ones have no id yet, so they're told apart by `key`. */
type Draft<A> = A & { key: string };
let next = 0;
const draft = <A extends { id: string }>(a: A): Draft<A> => ({ ...a, key: a.id || `new-${++next}` });

function Form({ onOpenChange, instanceKey, serverId, channelId, columns, labels, counts }: Omit<Props, "open">) {
  const { t } = useI18n();
  const [cols, setCols] = useState(() => columns.map(draft));
  const [labs, setLabs] = useState(() => labels.map(draft));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const setCol = (key: string, change: Partial<BoardColumn>) => setCols((list) => list.map((c) => (c.key === key ? { ...c, ...change } : c)));
  const setLab = (key: string, change: Partial<BoardLabel>) => setLabs((list) => list.map((l) => (l.key === key ? { ...l, ...change } : l)));
  const moveCol = (i: number, by: number) =>
    setCols((list) => {
      const out = [...list];
      const [c] = out.splice(i, 1);
      out.splice(i + by, 0, c!);
      return out;
    });
  const valid = cols.length > 0 && cols.every((c) => c.name.trim()) && labs.every((l) => l.name.trim());

  async function save() {
    setSaving(true);
    setError(null);
    try {
      await setBoard(
        instanceKey,
        serverId,
        channelId,
        cols.map(({ key: _, ...c }) => create(BoardColumnSchema, { ...c, name: c.name.trim() })),
        labs.map(({ key: _, ...l }) => create(BoardLabelSchema, { ...l, name: l.name.trim() })),
      );
      onOpenChange(false);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="flex flex-col gap-5">
      <section className="flex flex-col gap-2">
        <h3 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("chattools.board.columns")}</h3>
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false} mode="popLayout">
            {cols.map((c, i) => {
              const held = counts[c.id] ?? 0;
              return (
                <motion.li
                  key={c.key}
                  layout
                  initial={{ opacity: 0, y: 8 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, scale: 0.95 }}
                  transition={SPRING}
                  className="flex flex-col gap-2 rounded-2xl border bg-muted/30 p-3"
                >
                  <div className="flex items-center gap-2">
                    <Swatches value={c.color} onPick={(color) => setCol(c.key, { color })} />
                    <Input
                      value={c.name}
                      maxLength={COLUMN_NAME_MAX}
                      onChange={(e) => setCol(c.key, { name: e.target.value })}
                      aria-label={t("chattools.board.columnName")}
                      placeholder={t("chattools.board.columnName")}
                      className="h-9 flex-1 rounded-xl"
                    />
                    <IconButton label={t("chattools.board.moveUp")} disabled={i === 0} onClick={() => moveCol(i, -1)}>
                      <ArrowUpIcon />
                    </IconButton>
                    <IconButton label={t("chattools.board.moveDown")} disabled={i === cols.length - 1} onClick={() => moveCol(i, 1)}>
                      <ArrowDownIcon />
                    </IconButton>
                    <IconButton
                      label={held ? t("chattools.board.columnHasCards", { count: held }) : t("chattools.board.removeColumn")}
                      disabled={held > 0 || cols.length === 1}
                      danger
                      onClick={() => setCols((list) => list.filter((x) => x.key !== c.key))}
                    >
                      <Trash2Icon />
                    </IconButton>
                  </div>
                  <div className="flex flex-wrap items-center gap-x-5 gap-y-2 pl-1 text-sm">
                    <label className="flex items-center gap-2">
                      <Switch checked={c.done} onCheckedChange={(done: boolean) => setCol(c.key, { done })} />
                      <span>{t("chattools.board.doneColumn")}</span>
                    </label>
                    <label className="flex items-center gap-2">
                      <span>{t("chattools.board.cardLimit")}</span>
                      <Input
                        type="number"
                        min={0}
                        max={10000}
                        value={c.cardLimit || ""}
                        placeholder={t("chattools.board.noLimit")}
                        onChange={(e) => setCol(c.key, { cardLimit: Math.max(0, Math.min(10000, Math.floor(Number(e.target.value) || 0))) })}
                        className="h-8 w-24 rounded-lg"
                      />
                    </label>
                  </div>
                </motion.li>
              );
            })}
          </AnimatePresence>
        </ul>
        {cols.length < COLUMNS_MAX && (
          <AddButton onClick={() => setCols((list) => [...list, draft(create(BoardColumnSchema, { name: "", color: PALETTE[list.length % PALETTE.length]! }))])}>
            {t("chattools.board.addColumn")}
          </AddButton>
        )}
      </section>

      <section className="flex flex-col gap-2">
        <h3 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("chattools.board.labels")}</h3>
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false} mode="popLayout">
            {labs.map((l) => (
              <motion.li
                key={l.key}
                layout
                initial={{ opacity: 0, y: 8 }}
                animate={{ opacity: 1, y: 0 }}
                exit={{ opacity: 0, scale: 0.95 }}
                transition={SPRING}
                className="flex items-center gap-2"
              >
                <Swatches value={l.color} onPick={(color) => setLab(l.key, { color })} />
                <Input
                  value={l.name}
                  maxLength={LABEL_NAME_MAX}
                  onChange={(e) => setLab(l.key, { name: e.target.value })}
                  aria-label={t("chattools.board.labelName")}
                  placeholder={t("chattools.board.labelName")}
                  className="h-9 flex-1 rounded-xl"
                />
                <span className="hidden w-28 shrink-0 justify-center sm:flex">
                  <LabelChip label={{ ...l, name: l.name || "…" }} />
                </span>
                <IconButton label={t("chattools.board.removeLabel")} danger onClick={() => setLabs((list) => list.filter((x) => x.key !== l.key))}>
                  <Trash2Icon />
                </IconButton>
              </motion.li>
            ))}
          </AnimatePresence>
        </ul>
        {!labs.length && <p className="text-sm text-muted-foreground">{t("chattools.board.noBoardLabels")}</p>}
        {labs.length < LABELS_MAX && (
          <AddButton onClick={() => setLabs((list) => [...list, draft(create(BoardLabelSchema, { name: "", color: PALETTE[(list.length * 5) % PALETTE.length]! }))])}>
            {t("chattools.board.addLabel")}
          </AddButton>
        )}
      </section>

      <AnimatePresence>
        {error && (
          <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="text-sm font-bold text-destructive">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <div className="flex justify-end gap-2">
        <Button variant="ghost" className="rounded-xl" onClick={() => onOpenChange(false)}>
          {t("chattools.board.cancel")}
        </Button>
        <Button className="btn rounded-xl font-bold" disabled={!valid || saving} onClick={() => void save()}>
          <CheckIcon /> {t("chattools.board.save")}
        </Button>
      </div>
    </div>
  );
}

/** A color from the palette, or none. */
function Swatches({ value, onPick }: { value: number; onPick: (color: number) => void }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  return (
    <div className="relative">
      <button
        type="button"
        aria-label={t("chattools.board.color")}
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="grid size-9 place-items-center rounded-xl border bg-card transition hover:scale-105"
      >
        <span className="size-4 rounded-full bg-muted-foreground" style={value ? { background: colorCss(value) } : undefined} />
      </button>
      <AnimatePresence>
        {open && (
          <motion.div
            initial={{ opacity: 0, scale: 0.9, y: -4 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.9, y: -4 }}
            transition={SPRING}
            className="absolute top-11 left-0 z-10 grid w-max grid-cols-7 gap-1.5 rounded-2xl border bg-popover p-2 shadow-xl"
          >
            {[0, ...PALETTE].map((color) => (
              <button
                key={color}
                type="button"
                aria-label={color ? colorCss(color) : t("chattools.board.noColor")}
                onClick={() => (onPick(color), setOpen(false))}
                className={cn("size-6 rounded-full ring-offset-2 ring-offset-popover transition hover:scale-110", color === value && "ring-2 ring-primary", !color && "border border-dashed")}
                style={color ? { background: colorCss(color) } : undefined}
              />
            ))}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function IconButton({ label, disabled, danger, onClick, children }: { label: string; disabled?: boolean; danger?: boolean; onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "grid size-8 shrink-0 place-items-center rounded-full text-muted-foreground transition hover:bg-muted active:scale-90 disabled:opacity-30 disabled:hover:bg-transparent [&_svg]:size-4",
        danger && "hover:text-destructive",
      )}
    >
      {children}
    </button>
  );
}

function AddButton({ onClick, children }: { onClick: () => void; children: React.ReactNode }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex items-center gap-1.5 self-start rounded-xl px-2 py-1.5 text-sm font-bold text-primary transition hover:bg-primary/10 active:scale-95"
    >
      <PlusIcon className="size-4" /> {children}
    </button>
  );
}
