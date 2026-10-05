import {
  CheckIcon,
  GripVerticalIcon,
  LoaderCircleIcon,
  LockIcon,
  PlusIcon,
  SearchIcon,
  ShieldAlertIcon,
  Trash2Icon,
  TriangleAlertIcon,
  UserMinusIcon,
  UserPlusIcon,
  UsersIcon,
} from "lucide-react";
import { AnimatePresence, m as motion, Reorder, useDragControls } from "motion/react";
import { useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { Permission, type Member, type Role } from "@/gen/fuwa/v1/types_pb";
import { createRole, deleteRole, giveRole, reorderRoles, run, takeRole, updateRole } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useAction, useInstance, useRoles } from "@/fuwa/hooks";
import { RoleDot } from "@/components/chat/mentions";
import { UserAvatar } from "@/components/Icons";
import { Count, SwapText } from "@/components/motion";
import { SPRING } from "@/lib/motion";
import { RoleName } from "@/components/RoleName";
import { Row, Segmented } from "@/components/settings/account/common";
import { SaveBar, Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { T, useI18n } from "@/i18n/react";
import { memberName } from "@/lib/format";
import {
  above,
  bit,
  cssColor,
  fromList,
  has,
  mayChange,
  PERMISSION_GROUPS,
  permissionInfo,
  toList,
  type Access,
  type Bits,
} from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Colors to pick from, light to deep, then a custom one. */
const SWATCHES = [
  0xf472b6, 0xfb7185, 0xf87171, 0xfb923c, 0xfbbf24, 0xa3e635, 0x34d399, 0x2dd4bf, 0x38bdf8, 0x60a5fa, 0x818cf8, 0xa78bfa, 0xe879f9,
  0xdb2777, 0xe11d48, 0xea580c, 0xca8a04, 0x16a34a, 0x0d9488, 0x0284c7, 0x4f46e5, 0x7c3aed, 0x94a3b8, 0x64748b,
];

type Tab = "display" | "permissions" | "members";

/**
 * The roles' order as shown: the store's, or one moved here until the roles
 * change (the server took it, or something else did), but never mid-drag.
 */
function useRankOrder(instanceKey: string, serverId: string, ranked: Role[]) {
  const rankedIds = useMemo(() => ranked.map((r) => r.id), [ranked]);
  const [moved, setMoved] = useState<{ order: string[]; over: string[] } | null>(null);
  const [dragging, setDragging] = useState(false);
  const order = moved && (dragging || moved.over === rankedIds) ? moved.order : rankedIds;
  const latest = useRef(order);
  useLayoutEffect(() => {
    latest.current = order;
  });

  const commit = (next: string[]) => {
    setDragging(false);
    setMoved({ order: next, over: rankedIds });
    if (next.join() === rankedIds.join()) return;
    run(reorderRoles(instanceKey, serverId, next)).catch((err: FuwaError) => {
      toast(err.message);
      setMoved(null);
    });
  };
  const nudge = (id: string, by: -1 | 1) => {
    const at = order.indexOf(id);
    const to = at + by;
    if (at === -1 || to < 0 || to >= order.length) return;
    const next = [...order];
    [next[at], next[to]] = [next[to]!, next[at]!];
    commit(next);
  };
  const reorder = (next: string[]) => {
    setDragging(true);
    setMoved({ order: next, over: rankedIds });
  };

  return { order, reorder, drop: () => commit(latest.current), nudge };
}

/**
 * Every role, highest first, dragged into rank by its handle (only those
 * below your own move), and the chosen one beside the list: how it looks,
 * what it can do and who has it. @everyone sits at the bottom and holds
 * what everybody can do.
 */
export function Roles({ instanceKey, serverId, initial }: { instanceKey: string; serverId: string; initial?: string | null }) {
  const { t } = useI18n();
  const roles = useRoles(instanceKey, serverId);
  const access = useAccess(instanceKey, serverId);
  const members = useInstance(instanceKey)?.members[serverId];
  const ranked = useMemo(() => roles.filter((r) => r.id !== serverId), [roles, serverId]);
  const everyone = roles.find((r) => r.id === serverId);
  const { order, reorder, drop, nudge } = useRankOrder(instanceKey, serverId, ranked);
  const [picked, setSelected] = useState<string>(initial ?? ranked[0]?.id ?? serverId);
  const [chosenTab, setTab] = useState<Tab>("display");
  const creating = useAction(createRole);
  const editor = useRef<HTMLDivElement>(null);
  const byId = useMemo(() => new Map(roles.map((r) => [r.id, r])), [roles]);
  const counts = useMemo(() => {
    const out = new Map<string, number>();
    for (const m of members ?? []) for (const id of m.roleIds) out.set(id, (out.get(id) ?? 0) + 1);
    return out;
  }, [members]);

  // A role deleted elsewhere falls back to the first; @everyone has no members tab.
  const selected = byId.has(picked) ? picked : (ranked[0]?.id ?? serverId);
  const tab = selected === serverId && chosenTab === "members" ? "permissions" : chosenTab;

  const pick = (id: string) => {
    setSelected(id);
    if (id === serverId && tab === "members") setTab("permissions");
    if (window.matchMedia("(max-width: 1023px)").matches) setTimeout(() => editor.current?.scrollIntoView({ behavior: "smooth", block: "start" }), 50);
  };

  async function create() {
    const role = await creating.go(instanceKey, serverId, { name: t("serversettings.roles.newRole"), permissions: [], hoist: false, mentionable: false });
    if (!role) return toast(creating.error ?? t("serversettings.roles.createFailed"));
    setSelected(role.id);
    setTab("display");
  }

  const role = byId.get(selected);
  const canCreate = has(access, Permission.MANAGE_ROLES);

  return (
    <div className="grid gap-6 lg:grid-cols-[minmax(0,17rem)_minmax(0,1fr)] lg:gap-8">
      <div className="flex flex-col gap-3">
        <div className="flex items-center justify-between gap-2">
          <p className="text-xs text-muted-foreground">{t("serversettings.roles.intro")}</p>
          {canCreate && (
            <Button type="button" size="sm" onClick={() => void create()} disabled={creating.pending} className="btn shrink-0 rounded-xl font-bold">
              {creating.pending ? <LoaderCircleIcon className="animate-spin" /> : <PlusIcon />} {t("serversettings.shared.new")}
            </Button>
          )}
        </div>
        <div className="rounded-2xl border bg-background/40 p-2">
          <Reorder.Group
            axis="y"
            values={order}
            onReorder={reorder}
            className="flex flex-col gap-0.5"
          >
            <AnimatePresence initial={false}>
              {order.map((id, n) => {
                const r = byId.get(id);
                if (!r) return null;
                return (
                  <RoleItem
                    key={id}
                    role={r}
                    count={counts.get(id) ?? 0}
                    locked={!above(access, r.position)}
                    active={selected === id}
                    position={t("serversettings.shared.position", { index: n + 1, total: order.length })}
                    onPick={() => pick(id)}
                    onDragEnd={drop}
                    onKeyMove={(e) => {
                      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
                      e.preventDefault();
                      nudge(id, e.key === "ArrowUp" ? -1 : 1);
                    }}
                  />
                );
              })}
            </AnimatePresence>
          </Reorder.Group>
          {everyone && (
            <button
              type="button"
              onClick={() => pick(everyone.id)}
              className={cn(
                "relative mt-2 flex h-11 w-full items-center gap-2 rounded-xl border border-dashed px-3 text-left text-sm transition-colors",
                selected === everyone.id ? "font-bold text-primary" : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
              )}
            >
              {selected === everyone.id && <motion.span layoutId="roles-editing" transition={SPRING} className="absolute inset-0 rounded-xl bg-primary/12" />}
              <UsersIcon className="relative size-4 shrink-0" />
              <span className="relative min-w-0 flex-1">
                <span className="block truncate">@everyone</span>
                <span className="block truncate text-[0.7rem] font-normal text-muted-foreground">{t("serversettings.roles.everyoneHint")}</span>
              </span>
            </button>
          )}
        </div>
      </div>
      <div ref={editor} className="min-w-0 scroll-mt-4">
        <AnimatePresence mode="wait" initial={false}>
          {role ? (
            <motion.div key={role.id} initial={{ opacity: 0, x: 16 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -16 }} transition={SPRING}>
              <RoleEditor instanceKey={instanceKey} serverId={serverId} role={role} access={access} tab={tab} onTab={setTab} members={members ?? []} />
            </motion.div>
          ) : (
            <motion.p key="none" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="py-10 text-center text-sm text-muted-foreground">
              {t("serversettings.roles.pick")}
            </motion.p>
          )}
        </AnimatePresence>
      </div>
    </div>
  );
}

function RoleItem({
  role,
  count,
  locked,
  active,
  position,
  onPick,
  onDragEnd,
  onKeyMove,
}: {
  role: Role;
  count: number;
  locked: boolean;
  active: boolean;
  position: string;
  onPick: () => void;
  onDragEnd: () => void;
  onKeyMove: (e: KeyboardEvent) => void;
}) {
  const { t } = useI18n();
  const controls = useDragControls();
  return (
    <Reorder.Item
      value={role.id}
      dragListener={false}
      dragControls={controls}
      onDragEnd={onDragEnd}
      initial={{ opacity: 0, scale: 0.9 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.9, height: 0 }}
      whileDrag={{ scale: 1.03, boxShadow: "0 12px 30px -12px rgb(0 0 0 / 0.45)", zIndex: 10 }}
      transition={SPRING}
      className="relative flex items-center gap-0.5 rounded-lg bg-background"
    >
      {locked ? (
        <span className="grid size-7 shrink-0 place-items-center text-muted-foreground/50" title={t("serversettings.roles.lockedTitle")}>
          <LockIcon className="size-3.5" />
        </span>
      ) : (
        <button
          type="button"
          aria-label={t("serversettings.roles.move", { role: role.name, position })}
          onPointerDown={(e) => {
            e.preventDefault();
            controls.start(e);
          }}
          onKeyDown={onKeyMove}
          className="grid size-7 shrink-0 cursor-grab touch-none place-items-center rounded-md text-muted-foreground/60 transition hover:bg-muted hover:text-foreground focus-visible:text-foreground active:cursor-grabbing"
        >
          <GripVerticalIcon className="size-4" />
        </button>
      )}
      <button
        type="button"
        onClick={onPick}
        className={cn(
          "relative flex h-9 min-w-0 flex-1 items-center gap-2 rounded-lg px-2 text-left text-sm transition-colors",
          active ? "font-bold" : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
        )}
      >
        {active && <motion.span layoutId="roles-editing" transition={SPRING} className="absolute inset-0 rounded-lg bg-primary/12" />}
        <motion.span key={role.color ?? "none"} initial={{ scale: 0.3 }} animate={{ scale: 1 }} transition={{ type: "spring", stiffness: 600, damping: 15 }} className="relative">
          <RoleDot role={role} />
        </motion.span>
        <span className="relative truncate" style={active && role.color !== undefined ? { color: cssColor(role.color) } : undefined}>
          <SwapText className="truncate align-bottom">{role.name}</SwapText>
        </span>
        <span className="relative ml-auto flex shrink-0 items-center gap-1 text-[0.7rem] font-bold text-muted-foreground tabular-nums">
          <UsersIcon className="size-3" /> <Count value={count} />
        </span>
      </button>
    </Reorder.Item>
  );
}

type Draft = { name: string; color: number | null; hoist: boolean; mentionable: boolean; permissions: Bits };

const draftOf = (r: Role): Draft => ({
  name: r.name,
  color: r.color ?? null,
  hoist: r.hoist,
  mentionable: r.mentionable,
  permissions: fromList(r.permissions),
});

/**
 * Your edits over the role as saved. A change saved elsewhere shows at once,
 * except in fields being edited here.
 */
function useRoleDraft(instanceKey: string, serverId: string, role: Role) {
  const { t } = useI18n();
  const base = useMemo(() => draftOf(role), [role]);
  const [edits, setEdits] = useState<Partial<Draft>>({});
  const save = useAction(updateRole);
  const draft: Draft = { ...base, ...edits };

  const changed = (Object.keys(base) as (keyof Draft)[]).filter((k) => draft[k] !== base[k]);
  const set = (patch: Partial<Draft>) => {
    setEdits((e) => {
      const next: Partial<Draft> = { ...e, ...patch };
      // A field put back as it's saved isn't being edited any more.
      for (const k of Object.keys(patch) as (keyof Draft)[]) if (next[k] === base[k]) delete next[k];
      return next;
    });
    save.setError(null);
  };

  async function submit() {
    const name = draft.name.trim();
    if (!name) return save.setError(t("serversettings.roles.needsName"));
    const done = await save.go(instanceKey, serverId, role.id, {
      ...(draft.name !== base.name && { name }),
      ...(draft.color !== base.color && { color: draft.color }),
      ...(draft.hoist !== base.hoist && { hoist: draft.hoist }),
      ...(draft.mentionable !== base.mentionable && { mentionable: draft.mentionable }),
      ...(draft.permissions !== base.permissions && { permissions: toList(draft.permissions) }),
    });
    // The saved role is in the store already, so it's the base now.
    if (done) setEdits({});
  }

  return { draft, set, changed, submit, discard: () => setEdits({}), save };
}

/** The role's dot and name as they'll look. */
function RoleTitle({ draft }: { draft: Draft }) {
  const { t } = useI18n();
  return (
    <span className="inline-flex items-center gap-2">
      <motion.span key={draft.color ?? "none"} initial={{ scale: 0.3, rotate: -90 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 14 }}>
        <RoleDot role={{ color: draft.color ?? undefined }} className="size-4" />
      </motion.span>
      <span className="truncate" style={draft.color !== null ? { color: cssColor(draft.color) } : undefined}>
        {draft.name || t("serversettings.roles.newRole")}
      </span>
    </span>
  );
}

function RoleEditor({
  instanceKey,
  serverId,
  role,
  access,
  tab,
  onTab,
  members,
}: {
  instanceKey: string;
  serverId: string;
  role: Role;
  access: Access;
  tab: Tab;
  onTab: (tab: Tab) => void;
  members: Member[];
}) {
  const { t } = useI18n();
  const everyone = role.id === serverId;
  const locked = !everyone && !above(access, role.position);
  const { draft, set, changed, submit, discard, save } = useRoleDraft(instanceKey, serverId, role);

  const holders = members.filter((m) => m.roleIds.includes(role.id));
  const tabs = [
    ...(everyone ? [] : [{ value: "display" as const, label: t("serversettings.roles.display") }]),
    { value: "permissions" as const, label: t("serversettings.shared.permissions") },
    ...(everyone ? [] : [{ value: "members" as const, label: t("serversettings.roles.membersTab", { count: holders.length }) }]),
  ];

  return (
    <div className="flex flex-col">
      <div className="mb-5 flex flex-wrap items-center gap-3">
        <h3 className="min-w-0 flex-1 truncate text-lg font-extrabold">{everyone ? "@everyone" : <RoleTitle draft={draft} />}</h3>
        {tabs.length > 1 && <Segmented label={t("serversettings.roles.settings")} value={tab} onChange={onTab} options={tabs} />}
      </div>
      <AnimatePresence>
        {locked && (
          <motion.p
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            className="mb-4 flex items-center gap-2 overflow-hidden rounded-xl bg-muted/60 px-3 py-2 text-sm text-muted-foreground"
          >
            <LockIcon className="size-4 shrink-0" /> {t("serversettings.roles.locked")}
          </motion.p>
        )}
      </AnimatePresence>
      <AnimatePresence mode="wait" initial={false}>
        <motion.div
          key={tab}
          initial={{ opacity: 0, y: 10 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -6 }}
          transition={{ duration: 0.18 }}
        >
          {tab === "display" && !everyone && (
            <Display instanceKey={instanceKey} serverId={serverId} role={role} draft={draft} set={set} locked={locked} />
          )}
          {tab === "permissions" && <Permissions draft={draft} set={set} access={access} locked={locked} everyone={everyone} />}
          {tab === "members" && !everyone && (
            <RoleMembers instanceKey={instanceKey} serverId={serverId} role={role} members={members} holders={holders} locked={locked} />
          )}
        </motion.div>
      </AnimatePresence>
      <SaveBar count={changed.length} saving={save.pending} error={save.error} onSave={() => void submit()} onDiscard={discard} />
    </div>
  );
}

function Display({
  instanceKey,
  serverId,
  role,
  draft,
  set,
  locked,
}: {
  instanceKey: string;
  serverId: string;
  role: Role;
  draft: Draft;
  set: (patch: Partial<Draft>) => void;
  locked: boolean;
}) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col">
      <Row label={t("serversettings.roles.name")} htmlFor="role-name">
        <Input id="role-name" maxLength={100} value={draft.name} disabled={locked} onChange={(e) => set({ name: e.target.value })} className="h-11 rounded-xl" />
      </Row>
      <Row label={t("serversettings.roles.color")} hint={t("serversettings.roles.colorHint")}>
        <ColorChoice color={draft.color} locked={locked} onPick={(color) => set({ color })} />
      </Row>
      <Row label={t("serversettings.roles.howItShows")}>
        <div className="flex flex-col gap-4">
          <Toggle
            checked={draft.hoist}
            disabled={locked}
            onChange={(hoist) => set({ hoist })}
            label={t("serversettings.roles.hoist")}
            hint={t("serversettings.roles.hoistHint")}
          />
          <Toggle
            checked={draft.mentionable}
            disabled={locked}
            onChange={(mentionable) => set({ mentionable })}
            label={t("serversettings.roles.mentionable")}
            hint={t("serversettings.roles.mentionableHint")}
          />
        </div>
      </Row>
      <Row label={t("settings.controls.preview")}>
        <RolePreview instanceKey={instanceKey} draft={draft} />
      </Row>
      <DeleteRole instanceKey={instanceKey} serverId={serverId} role={role} locked={locked} />
    </div>
  );
}

