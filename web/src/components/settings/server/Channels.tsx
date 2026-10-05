import { FolderIcon, GripVerticalIcon, HashIcon, LoaderCircleIcon, LockIcon, PlusIcon, SnailIcon, Trash2Icon, TriangleAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { ChannelType, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { deleteChannel, reorderChannels, run, updateChannel } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useAction, useInstance } from "@/fuwa/hooks";
import { CHANNEL_ICON } from "@/components/ChannelSidebar";
import { useArrange } from "@/hooks/use-arrange";
import { layoutOf, placements, step, type Layout } from "@/lib/arrange";
import { CreateChannelDialog } from "@/components/dialogs/CreateChannelDialog";
import { SPRING } from "@/lib/motion";
import { Row, Segmented } from "@/components/settings/account/common";
import { ChannelPermissions } from "@/components/settings/server/ChannelPermissions";
import { ChannelShare, useSharingOn } from "@/components/settings/server/SharedChannels";
import { SaveBar } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Slider } from "@/components/ui/slider";
import { Textarea } from "@/components/ui/textarea";
import { formatDuration, type Lang, shortDuration } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { has, hasIn, isPrivate } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Discord's slow mode stops, in seconds. */
const SLOW = [0, 5, 10, 15, 30, 60, 120, 300, 600, 900, 1800, 3600, 7200, 21600];
const slowLabel = (lang: Lang, seconds: number) => (seconds ? formatDuration(lang, seconds) : lang.t("serversettings.shared.off"));

/**
 * Every channel, dragged into order by its handle (or moved with the arrow
 * keys), into and out of categories, and the chosen one's settings beside the
 * list: name, topic, category, slow mode, or deleting it.
 */
export function Channels({ instanceKey, serverId, initial: opened }: { instanceKey: string; serverId: string; initial?: string | null }) {
  // "<id>:permissions" opens a channel's permissions, as its right-click menu asks.
  const [initial, initialTab] = (opened ?? "").split(":") as [string, Tab | undefined];
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const arrange = has(access, Permission.MANAGE_CHANNELS);
  const channels = useMemo(() => inst?.channels[serverId] ?? [], [inst?.channels, serverId]);
  const byId = useMemo(() => new Map(channels.map((c) => [c.id, c])), [channels]);
  const layout = useMemo(() => layoutOf(channels), [channels]);
  const list = useRef<HTMLDivElement>(null);
  const [selected, setSelected] = useState<string | null>(initial || (layout.loose[0] ?? null));
  const [creating, setCreating] = useState<string | null>(null);
  const editor = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (initial) setSelected(initial);
  }, [initial]);
  useEffect(() => {
    if (selected && !byId.has(selected)) setSelected(null);
  }, [byId, selected]);

  const save = (next: Layout) => void run(reorderChannels(instanceKey, serverId, placements(next))).catch((err: FuwaError) => toast(err.message));
  useArrange({ container: list, enabled: arrange, layout, onArrange: save, handle: "[data-arrange-handle]" });

  /** Arrow keys on a handle move the item one place (across categories too) and save. */
  const moveKey = (e: KeyboardEvent, id: string) => {
    if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
    e.preventDefault();
    const next = step(layout, id, e.key === "ArrowUp" ? -1 : 1);
    if (next) save(next);
  };

  const pick = (id: string) => {
    setSelected(id);
    if (window.matchMedia("(max-width: 1023px)").matches) setTimeout(() => editor.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 50);
  };

  const item = (id: string, parent: string, list: string[]) => {
    const channel = byId.get(id);
    if (!channel) return null;
    return (
      <ChannelItem
        key={id}
        channel={channel}
        parent={parent}
        active={selected === id}
        onPick={() => pick(id)}
        onKeyMove={(e) => moveKey(e, id)}
        position={t("serversettings.shared.position", { index: list.indexOf(id) + 1, total: list.length })}
        movable={arrange}
        locked={isPrivate(channel, serverId)}
      />
    );
  };
  const ids = layout.categories.map((c) => c.id);

  return (
    <div className="grid gap-6 lg:grid-cols-[minmax(0,19rem)_minmax(0,1fr)] lg:gap-8">
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-2">
          <p className="text-xs text-muted-foreground">
            {arrange ? t("serversettings.channels.introArrange") : t("serversettings.channels.introPick")}
          </p>
          {arrange && (
            <Button type="button" size="sm" onClick={() => setCreating("")} className="btn shrink-0 rounded-xl font-bold">
              <PlusIcon /> {t("serversettings.shared.new")}
            </Button>
          )}
        </div>
        <div ref={list} className="relative flex flex-col gap-0.5 rounded-2xl border bg-background/40 p-2">
          {layout.loose.map((id) => item(id, "", layout.loose))}
          {layout.categories.flatMap((category) => {
            const channel = byId.get(category.id);
            if (!channel) return [];
            return [
              <CategoryItem
                key={category.id}
                channel={channel}
                active={selected === category.id}
                onPick={() => pick(category.id)}
                onAdd={() => setCreating(category.id)}
                onKeyMove={(e) => moveKey(e, category.id)}
                position={t("serversettings.shared.position", { index: ids.indexOf(category.id) + 1, total: ids.length })}
                movable={arrange}
                locked={isPrivate(channel, serverId)}
                canAdd={hasIn(access, category.id, Permission.MANAGE_CHANNELS)}
              />,
              ...category.children.map((id) => item(id, category.id, category.children)),
            ];
          })}
        </div>
      </div>
      <div ref={editor} className="min-w-0 scroll-mt-4">
        <AnimatePresence mode="wait" initial={false}>
          {selected && byId.get(selected) ? (
            <motion.div key={selected} initial={{ opacity: 0, x: 16 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -16 }} transition={SPRING}>
              <ChannelSettings
                instanceKey={instanceKey}
                serverId={serverId}
                channel={byId.get(selected)!}
                channels={channels}
                initialTab={selected === initial ? initialTab : undefined}
              />
            </motion.div>
          ) : (
            <motion.p key="none" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="py-10 text-center text-sm text-muted-foreground">
              {t("serversettings.channels.pick")}
            </motion.p>
          )}
        </AnimatePresence>
      </div>
      <CreateChannelDialog
        open={creating !== null}
        onOpenChange={(open) => !open && setCreating(null)}
        instanceKey={instanceKey}
        serverId={serverId}
        parentId={creating ?? ""}
        stay
      />
    </div>
  );
}

