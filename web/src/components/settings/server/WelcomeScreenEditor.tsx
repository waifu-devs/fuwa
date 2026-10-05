import { create } from "@bufbuild/protobuf";
import { ChevronDownIcon, GripVerticalIcon, HashIcon, PlusIcon, SmilePlusIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion, Reorder, useDragControls } from "motion/react";
import {
  ChannelType,
  WelcomeChannelSchema,
  WelcomeScreenSchema,
  type Channel,
  type Emoji,
  type Server,
  type WelcomeScreen,
} from "@/gen/fuwa/v1/types_pb";
import { useInstance } from "@/fuwa/hooks";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { EmojiPicker } from "@/components/EmojiPicker";
import { SPRING } from "@/lib/motion";
import { Toggle } from "@/components/settings/controls";
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
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

const MAX_CHANNELS = 5;
const DESCRIPTION_MAX = 300;
const NOTE_MAX = 60;

/** A suggested channel being edited, with a key that survives reordering. */
type Row = { key: string; channelId: string; description: string; emoji: string };

/** The welcome screen as it's being edited. */
export type WelcomeDraft = { enabled: boolean; description: string; list: Row[] };

export const welcomeDraft = (screen: WelcomeScreen): WelcomeDraft => ({
  enabled: screen.enabled,
  description: screen.description,
  list: screen.channels.map((c, n) => ({ key: `${n}-${c.channelId}`, channelId: c.channelId, description: c.description, emoji: c.emoji })),
});

const same = (a: Row[], b: Row[]) =>
  a.length === b.length && a.every((r, n) => r.channelId === b[n]!.channelId && r.description === b[n]!.description && r.emoji === b[n]!.emoji);

/** How many of the welcome screen's settings differ from what's saved. */
export const welcomeChanges = (draft: WelcomeDraft, saved: WelcomeScreen) => {
  const was = welcomeDraft(saved);
  return [draft.enabled !== was.enabled, draft.description !== was.description, !same(draft.list, was.list)].filter(Boolean).length;
};

/** The draft as the server takes it. */
export const welcomeScreen = (draft: WelcomeDraft): WelcomeScreen =>
  create(WelcomeScreenSchema, {
    enabled: draft.enabled,
    description: draft.description.trim(),
    channels: draft.list
      .filter((r) => r.channelId)
      .map((r) => create(WelcomeChannelSchema, { channelId: r.channelId, description: r.description.trim(), emoji: r.emoji })),
  });

/**
 * What new members see first: a few words and up to five channels to start
 * in, each with an emoji and a note. Saving and the preview are the page's.
 */
export function WelcomeFields({
  instanceKey,
  server,
  draft,
  onChange,
}: {
  instanceKey: string;
  server: Server;
  draft: WelcomeDraft;
  onChange: (draft: WelcomeDraft) => void;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const channels = (inst?.channels[server.id] ?? []).filter((c) => c.type !== ChannelType.CATEGORY);
  const emojis = inst?.emojis[server.id];
  const { enabled, description, list } = draft;
  const setList = (next: Row[] | ((l: Row[]) => Row[])) => onChange({ ...draft, list: typeof next === "function" ? next(list) : next });
  const unused = channels.filter((c) => !list.some((r) => r.channelId === c.id));
  const update = (key: string, patch: Partial<Row>) => setList((l) => l.map((r) => (r.key === key ? { ...r, ...patch } : r)));

  return (
    <div className="flex flex-col">
      <div data-setting="welcome-enabled" className="border-b border-border/70 pb-5">
        <Toggle
          checked={enabled}
          onChange={(on) => onChange({ ...draft, enabled: on })}
          label={t("serversettings.nav.welcomeEnabled")}
          hint={t("serversettings.welcomeScreen.enabledHint")}
        />
      </div>
      <div data-setting="welcome-description" className="flex flex-col gap-2 border-b border-border/70 py-5">
        <Label htmlFor="welcome-description" className="font-extrabold">
          {t("serversettings.welcomeScreen.words")}
        </Label>
        <Textarea
          id="welcome-description"
          rows={3}
          maxLength={DESCRIPTION_MAX}
          value={description}
          placeholder={t("serversettings.welcomeScreen.wordsPlaceholder")}
          onChange={(e) => onChange({ ...draft, description: e.target.value })}
          className="rounded-xl"
        />
        <p className="flex justify-between gap-3 text-sm text-muted-foreground">
          <span>
            {t("serversettings.welcomeScreen.markdown", {
              bold: `**${t("serversettings.welcomeScreen.bold")}**`,
              italics: `*${t("serversettings.welcomeScreen.italics")}*`,
            })}
          </span>
          <span className={cn("tabular-nums", description.length > DESCRIPTION_MAX - 30 && "text-amber-600 dark:text-amber-400")}>
            {description.length}/{DESCRIPTION_MAX}
          </span>
        </p>
      </div>
      <div data-setting="welcome-channels" className="flex flex-col gap-3 py-5">
        <span>
          <span className="block font-extrabold">{t("serversettings.welcomeScreen.channels")}</span>
          <span className="block text-sm text-muted-foreground">{t("serversettings.welcomeScreen.channelsHint", { max: MAX_CHANNELS })}</span>
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
              <PlusIcon className="transition-transform group-hover:rotate-90" /> {t("serversettings.welcomeScreen.addChannel")}
            </Button>
          </motion.div>
        )}
      </div>
    </div>
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
  const { t } = useI18n();
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
        aria-label={t("serversettings.shared.dragToReorder")}
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
            aria-label={row.emoji ? t("serversettings.shared.changeEmoji") : t("serversettings.shared.pickEmoji")}
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
            <span className="flex-1 truncate font-bold">{channel?.name ?? t("serversettings.shared.pickChannel")}</span>
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
        placeholder={t("serversettings.welcomeScreen.notePlaceholder")}
        aria-label={channel ? t("serversettings.welcomeScreen.noteFor", { channel: channel.name }) : t("serversettings.welcomeScreen.noteForThis")}
        onChange={(e) => onChange({ description: e.target.value })}
        className="h-9 min-w-0 basis-full rounded-xl sm:basis-auto sm:flex-1"
      />
      <Button type="button" variant="ghost" size="icon" aria-label={t("serversettings.channelPermissions.remove")} onClick={onRemove} className="size-9 shrink-0 rounded-full text-muted-foreground hover:text-destructive">
        <XIcon />
      </Button>
    </Reorder.Item>
  );
}