/** No color, the swatches, or any color at all. */
function ColorChoice({ color, locked, onPick }: { color: number | null; locked: boolean; onPick: (color: number | null) => void }) {
  const { t } = useI18n();
  const custom = color !== null && !SWATCHES.includes(color);
  return (
    <div role="radiogroup" aria-label={t("serversettings.roles.colorLabel")} className="flex flex-wrap gap-2">
      <Swatch label={t("serversettings.roles.noColor")} selected={color === null} disabled={locked} onPick={() => onPick(null)} />
      {SWATCHES.map((c) => (
        <Swatch key={c} color={c} label={cssColor(c)} selected={color === c} disabled={locked} onPick={() => onPick(c)} />
      ))}
      <label
        className={cn(
          "relative grid size-9 cursor-pointer place-items-center overflow-hidden rounded-full border-2 border-dashed transition hover:scale-110",
          custom ? "border-transparent ring-2 ring-foreground ring-offset-2 ring-offset-background" : "border-muted-foreground/40",
          locked && "pointer-events-none opacity-50",
        )}
        style={custom ? { background: cssColor(color!) } : { background: "conic-gradient(#f472b6, #fbbf24, #34d399, #38bdf8, #a78bfa, #f472b6)" }}
        title={t("serversettings.roles.anyColor")}
      >
        <input
          type="color"
          aria-label={t("serversettings.roles.customColor")}
          disabled={locked}
          value={color !== null ? cssColor(color) : "#f472b6"}
          onChange={(e) => onPick(parseInt(e.target.value.slice(1), 16))}
          className="absolute inset-0 cursor-pointer opacity-0"
        />
        {custom && <CheckIcon className="size-4 text-white mix-blend-difference" strokeWidth={3} />}
      </label>
    </div>
  );
}