type ItemProps = {
  channel: Channel;
  active: boolean;
  onPick: () => void;
  onKeyMove: (e: KeyboardEvent) => void;
  position: string;
  /** You can rearrange channels. */
  movable: boolean;
  /** Private: hidden from @everyone. */
  locked: boolean;
};

function Grip({ label, onKeyMove }: { label: string; onKeyMove: (e: KeyboardEvent) => void }) {
  return (
    <button
      type="button"
      data-arrange-handle
      aria-label={label}
      onKeyDown={onKeyMove}
      className="grid size-7 shrink-0 cursor-grab touch-none place-items-center rounded-md text-muted-foreground/60 transition hover:bg-muted hover:text-foreground focus-visible:text-foreground active:cursor-grabbing"
    >
      <GripVerticalIcon className="size-4" />
    </button>
  );
}

function ChannelItem({ channel, parent, active, onPick, onKeyMove, position, movable, locked }: ItemProps & { parent: string }) {
  const lang = useI18n();
  const Icon = CHANNEL_ICON[channel.type] ?? HashIcon;
  return (
    <motion.div
      layout="position"
      data-arrange="channel"
      data-id={channel.id}
      data-parent={parent}
      transition={SPRING}
      className={cn("relative flex items-center gap-0.5 rounded-lg bg-background", parent && "ml-3")}
    >
      {movable && <Grip label={lang.t("serversettings.channels.move", { channel: channel.name, position })} onKeyMove={onKeyMove} />}
      <button
        type="button"
        onClick={onPick}
        className={cn(
          "relative flex h-8 min-w-0 flex-1 items-center gap-1.5 rounded-lg px-2 text-left text-sm transition-colors",
          active ? "font-bold text-primary" : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
        )}
      >
        {active && <motion.span layoutId="channels-editing" transition={SPRING} className="absolute inset-0 rounded-lg bg-primary/12" />}
        <Icon className="relative size-4 shrink-0" />
        <span className="relative truncate">{channel.name}</span>
        <PrivateMark on={locked} />
        {channel.slowmodeSeconds > 0 && (
          <span className="relative ml-auto flex shrink-0 items-center gap-0.5 text-[0.7rem] text-muted-foreground" title={lang.t("serversettings.channels.slowmodeIs", { time: slowLabel(lang, channel.slowmodeSeconds) })}>
            <SnailIcon className="size-3" /> {shortDuration(lang, channel.slowmodeSeconds)}
          </span>
        )}
      </button>
    </motion.div>
  );
}

