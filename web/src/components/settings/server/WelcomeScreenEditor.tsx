import { create } from "@bufbuild/protobuf";
import { ChevronDownIcon, GripVerticalIcon, HashIcon, PlusIcon, SmilePlusIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion, Reorder, useDragControls } from "motion/react";
import { useEffect, useState } from "react";
import {
  ChannelType,
  WelcomeChannelSchema,
  WelcomeScreenSchema,
  type Channel,
  type Emoji,
  type Server,
  type WelcomeScreen,
} from "@/gen/fuwa/v1/types_pb";
import { getWelcomeScreen, run, setWelcomeScreen } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction, useInstance } from "@/fuwa/hooks";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { EmojiPicker } from "@/components/EmojiPicker";
import { WelcomeCard } from "@/components/join/Welcome";
import { SPRING } from "@/components/motion";
import { SaveBar, Toggle, WithPreview } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";

const MAX_CHANNELS = 5;
const DESCRIPTION_MAX = 300;
const NOTE_MAX = 60;

/** A suggested channel being edited, with a key that survives reordering. */
type Row = { key: string; channelId: string; description: string; emoji: string };

const rows = (screen: WelcomeScreen): Row[] => screen.channels.map((c, n) => ({ key: `${n}-${c.channelId}`, ...c }));
const same = (a: Row[], b: Row[]) =>
  a.length === b.length && a.every((r, n) => r.channelId === b[n]!.channelId && r.description === b[n]!.description && r.emoji === b[n]!.emoji);

/**
 * What new members see first: a few words and up to five channels to start
 * in, each with an emoji and a note. The preview is the real thing.
 */