/** How the role shows: your name in its color, a mention of it, and its heading in the member list. */
function RolePreview({ instanceKey, draft }: { instanceKey: string; draft: Draft }) {
  const { t } = useI18n();
  const me = useInstance(instanceKey)?.me;
  return (
    <div className="flex flex-col gap-3 rounded-2xl bg-muted/50 p-3">
      <div className="flex items-center gap-2.5">
        <UserAvatar user={me ?? undefined} className="size-9" />
        <div className="min-w-0">
          <RoleName id={me?.id ?? ""} name={me?.displayName || me?.username || t("serversettings.shared.you")} color={draft.color ?? undefined} />
          <p className="text-sm text-muted-foreground">
            <T
              k="serversettings.roles.previewLine"
              values={{
                role: (
                  <span
                    className="mention rounded-md px-1 font-bold"
                    style={
                      draft.color !== null
                        ? { color: cssColor(draft.color), backgroundColor: `color-mix(in srgb, ${cssColor(draft.color)} 15%, transparent)` }
                        : undefined
                    }
                  >
                    @{draft.name || t("serversettings.roles.newRole")}
                  </span>
                ),
              }}
            />
          </p>
        </div>
      </div>
      <AnimatePresence initial={false}>
        {draft.hoist && (
          <motion.p
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            className="flex items-center gap-1.5 overflow-hidden text-xs font-bold tracking-wide text-muted-foreground uppercase"
          >
            <RoleDot role={{ color: draft.color ?? undefined }} className="size-2" /> {draft.name || t("serversettings.roles.newRole")} — 1
          </motion.p>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Deleting the role, asked once more first. Hidden from those who can't change it. */
function DeleteRole({ instanceKey, serverId, role, locked }: { instanceKey: string; serverId: string; role: Role; locked: boolean }) {
  const { t } = useI18n();
  const [confirming, setConfirming] = useState(false);
  const remove = useAction(deleteRole);
  return (
    <div hidden={locked} className="py-5">
      <AnimatePresence mode="wait" initial={false}>
        {locked ? null : confirming ? (
          <motion.div
            key="confirm"
            initial={{ opacity: 0, y: 8, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8 }}
            transition={SPRING}
            className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4"
          >
            <p className="flex items-center gap-2 font-bold text-destructive">
              <TriangleAlertIcon className="size-4" /> {t("serversettings.roles.deleteAsk", { role: role.name })}
            </p>
            <p className="text-sm text-muted-foreground">{t("serversettings.roles.deleteHint")}</p>
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
                  if ((await remove.go(instanceKey, serverId, role.id)) !== undefined) toast(t("serversettings.shared.deleted", { name: role.name }));
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
              <Trash2Icon className="transition-transform group-hover:-rotate-12" /> {t("serversettings.roles.delete")}
            </Button>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function Swatch({ color, label, selected, disabled, onPick }: { color?: number; label: string; selected: boolean; disabled: boolean; onPick: () => void }) {
  return (
    <motion.button
      type="button"
      role="radio"
      aria-checked={selected}
      aria-label={label}
      title={label}
      disabled={disabled}
      whileHover={disabled ? undefined : { scale: 1.12 }}
      whileTap={disabled ? undefined : { scale: 0.9 }}
      onClick={onPick}
      className={cn(
        "relative grid size-9 place-items-center rounded-full transition-shadow disabled:opacity-50",
        color === undefined && "border-2 border-muted-foreground/40 bg-background",
        selected && "ring-2 ring-foreground ring-offset-2 ring-offset-background",
      )}
      style={color !== undefined ? { background: cssColor(color) } : undefined}
    >
      {color === undefined && <span className="h-0.5 w-5 rotate-45 rounded-full bg-muted-foreground/60" />}
      <AnimatePresence>
        {selected && (
          <motion.span initial={{ scale: 0, rotate: -60 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={{ type: "spring", stiffness: 600, damping: 16 }} className="absolute">
            <CheckIcon className={cn("size-4", color === undefined ? "text-foreground" : "text-white drop-shadow")} strokeWidth={3} />
          </motion.span>
        )}
      </AnimatePresence>
    </motion.button>
  );
}

function Permissions({
  draft,
  set,
  access,
  locked,
  everyone,
}: {
  draft: Draft;
  set: (patch: Partial<Draft>) => void;
  access: Access;
  locked: boolean;
  everyone: boolean;
}) {
  const { t } = useI18n();
  const [query, setQuery] = useState("");
  const q = query.trim().toLowerCase();
  const groups = PERMISSION_GROUPS.map((g) => ({
    ...g,
    permissions: g.permissions.filter((p) => !q || `${t(permissionInfo(p).label)} ${t(permissionInfo(p).about)}`.toLowerCase().includes(q)),
  })).filter((g) => g.permissions.length);
  const count = toList(draft.permissions).length;
  return (
    <div data-setting="role-permissions" className="flex flex-col gap-4">
      <p className="text-sm text-muted-foreground">
        {everyone ? t("serversettings.roles.everyoneIntro") : t("serversettings.roles.roleIntro")}
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 basis-48">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("serversettings.roles.search")} aria-label={t("serversettings.roles.search")} className="h-10 rounded-xl pl-9" />
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={locked || !count}
          onClick={() => set({ permissions: draft.permissions & ~toList(draft.permissions).reduce((b, p) => (mayChange(access, bit(p), access.server) ? b | bit(p) : b), 0) })}
          className="rounded-xl"
        >
          {t("serversettings.roles.clearAll")}
        </Button>
      </div>
      {groups.map((group, g) => (
        <section key={group.title}>
          <h4 className="mb-1 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t(group.title)}</h4>
          <ul className="flex flex-col">
            {group.permissions.map((p, n) => {
              const info = permissionInfo(p);
              const on = !!(draft.permissions & bit(p));
              const allowed = mayChange(access, bit(p), access.server);
              const admin = p === Permission.ADMINISTRATOR;
              return (
                <motion.li
                  key={p}
                  initial={{ opacity: 0, y: 6 }}
                  animate={{ opacity: 1, y: 0 }}
                  transition={{ ...SPRING, delay: Math.min(g * 4 + n, 16) * 0.015 }}
                  className={cn(
                    "flex items-center gap-4 border-b border-border/60 py-3 last:border-b-0",
                    admin && on && "-mx-3 rounded-xl border-transparent bg-destructive/8 px-3",
                  )}
                >
                  <div className="min-w-0 flex-1">
                    <p className="flex items-center gap-1.5 text-sm font-bold">
                      {admin && <ShieldAlertIcon className={cn("size-4", on ? "text-destructive" : "text-muted-foreground")} />}
                      {t(info.label)}
                      {!allowed && !locked && (
                        <span className="flex items-center gap-1 rounded-full bg-muted px-2 py-0.5 text-[0.65rem] font-bold text-muted-foreground" title={t("serversettings.roles.notYoursHint")}>
                          <LockIcon className="size-3" /> {t("serversettings.roles.notYours")}
                        </span>
                      )}
                    </p>
                    <p className="text-xs text-muted-foreground">{t(info.about)}</p>
                  </div>
                  <Switch
                    checked={on}
                    disabled={locked || !allowed}
                    onCheckedChange={(checked) => set({ permissions: checked ? draft.permissions | bit(p) : draft.permissions & ~bit(p) })}
                    aria-label={t(info.label)}
                  />
                </motion.li>
              );
            })}
          </ul>
        </section>
      ))}
      {!groups.length && <p className="py-6 text-center text-sm text-muted-foreground">{t("serversettings.roles.noMatch")}</p>}
    </div>
  );
}

function RoleMembers({
  instanceKey,
  serverId,
  role,
  members,
  holders,
  locked,
}: {
  instanceKey: string;
  serverId: string;
  role: Role;
  members: Member[];
  holders: Member[];
  locked: boolean;
}) {
  const { t } = useI18n();
  const [adding, setAdding] = useState(false);
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const q = query.trim().toLowerCase();
  const others = members
    .filter((m) => !m.roleIds.includes(role.id))
    .filter((m) => !q || `${memberName(m)} ${m.user?.username ?? ""}`.toLowerCase().includes(q))
    .slice(0, 30);

  const toggle = async (member: Member, give: boolean) => {
    const id = member.user?.id ?? "";
    setBusy(id);
    try {
      await run((give ? giveRole : takeRole)(instanceKey, serverId, id, role.id));
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div data-setting="role-members" className="flex flex-col gap-3">
      <div hidden={locked} className="flex flex-col gap-2">
        <Button type="button" variant={adding ? "secondary" : "outline"} onClick={() => setAdding((a) => !a)} className="self-start rounded-xl font-bold">
          <UserPlusIcon className={cn("transition-transform duration-300", adding && "rotate-12")} /> {t("serversettings.roles.addMembers")}
        </Button>
        <AnimatePresence initial={false}>
          {!locked && adding && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: "auto" }}
              exit={{ opacity: 0, height: 0 }}
              transition={{ duration: 0.25, ease: [0.22, 1, 0.36, 1] }}
              className="overflow-hidden"
            >
              <div className="flex flex-col gap-2 rounded-2xl border bg-background/40 p-2">
                <div className="relative">
                  <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
                  <Input autoFocus value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("serversettings.shared.findSomeone")} aria-label={t("serversettings.shared.findSomeone")} className="h-10 rounded-xl pl-9" />
                </div>
                <ul className="scroll-thin flex max-h-60 flex-col gap-0.5 overflow-y-auto">
                  <AnimatePresence initial={false} mode="popLayout">
                    {others.map((m) => (
                      <motion.li key={m.user?.id} layout initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 24, transition: { duration: 0.15 } }} transition={SPRING}>
                        <button
                          type="button"
                          disabled={!!busy}
                          onClick={() => void toggle(m, true)}
                          className="group flex w-full items-center gap-2.5 rounded-xl px-2 py-1.5 text-left text-sm transition hover:bg-primary/10"
                        >
                          <UserAvatar user={m.user} className="size-7" />
                          <span className="min-w-0 flex-1 truncate font-bold">{memberName(m)}</span>
                          <span className="truncate text-xs text-muted-foreground">@{m.user?.username}</span>
                          <PlusIcon className="size-4 text-primary opacity-0 transition group-hover:rotate-90 group-hover:opacity-100" />
                        </button>
                      </motion.li>
                    ))}
                  </AnimatePresence>
                  {!others.length && <li className="px-2 py-3 text-center text-sm text-muted-foreground">{q ? t("serversettings.shared.nobodyMatches") : t("serversettings.roles.everyoneHas")}</li>}
                </ul>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {holders.map((m, n) => (
            <motion.li
              key={m.user?.id}
              layout
              initial={{ opacity: 0, y: 10, scale: 0.98 }}
              animate={{ opacity: busy === m.user?.id ? 0.5 : 1, y: 0, scale: 1, transition: { ...SPRING, delay: Math.min(n, 12) * 0.02 } }}
              exit={{ opacity: 0, x: -24, transition: { duration: 0.2 } }}
              transition={SPRING}
              className="group flex items-center gap-3 rounded-2xl border bg-background/40 p-2 pr-2 transition-colors hover:border-primary/30"
            >
              <UserAvatar user={m.user} className="size-9" />
              <div className="min-w-0 flex-1">
                <RoleName id={m.user?.id ?? ""} name={memberName(m)} color={role.color} />
                <p className="truncate text-xs text-muted-foreground">@{m.user?.username}</p>
              </div>
              {!locked && (
                <button
                  type="button"
                  aria-label={t("serversettings.roles.take", { role: role.name, name: memberName(m) })}
                  disabled={!!busy}
                  onClick={() => void toggle(m, false)}
                  className="grid size-9 place-items-center rounded-xl text-muted-foreground transition hover:bg-destructive/10 hover:text-destructive"
                >
                  <UserMinusIcon className="size-4 transition-transform group-hover:-rotate-6" />
                </button>
              )}
            </motion.li>
          ))}
        </AnimatePresence>
      </ul>
      <AnimatePresence>
        {!holders.length && (
          <motion.p initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="py-6 text-center text-sm text-muted-foreground">
            {t("serversettings.roles.nobody")}
          </motion.p>
        )}
      </AnimatePresence>
    </div>
  );
}