/** A lock that pops in beside a channel once it's private. */
function PrivateMark({ on }: { on: boolean }) {
  const { t } = useI18n();
  return (
    <AnimatePresence initial={false}>
      {on && (
        <motion.span
          initial={{ scale: 0, rotate: -30 }}
          animate={{ scale: 1, rotate: 0 }}
          exit={{ scale: 0 }}
          transition={{ type: "spring", stiffness: 600, damping: 18 }}
          className="relative shrink-0 text-muted-foreground"
          title={t("serversettings.channels.private")}
        >
          <LockIcon className="size-3" />
        </motion.span>
      )}
    </AnimatePresence>
  );
}

function CategoryItem({
  channel,
  active,
  onPick,
  onAdd,
  onKeyMove,
  position,
  movable,
  locked,
  canAdd,
}: ItemProps & { onAdd: () => void; canAdd: boolean }) {
  const { t } = useI18n();
  return (
    <motion.div
      layout="position"
      data-arrange="category"
      data-id={channel.id}
      transition={SPRING}
      className="group relative mt-2 flex items-center gap-0.5 rounded-lg bg-background"
    >
      {movable && <Grip label={t("serversettings.channels.moveCategory", { category: channel.name, position })} onKeyMove={onKeyMove} />}
      <button
        type="button"
        onClick={onPick}
        className={cn(
          "relative flex h-8 min-w-0 flex-1 items-center gap-1.5 rounded-lg px-2 text-left text-xs font-bold tracking-wide uppercase transition-colors group-data-[drop-into]:text-primary",
          active ? "text-primary" : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
        )}
      >
        {active && <motion.span layoutId="channels-editing" transition={SPRING} className="absolute inset-0 rounded-lg bg-primary/12" />}
        <FolderIcon className="relative size-3.5 shrink-0" />
        <span className="relative truncate">{channel.name}</span>
        <PrivateMark on={locked} />
      </button>
      {canAdd && (
        <button
          type="button"
          aria-label={t("serversettings.channels.createIn", { category: channel.name })}
          onClick={onAdd}
          className="grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition hover:rotate-90 hover:bg-muted hover:text-foreground"
        >
          <PlusIcon className="size-3.5" />
        </button>
      )}
    </motion.div>
  );
}

type Tab = "overview" | "permissions" | "share";

/** The parts of a channel's settings you may open there. */
function useChannelTabs(instanceKey: string, serverId: string, channel: Channel): { value: Tab; label: string }[] {
  const { t } = useI18n();
  const access = useAccess(instanceKey, serverId);
  const sharingOn = useSharingOn(instanceKey);
  const texty = channel.type === ChannelType.TEXT || channel.type === ChannelType.ANNOUNCEMENT;
  // Sharing takes managing the server and the channel, while the instance allows it (or it's shared already).
  const share = texty && has(access, Permission.MANAGE_SERVER) && hasIn(access, channel.id, Permission.MANAGE_CHANNELS) && (sharingOn || !!channel.shared);
  return [
    ...(hasIn(access, channel.id, Permission.MANAGE_CHANNELS) ? [{ value: "overview" as const, label: t("serversettings.nav.overview") }] : []),
    ...(hasIn(access, channel.id, Permission.MANAGE_ROLES) ? [{ value: "permissions" as const, label: t("serversettings.shared.permissions") }] : []),
    ...(share ? [{ value: "share" as const, label: t("serversettings.channels.share") }] : []),
  ];
}