export function WelcomeScreenEditor({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const inst = useInstance(instanceKey);
  const channels = (inst?.channels[server.id] ?? []).filter((c) => c.type !== ChannelType.CATEGORY);
  const emojis = inst?.emojis[server.id];
  const [saved, setSaved] = useState<WelcomeScreen | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [enabled, setEnabled] = useState(false);
  const [description, setDescription] = useState("");
  const [list, setList] = useState<Row[]>([]);
  const save = useAction(setWelcomeScreen);

  function load(screen: WelcomeScreen) {
    setSaved(screen);
    setEnabled(screen.enabled);
    setDescription(screen.description);
    setList(rows(screen));
  }

  useEffect(() => {
    run(getWelcomeScreen(instanceKey, server.id)).then(load, (e: FuwaError) => setProblem(e.message));
  }, [instanceKey, server.id]);

  if (problem) return <p className="text-sm text-muted-foreground first-letter:uppercase">{problem}</p>;
  if (!saved) return <div className="shimmer h-64 rounded-2xl" />;

  const changes = [enabled !== saved.enabled, description !== saved.description, !same(list, rows(saved))].filter(Boolean).length;
  const draft = create(WelcomeScreenSchema, {
    enabled,
    description: description.trim(),
    channels: list.filter((r) => r.channelId).map((r) => create(WelcomeChannelSchema, { channelId: r.channelId, description: r.description.trim(), emoji: r.emoji })),
  });
  const empty = !draft.description && draft.channels.length === 0;
  const unused = channels.filter((c) => !list.some((r) => r.channelId === c.id));
  const update = (key: string, patch: Partial<Row>) => setList((l) => l.map((r) => (r.key === key ? { ...r, ...patch } : r)));

  async function submit() {
    if (enabled && empty) return save.setError("add a few words or a channel first");
    const next = await save.go(instanceKey, server.id, draft);
    if (next) load(next);
  }

  return (
    <WithPreview
      preview={
        <div className="relative overflow-hidden rounded-3xl border bg-card p-5 shadow-lg">
          <div className="pointer-events-none absolute inset-x-0 top-0 h-28 bg-gradient-to-b from-primary/20 to-transparent" />
          <motion.div animate={{ opacity: enabled ? 1 : 0.35, filter: enabled ? "blur(0px)" : "blur(2px)" }} transition={{ duration: 0.3 }}>
            <WelcomeCard className="relative" server={server} screen={draft} channels={channels} emojis={emojis} />
            {empty && <p className="relative mt-3 text-center text-xs text-muted-foreground">Add a few words or a channel to see it here.</p>}
          </motion.div>
          <AnimatePresence>
            {!enabled && (
              <motion.p
                initial={{ opacity: 0, scale: 0.9 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.9 }}
                transition={SPRING}
                className="absolute inset-x-6 top-1/2 -translate-y-1/2 rounded-2xl bg-popover/90 p-3 text-center text-sm font-bold shadow-lg backdrop-blur"
              >
                Off: new members go straight to the server.
              </motion.p>
            )}
          </AnimatePresence>
        </div>
      }
    >
      <div className="flex flex-col">
        <div data-setting="welcome-enabled" className="border-b border-border/70 pb-5">
          <Toggle
            checked={enabled}
            onChange={setEnabled}
            label="Show a welcome screen"
            hint="New members see it once, after agreeing to any rules. Anyone can open it again from the server menu."
          />
        </div>
        <div data-setting="welcome-description" className="flex flex-col gap-2 border-b border-border/70 py-5">
          <Label htmlFor="welcome-description" className="font-extrabold">
            A few words
          </Label>
          <Textarea
            id="welcome-description"
            rows={3}
            maxLength={DESCRIPTION_MAX}
            value={description}
            placeholder="What this place is about, and where to begin."
            onChange={(e) => setDescription(e.target.value)}
            className="rounded-xl"
          />
          <p className="flex justify-between gap-3 text-sm text-muted-foreground">
            <span>Markdown works on one line: **bold**, *italics*, links.</span>
            <span className={cn("tabular-nums", description.length > DESCRIPTION_MAX - 30 && "text-amber-600 dark:text-amber-400")}>
              {description.length}/{DESCRIPTION_MAX}
            </span>
          </p>
        </div>
        <div data-setting="welcome-channels" className="flex flex-col gap-3 py-5">
          <span>
            <span className="block font-extrabold">Channels to start in</span>
            <span className="block text-sm text-muted-foreground">
              Up to {MAX_CHANNELS}. Drag to reorder. People only see the ones they're allowed into.
            </span>
          </span>
          <Reorder.Group axis="y" values={list} onReorder={setList} className="flex flex-col gap-2">
            <AnimatePresence initial={false}>
              {list.map((row) => (
                <ChannelRow
                  key={row.key}
                  row={row}
                  channels={channels}
                  unused={unused}
                  emojis={emojis}
                  server={server}
                  onChange={(patch) => update(row.key, patch)}
                  onRemove={() => setList((l) => l.filter((r) => r.key !== row.key))}
                />
              ))}
            </AnimatePresence>
          </Reorder.Group>
          {list.length < MAX_CHANNELS && unused.length > 0 && (
            <motion.div layout transition={SPRING}>
              <Button
                type="button"
                variant="outline"
                className="group rounded-xl border-dashed"
                onClick={() => setList((l) => [...l, { key: `new-${Date.now()}`, channelId: unused[0]!.id, description: "", emoji: "" }])}
              >
                <PlusIcon className="transition-transform group-hover:rotate-90" /> Add a channel
              </Button>
            </motion.div>
          )}
        </div>
      </div>
      <SaveBar
        count={changes}
        saving={save.pending}
        error={save.error}
        onSave={() => void submit()}
        onDiscard={() => {
          load(saved);
          save.setError(null);
        }}
      />
    </WithPreview>
  );
}

function ChannelRow({
  row,
  channels,
  unused,
  emojis,
  server,
  onChange,
  onRemove,
}: {
  row: Row;
  channels: Channel[];
  unused: Channel[];
  emojis: Emoji[] | undefined;
  server: Server;
  onChange: (patch: Partial<Row>) => void;
  onRemove: () => void;
}) {
  const drag = useDragControls();
  const channel = channels.find((c) => c.id === row.channelId);
  return (
    <Reorder.Item
      value={row}
      dragListener={false}
      dragControls={drag}
      initial={{ opacity: 0, y: -8, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, x: 24, scale: 0.95 }}
      transition={SPRING}
      whileDrag={{ scale: 1.03, boxShadow: "0 12px 30px -10px rgb(0 0 0 / 0.35)" }}
      className="flex flex-wrap items-center gap-2 rounded-2xl border bg-background/70 p-2 sm:flex-nowrap"
    >
      <button
        type="button"
        aria-label="Drag to reorder"
        onPointerDown={(e) => drag.start(e)}
        className="grid h-9 w-5 shrink-0 cursor-grab touch-none place-items-center text-muted-foreground active:cursor-grabbing"
      >
        <GripVerticalIcon className="size-4" />
      </button>
      <EmojiPicker emojis={emojis} server={server} placement="bottom-start" onPick={(e) => onChange({ emoji: e.text })}>
        {(open) => (
          <motion.button
            type="button"
            whileHover={{ scale: 1.08, rotate: -6 }}
            whileTap={{ scale: 0.9 }}
            aria-label={row.emoji ? "Change the emoji" : "Pick an emoji"}
            className={cn(
              "grid size-9 shrink-0 place-items-center rounded-xl border text-xl transition-colors",
              open ? "border-primary/60 bg-primary/10" : "hover:border-primary/40",
            )}
          >
            {row.emoji ? <EmojiGlyph value={row.emoji} emojis={emojis} className="size-6" /> : <SmilePlusIcon className="size-4 text-muted-foreground" />}
          </motion.button>
        )}
      </EmojiPicker>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button
            type="button"
            className="group flex h-9 min-w-0 flex-1 items-center gap-1.5 rounded-xl border px-2.5 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60 sm:w-40 sm:flex-none"
          >
            <HashIcon className="size-4 shrink-0 text-muted-foreground" />
            <span className="flex-1 truncate font-bold">{channel?.name ?? "Pick a channel"}</span>
            <ChevronDownIcon className="size-4 shrink-0 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="max-h-72 w-56 overflow-y-auto">
          {[...(channel ? [channel] : []), ...unused].map((c) => (
            <DropdownMenuItem key={c.id} onSelect={() => onChange({ channelId: c.id })}>
              <HashIcon /> {c.name}
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
      <Input
        value={row.description}
        maxLength={NOTE_MAX}
        placeholder="Why go there (optional)"
        aria-label={`Why go to #${channel?.name ?? "this channel"}`}
        onChange={(e) => onChange({ description: e.target.value })}
        className="h-9 min-w-0 basis-full rounded-xl sm:basis-auto sm:flex-1"
      />
      <Button type="button" variant="ghost" size="icon" aria-label="Remove" onClick={onRemove} className="size-9 shrink-0 rounded-full text-muted-foreground hover:text-destructive">
        <XIcon />
      </Button>
    </Reorder.Item>
  );
}