/** One channel's settings: what it is (with Manage Channels there), and who can do what in it (with Manage Roles there). */
function ChannelSettings({
  instanceKey,
  serverId,
  channel,
  channels,
  initialTab,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  channels: Channel[];
  initialTab?: Tab;
}) {
  const { t } = useI18n();
  const tabs = useChannelTabs(instanceKey, serverId, channel);
  const [tab, setTab] = useState<Tab>(initialTab ?? tabs[0]?.value ?? "overview");
  const shown = tabs.some((x) => x.value === tab) ? tab : tabs[0]?.value;
  const Icon = channel.type === ChannelType.CATEGORY ? FolderIcon : (CHANNEL_ICON[channel.type] ?? HashIcon);
  return (
    <div className="flex flex-col">
      <div className="mb-5 flex flex-wrap items-center gap-3">
        <h3 className="flex min-w-0 flex-1 items-center gap-2 text-lg font-extrabold">
          <Icon className="size-5 shrink-0 text-muted-foreground" />
          <span className="truncate">{channel.name}</span>
        </h3>
        {tabs.length > 1 && <Segmented label={t("serversettings.channels.settings")} value={shown ?? "overview"} onChange={setTab} options={tabs} />}
      </div>
      <AnimatePresence mode="wait" initial={false}>
        <motion.div key={shown} initial={{ opacity: 0, y: 10 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={{ duration: 0.18 }}>
          {shown === "overview" && <ChannelEditor instanceKey={instanceKey} serverId={serverId} channel={channel} channels={channels} />}
          {shown === "share" && <ChannelShare instanceKey={instanceKey} serverId={serverId} channel={channel} />}
          {shown === "permissions" && <ChannelPermissions instanceKey={instanceKey} serverId={serverId} channel={channel} channels={channels} />}
          {!shown && <p className="py-10 text-center text-sm text-muted-foreground">{t("serversettings.channels.cantChange")}</p>}
        </motion.div>
      </AnimatePresence>
    </div>
  );
}

type Draft = { name: string; topic: string; parentId: string; slowmode: number };
const draftOf = (c: Channel): Draft => ({ name: c.name, topic: c.topic, parentId: c.parentId, slowmode: c.slowmodeSeconds });

/** Channel names are lowercase with dashes, like the server makes them. */
const slug = (name: string) => name.toLowerCase().replace(/\s+/g, "-").replace(/[^\p{L}\p{N}_-]/gu, "").slice(0, 100);

function ChannelEditor({ instanceKey, serverId, channel, channels }: { instanceKey: string; serverId: string; channel: Channel; channels: Channel[] }) {
  const lang = useI18n();
  const { t } = lang;
  const category = channel.type === ChannelType.CATEGORY;
  /** Categories and voice channels keep their names as typed. */
  const free = category || channel.type === ChannelType.VOICE;
  const texty = channel.type === ChannelType.TEXT || channel.type === ChannelType.ANNOUNCEMENT || channel.type === ChannelType.SECURE;
  const [draft, setDraft] = useState(() => draftOf(channel));
  const base = useMemo(() => draftOf(channel), [channel]);
  const save = useAction(updateChannel);
  const remove = useAction(deleteChannel);
  const [confirming, setConfirming] = useState(false);
  const system = useInstance(instanceKey)?.servers.find((s) => s.id === serverId)?.systemChannelId === channel.id;
  const categories = channels.filter((c) => c.type === ChannelType.CATEGORY);

  // A change saved elsewhere lands in the draft, except in fields being edited here.
  const previous = useRef(base);
  useEffect(() => {
    const before = previous.current;
    previous.current = base;
    setDraft((d) => {
      const next = { ...base };
      for (const k of Object.keys(d) as (keyof Draft)[]) if (d[k] !== before[k]) (next as Record<string, unknown>)[k] = d[k];
      return next;
    });
  }, [base]);

  const changed = (Object.keys(base) as (keyof Draft)[]).filter((k) => draft[k] !== base[k]);
  const set = (patch: Partial<Draft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    save.setError(null);
  };

  async function submit() {
    const name = free ? draft.name.trim() : slug(draft.name);
    if (!name) return save.setError(t("serversettings.channels.needsName"));
    const done = await save.go(instanceKey, serverId, channel.id, {
      ...(draft.name !== base.name && { name }),
      ...(draft.topic !== base.topic && { topic: draft.topic.trim() }),
      ...(draft.parentId !== base.parentId && { parentId: draft.parentId }),
      ...(draft.slowmode !== base.slowmode && { slowmodeSeconds: draft.slowmode }),
    });
    if (done) setDraft(draftOf(done));
  }

  const index = Math.max(0, SLOW.findIndex((s) => s >= draft.slowmode));
  const parentName = categories.find((c) => c.id === draft.parentId)?.name ?? t("serversettings.channels.noCategory");

  return (
    <div className="flex flex-col">
      <Row label={category ? t("serversettings.channels.categoryName") : t("serversettings.channels.channelName")} htmlFor="channel-edit-name">
        <div className="relative">
          {!category && <HashIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />}
          <Input
            id="channel-edit-name"
            maxLength={100}
            value={free ? draft.name : slug(draft.name)}
            onChange={(e) => set({ name: e.target.value })}
            className={cn("h-11 rounded-xl", !category && "pl-9")}
          />
        </div>
      </Row>
      {texty && (
        <Row label={t("serversettings.channels.topic")} htmlFor="channel-edit-topic" hint={t("serversettings.channels.topicHint")}>
          <Textarea id="channel-edit-topic" rows={3} maxLength={1024} value={draft.topic} onChange={(e) => set({ topic: e.target.value })} className="rounded-xl" />
        </Row>
      )}
      {!category && (
        <Row label={t("serversettings.channels.category")}>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button type="button" className="group flex h-11 items-center gap-2 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60">
                <FolderIcon className="size-4 text-muted-foreground" />
                <span className="flex-1 truncate font-bold">{parentName}</span>
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="w-64">
              <DropdownMenuRadioGroup value={draft.parentId} onValueChange={(parentId) => set({ parentId })}>
                <DropdownMenuRadioItem value="">{t("serversettings.channels.noCategory")}</DropdownMenuRadioItem>
                {categories.map((c) => (
                  <DropdownMenuRadioItem key={c.id} value={c.id}>
                    {c.name}
                  </DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuContent>
          </DropdownMenu>
        </Row>
      )}
      {texty && (
        <Row
          id="slowmode"
          label={t("serversettings.nav.slowmode")}
          hint={t("serversettings.channels.slowmodeHint")}
        >
          <div className="flex items-center gap-3">
            <motion.span
              animate={draft.slowmode ? { x: [0, 3, 0], rotate: [0, -4, 0] } : { x: 0, rotate: 0 }}
              transition={draft.slowmode ? { duration: 2.4 - Math.min(index, 12) * 0.12, repeat: Infinity, ease: "easeInOut" } : SPRING}
              className={cn("grid size-10 shrink-0 place-items-center rounded-xl transition-colors", draft.slowmode ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground")}
            >
              <SnailIcon className="size-5" />
            </motion.span>
            <Slider
              label={t("serversettings.nav.slowmode")}
              min={0}
              max={SLOW.length - 1}
              value={index}
              onChange={(i) => set({ slowmode: SLOW[i]! })}
              format={(i) => slowLabel(lang, SLOW[i]!)}
              marks={[
                { value: 0, label: t("serversettings.shared.off") },
                { value: 4, label: shortDuration(lang, SLOW[4]!) },
                { value: 7, label: shortDuration(lang, SLOW[7]!) },
                { value: 11, label: shortDuration(lang, SLOW[11]!) },
                { value: 13, label: shortDuration(lang, SLOW[13]!) },
              ]}
              className="min-w-0 flex-1 px-2"
            />
            <span className="min-w-20 shrink-0 text-right text-sm font-bold whitespace-nowrap tabular-nums">{slowLabel(lang, draft.slowmode)}</span>
          </div>
        </Row>
      )}
      <div className="py-5">
        <AnimatePresence mode="wait" initial={false}>
          {confirming ? (
            <motion.div
              key="confirm"
              initial={{ opacity: 0, y: 8, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, y: 8 }}
              transition={SPRING}
              className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4"
            >
              <p className="flex items-center gap-2 font-bold text-destructive">
                <TriangleAlertIcon className="size-4" />{" "}
                {category ? t("serversettings.channels.deleteCategoryAsk", { name: channel.name }) : t("serversettings.channels.deleteChannelAsk", { name: channel.name })}
              </p>
              <p className="text-sm text-muted-foreground">
                {category
                  ? t("serversettings.channels.deleteCategoryHint")
                  : system
                    ? t("serversettings.channels.deleteSystemHint")
                    : t("serversettings.channels.deleteChannelHint")}
              </p>
              {remove.error && <p className="text-sm text-destructive first-letter:uppercase">{remove.error}</p>}
              <div className="flex justify-end gap-2">
                <Button type="button" variant="ghost" onClick={() => setConfirming(false)} className="rounded-xl">
                  {t("serversettings.shared.keepIt")}
                </Button>
                <Button
                  type="button"
                  variant="destructive"
                  disabled={remove.pending}
                  onClick={async () => {
                    if ((await remove.go(instanceKey, serverId, channel.id)) !== undefined) toast(category ? t("serversettings.shared.deleted", { name: channel.name }) : t("serversettings.shared.deletedChannel", { name: channel.name }));
                  }}
                  className="rounded-xl font-bold"
                >
                  {remove.pending ? <LoaderCircleIcon className="animate-spin" /> : <Trash2Icon />} {t("serversettings.shared.delete")}
                </Button>
              </div>
            </motion.div>
          ) : (
            <motion.div key="button" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
              <Button type="button" variant="ghost" onClick={() => setConfirming(true)} className="group rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive">
                <Trash2Icon className="transition-transform group-hover:-rotate-12" />{" "}
                {category ? t("serversettings.channels.deleteCategory") : t("serversettings.channels.deleteChannel")}
              </Button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      <SaveBar count={changed.length} saving={save.pending} error={save.error} onSave={() => void submit()} onDiscard={() => setDraft(base)} />
    </div>
  );
}
