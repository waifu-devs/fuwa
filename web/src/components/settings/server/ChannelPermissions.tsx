import { CheckIcon, LinkIcon, LockIcon, LockOpenIcon, PlusIcon, SlashIcon, Trash2Icon, TriangleAlertIcon, UnlinkIcon, UserIcon, UsersIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from "react";
import { ChannelType, OverwriteTarget, Permission, type Channel, type Member, type PermissionOverwrite, type Role } from "@/gen/fuwa/v1/types_pb";
import { setChannelPermissions } from "@/fuwa/actions";
import { useAccess, useAction, useInstance, useRoles } from "@/fuwa/hooks";
import { RoleDot } from "@/components/chat/mentions";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/lib/motion";
import { SaveBar } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Switch } from "@/components/ui/switch";
import { T, useI18n } from "@/i18n/react";
import { memberName } from "@/lib/format";
import { accessOf, bit, canSee, CHANNEL_GROUPS, fromList, inChannel, mayChange, permissionInfo, toList, type Bits } from "@/lib/permissions";
import { cn } from "@/lib/utils";

/**
 * Who can see and do what in one channel or category: a private switch for
 * the common case, and for each role or person, every channel permission
 * allowed, denied, or left to their roles. Saved all at once.
 */

type Overwrite = { targetId: string; member: boolean; allow: Bits; deny: Bits };

const VIEW = bit(Permission.VIEW_CHANNELS);

const fromWire = (list: PermissionOverwrite[]): Overwrite[] =>
  list.map((o) => ({ targetId: o.targetId, member: o.target === OverwriteTarget.MEMBER, allow: fromList(o.allow), deny: fromList(o.deny) }));

const toWire = (list: Overwrite[]) =>
  list
    .filter((o) => o.allow || o.deny)
    .map((o) => ({
      targetId: o.targetId,
      target: o.member ? OverwriteTarget.MEMBER : OverwriteTarget.ROLE,
      allow: toList(o.allow),
      deny: toList(o.deny),
    }));

const key = (list: Overwrite[]) =>
  JSON.stringify(
    list
      .filter((o) => o.allow || o.deny)
      .map((o) => [o.targetId, o.member, o.allow, o.deny])
      .sort(),
  );

type State = "allow" | "inherit" | "deny";

export function ChannelPermissions({ instanceKey, serverId, channel, channels }: { instanceKey: string; serverId: string; channel: Channel; channels: Channel[] }) {
  const inst = useInstance(instanceKey);
  const roles = useRoles(instanceKey, serverId);
  const access = useAccess(instanceKey, serverId);
  const members = inst?.members[serverId] ?? [];
  const category = channel.type === ChannelType.CATEGORY;
  const parent = channels.find((c) => c.id === channel.parentId && c.type === ChannelType.CATEGORY);

  const base = useMemo(() => fromWire(channel.permissionOverwrites), [channel.permissionOverwrites]);
  const [draft, setDraft] = useState(base);
  // Whose permissions are open below; none picked yet means @everyone.
  const [picked, setPicked] = useState<string | null>(null);
  const save = useAction(setChannelPermissions);
  // A change saved elsewhere replaces the draft unless it's being edited.
  const previous = useRef(base);
  useEffect(() => {
    if (key(draft) === key(previous.current)) setDraft(base);
    previous.current = base;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [base]);

  const find = (id: string) => draft.find((o) => o.targetId === id);
  const everyone = find(serverId) ?? { targetId: serverId, member: false, allow: 0, deny: 0 };
  const isPrivate = !!(everyone.deny & VIEW);
  const dirty = key(draft) !== key(base);
  const changes = useMemo(() => countChanges(base, draft), [base, draft]);

  /** Changes one target's overwrite, adding it if it's new. */
  const put = (targetId: string, member: boolean, fn: (o: Overwrite) => Overwrite) =>
    setDraft((list) => {
      const current = list.find((o) => o.targetId === targetId) ?? { targetId, member, allow: 0, deny: 0 };
      const next = fn(current);
      const rest = list.filter((o) => o.targetId !== targetId);
      return [...rest, next];
    });

  const setPrivate = (on: boolean) =>
    put(serverId, false, (o) => (on ? { ...o, allow: o.allow & ~VIEW, deny: o.deny | VIEW } : { ...o, deny: o.deny & ~VIEW }));

  const losing = useLosingAccess(instanceKey, serverId, channel, channels, draft);

  // The permissions you may change here: the ones you have in this channel, unless you own the server or are an administrator.
  const have = inChannel(access, channel.id);
  const may = (p: Permission) => mayChange(access, bit(p), have);

  const targets = useMemo(() => {
    const roleOrder = new Map(roles.map((r, n) => [r.id, n]));
    return [...draft]
      .filter((o) => o.targetId !== serverId)
      .sort((a, b) => Number(a.member) - Number(b.member) || (roleOrder.get(a.targetId) ?? 0) - (roleOrder.get(b.targetId) ?? 0));
  }, [draft, roles, serverId]);
  const roleOf = (id: string) => roles.find((r) => r.id === id);
  const memberOf = (id: string) => members.find((m) => m.user?.id === id);
  const synced = !!parent && key(fromWire(parent.permissionOverwrites)) === key(draft);
  // A target that's no longer in the draft (removed, or replaced by a save elsewhere) falls back to @everyone.
  const selected = picked && find(picked) ? picked : serverId;
  const selectedOverwrite = find(selected) ?? (selected === serverId ? everyone : null);

  const add = (targetId: string, member: boolean, allowView: boolean) => {
    put(targetId, member, (o) => (allowView ? { ...o, allow: o.allow | VIEW, deny: o.deny & ~VIEW } : o));
    setPicked(targetId);
  };
  const removeTarget = (targetId: string) => setDraft((list) => list.filter((o) => o.targetId !== targetId));

  async function submit() {
    const done = await save.go(instanceKey, serverId, channel.id, toWire(draft));
    if (done) setDraft(fromWire(done.permissionOverwrites));
  }

  const badge = (o: Overwrite) => <TargetBadge overwrite={o} role={roleOf(o.targetId)} member={memberOf(o.targetId)} />;

  return (
    <div data-setting="channel-permissions" className="flex flex-col gap-5">
      {parent && <SyncBanner parent={parent} synced={synced} onMatch={() => setDraft(fromWire(parent.permissionOverwrites))} />}

      <PrivateSection
        isPrivate={isPrivate}
        category={category}
        canChange={may(Permission.VIEW_CHANNELS)}
        onPrivate={setPrivate}
        viewers={targets.filter((o) => o.allow & VIEW)}
        badge={badge}
        onHide={(o) => put(o.targetId, o.member, (x) => ({ ...x, allow: x.allow & ~VIEW }))}
        roles={roles.filter((r) => r.id !== serverId && !((find(r.id)?.allow ?? 0) & VIEW))}
        members={members.filter((m) => !((find(m.user?.id ?? "")?.allow ?? 0) & VIEW))}
        onAdd={(id, member) => add(id, member, true)}
      />

      <LosingWarning losing={losing} category={category} />

      <section className="flex flex-col gap-3">
        <AdvancedHeading
          roles={roles.filter((r) => r.id !== serverId && !find(r.id))}
          members={members.filter((m) => !find(m.user?.id ?? ""))}
          onAdd={(id, member) => add(id, member, false)}
        />
        <div className="grid gap-3 md:grid-cols-[minmax(0,12rem)_minmax(0,1fr)]">
          <TargetList overwrites={[everyone, ...targets]} everyoneId={serverId} selected={selected} onSelect={setPicked} badge={badge} />
          <AnimatePresence mode="wait" initial={false}>
            {selectedOverwrite && (
              <OverwriteEditor
                key={selected}
                overwrite={selectedOverwrite}
                channelType={channel.type}
                may={may}
                put={put}
                onRemove={selected !== serverId ? () => removeTarget(selected) : undefined}
              />
            )}
          </AnimatePresence>
        </div>
      </section>
      <SaveBar count={dirty ? Math.max(1, changes) : 0} saving={save.pending} error={save.error} onSave={() => void submit()} onDiscard={() => setDraft(base)} />
    </div>
  );
}

/** How many targets' overwrites differ between two lists. */
function countChanges(base: Overwrite[], draft: Overwrite[]) {
  const before = new Map(base.map((o) => [o.targetId, o]));
  const after = new Map(draft.map((o) => [o.targetId, o]));
  let n = 0;
  for (const id of new Set([...before.keys(), ...after.keys()])) {
    const a = before.get(id);
    const b = after.get(id);
    if ((a?.allow ?? 0) !== (b?.allow ?? 0) || (a?.deny ?? 0) !== (b?.deny ?? 0)) n++;
  }
  return n;
}

/** What saving would do to you: losing the channel is worth a warning. */
function useLosingAccess(instanceKey: string, serverId: string, channel: Channel, channels: Channel[], draft: Overwrite[]) {
  const inst = useInstance(instanceKey);
  const roles = useRoles(instanceKey, serverId);
  const server = inst?.servers.find((s) => s.id === serverId);
  const me = inst?.members[serverId]?.find((m) => m.user?.id === inst?.me?.id);
  const you = useMemo(() => {
    if (!server || !inst?.me) return null;
    const after = { ...channel, permissionOverwrites: toWire(draft) } as Channel;
    const list = channels.map((c) => (c.id === channel.id ? after : c));
    return accessOf(serverId, server.ownerId, roles, list, inst.me.id, me?.roleIds ?? []);
  }, [server, inst?.me, channel, draft, channels, serverId, roles, me?.roleIds]);
  return !!you && !canSee(you, channel.id);
}

/** Whether this channel follows its category's permissions, with a button to match them. */
function SyncBanner({ parent, synced, onMatch }: { parent: Channel; synced: boolean; onMatch: () => void }) {
  const { t } = useI18n();
  return (
    <motion.div layout className={cn("flex flex-wrap items-center gap-2 rounded-xl px-3 py-2 text-sm", synced ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "bg-muted/60")}>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span key={String(synced)} initial={{ scale: 0, rotate: -90 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={SPRING}>
          {synced ? <LinkIcon className="size-4" /> : <UnlinkIcon className="size-4 text-muted-foreground" />}
        </motion.span>
      </AnimatePresence>
      <span className="min-w-0 flex-1">
        <T k={synced ? "serversettings.channelPermissions.synced" : "serversettings.channelPermissions.apart"} values={{ category: <b>{parent.name}</b> }} />
      </span>
      {!synced && (
        <Button type="button" size="sm" variant="ghost" className="h-7 rounded-lg" onClick={onMatch}>
          {t("serversettings.channelPermissions.match")}
        </Button>
      )}
    </motion.div>
  );
}

/** The private switch, and who can see the channel while it's private. */
function PrivateSection({
  isPrivate,
  category,
  canChange,
  onPrivate,
  viewers,
  badge,
  onHide,
  roles,
  members,
  onAdd,
}: {
  isPrivate: boolean;
  category: boolean;
  canChange: boolean;
  onPrivate: (on: boolean) => void;
  viewers: Overwrite[];
  badge: (o: Overwrite) => ReactNode;
  onHide: (o: Overwrite) => void;
  /** Who could be let in. */
  roles: Role[];
  members: Member[];
  onAdd: (id: string, member: boolean) => void;
}) {
  const { t } = useI18n();
  return (
    <section className="rounded-2xl border bg-background/40 p-4">
      <label className="flex cursor-pointer items-center gap-3">
        <motion.span
          animate={isPrivate ? { rotate: [0, -12, 10, 0], scale: [1, 1.15, 1] } : { rotate: 0, scale: 1 }}
          transition={{ duration: 0.5 }}
          className={cn("grid size-10 shrink-0 place-items-center rounded-xl transition-colors", isPrivate ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground")}
        >
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span key={String(isPrivate)} initial={{ y: 10, opacity: 0 }} animate={{ y: 0, opacity: 1 }} exit={{ y: -10, opacity: 0 }} transition={SPRING}>
              {isPrivate ? <LockIcon className="size-5" /> : <LockOpenIcon className="size-5" />}
            </motion.span>
          </AnimatePresence>
        </motion.span>
        <span className="min-w-0 flex-1">
          <span className="block font-extrabold">
            {category ? t("serversettings.channelPermissions.privateCategory") : t("serversettings.channelPermissions.privateChannel")}
          </span>
          <span className="block text-sm text-muted-foreground">
            {category ? t("serversettings.channelPermissions.privateCategoryHint") : t("serversettings.channelPermissions.privateChannelHint")}
          </span>
        </span>
        <Switch checked={isPrivate} disabled={!canChange} onCheckedChange={onPrivate} aria-label={t("serversettings.channels.private")} />
      </label>
      <AnimatePresence initial={false}>
        {isPrivate && (
          <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={{ duration: 0.25, ease: [0.22, 1, 0.36, 1] }} className="overflow-hidden">
            <div className="mt-4 flex flex-col gap-2 border-t pt-4">
              <p className="text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t("serversettings.channelPermissions.whoCanSee")}</p>
              <ul className="flex flex-wrap gap-1.5">
                <AnimatePresence initial={false} mode="popLayout">
                  {viewers.map((o) => (
                    <motion.li
                      key={o.targetId}
                      layout
                      initial={{ opacity: 0, scale: 0.6 }}
                      animate={{ opacity: 1, scale: 1 }}
                      exit={{ opacity: 0, scale: 0.6 }}
                      transition={SPRING}
                      className="flex h-8 items-center gap-1.5 rounded-full border bg-background pr-1 pl-2 text-sm font-bold"
                    >
                      {badge(o)}
                      <button
                        type="button"
                        aria-label={t("serversettings.channelPermissions.remove")}
                        disabled={!canChange}
                        onClick={() => onHide(o)}
                        className="grid size-6 place-items-center rounded-full text-muted-foreground transition hover:rotate-90 hover:bg-destructive/10 hover:text-destructive"
                      >
                        <XIcon className="size-3.5" />
                      </button>
                    </motion.li>
                  ))}
                  <motion.li key="add" layout transition={SPRING}>
                    <AddTarget
                      roles={roles}
                      members={members}
                      onAdd={onAdd}
                      label={t("serversettings.channelPermissions.addViewer")}
                      compact
                    />
                  </motion.li>
                </AnimatePresence>
              </ul>
              {!viewers.length && <p className="text-sm text-muted-foreground">{t("serversettings.channelPermissions.nobodyYet")}</p>}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </section>
  );
}

/** Says so when saving would lock you out of the channel. */
function LosingWarning({ losing, category }: { losing: boolean; category: boolean }) {
  const { t } = useI18n();
  return (
    <AnimatePresence>
      {losing && (
        <motion.p
          initial={{ opacity: 0, y: -6 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -6 }}
          className="flex items-center gap-2 rounded-xl border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm text-amber-700 dark:text-amber-300"
        >
          <TriangleAlertIcon className="size-4 shrink-0" />{" "}
          {category ? t("serversettings.channelPermissions.losingCategory") : t("serversettings.channelPermissions.losingChannel")}
        </motion.p>
      )}
    </AnimatePresence>
  );
}

function AdvancedHeading({ roles, members, onAdd }: { roles: Role[]; members: Member[]; onAdd: (id: string, member: boolean) => void }) {
  const { t } = useI18n();
  return (
  <div className="flex items-center justify-between gap-2">
    <div>
      <h4 className="font-extrabold">{t("serversettings.channelPermissions.advanced")}</h4>
      <p className="text-sm text-muted-foreground">{t("serversettings.channelPermissions.advancedHint")}</p>
    </div>
    <AddTarget
      roles={roles}
      members={members}
      onAdd={onAdd}
      label={t("serversettings.channelPermissions.addTarget")}
    />
  </div>
  );
}

/** @everyone and every role or person with permissions here, to pick one to edit. */
function TargetList({
  overwrites,
  everyoneId,
  selected,
  onSelect,
  badge,
}: {
  overwrites: Overwrite[];
  everyoneId: string;
  selected: string;
  onSelect: (targetId: string) => void;
  badge: (o: Overwrite) => ReactNode;
}) {
  return (
  <ul className="flex flex-row gap-1 overflow-x-auto md:flex-col md:overflow-visible">
    {overwrites.map((o) => {
      const active = selected === o.targetId;
      const count = toList(o.allow).length + toList(o.deny).length;
      return (
        <li key={o.targetId} className="shrink-0">
          <button
            type="button"
            onClick={() => onSelect(o.targetId)}
            className={cn(
              "relative flex h-9 w-full items-center gap-2 rounded-lg px-2 text-left text-sm transition-colors",
              active ? "font-bold" : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
            )}
          >
            {active && <motion.span layoutId="overwrite-target" transition={SPRING} className="absolute inset-0 rounded-lg bg-primary/12" />}
            <span className="relative flex min-w-0 flex-1 items-center gap-2">
              {o.targetId === everyoneId ? (
                <>
                  <UsersIcon className="size-4 shrink-0" />
                  <span className="truncate">@everyone</span>
                </>
              ) : (
                badge(o)
              )}
            </span>
            {count > 0 && <span className="relative rounded-full bg-muted px-1.5 text-[0.65rem] font-bold tabular-nums">{count}</span>}
          </button>
        </li>
      );
    })}
  </ul>
  );
}

/** Every channel permission for one role or person: allowed, denied, or left to their roles. */
function OverwriteEditor({
  overwrite,
  channelType,
  may,
  put,
  onRemove,
}: {
  overwrite: Overwrite;
  channelType: ChannelType;
  may: (p: Permission) => boolean;
  put: (targetId: string, member: boolean, fn: (o: Overwrite) => Overwrite) => void;
  /** Takes the target off this channel; not for @everyone. */
  onRemove?: () => void;
}) {
  const { t } = useI18n();
  const category = channelType === ChannelType.CATEGORY;
  return (
    <motion.div
      initial={{ opacity: 0, x: 12 }}
      animate={{ opacity: 1, x: 0 }}
      exit={{ opacity: 0, x: -12 }}
      transition={SPRING}
      className="min-w-0 rounded-2xl border bg-background/40 p-3"
    >
      {CHANNEL_GROUPS.filter((g) => category || (g.kind === "voice") === (channelType === ChannelType.VOICE) || g.kind === "general").map((group) => (
        <div key={group.kind} className="mb-2 last:mb-0">
          <p className="mb-1 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t(group.title)}</p>
          <ul>
            {group.permissions.map((p) => {
              const info = permissionInfo(p);
              const state: State = overwrite.allow & bit(p) ? "allow" : overwrite.deny & bit(p) ? "deny" : "inherit";
              return (
                <li key={p} className="flex items-center gap-3 border-b border-border/50 py-2 last:border-b-0">
                  <div className="min-w-0 flex-1">
                    <p className="text-sm font-bold">{t(info.label)}</p>
                    <p className="text-xs text-muted-foreground">{t(info.channel ?? info.about)}</p>
                  </div>
                  <TriState
                    value={state}
                    disabled={!may(p)}
                    label={t(info.label)}
                    onChange={(next) =>
                      put(overwrite.targetId, overwrite.member, (o) => ({
                        ...o,
                        allow: next === "allow" ? o.allow | bit(p) : o.allow & ~bit(p),
                        deny: next === "deny" ? o.deny | bit(p) : o.deny & ~bit(p),
                      }))
                    }
                  />
                </li>
              );
            })}
          </ul>
        </div>
      ))}
      {onRemove && (
        <Button
          type="button"
          variant="ghost"
          size="sm"
          onClick={onRemove}
          className="group mt-1 rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive"
        >
          <Trash2Icon className="transition-transform group-hover:-rotate-12" />{" "}
          {category ? t("serversettings.channelPermissions.removeFromCategory") : t("serversettings.channelPermissions.removeFromChannel")}
        </Button>
      )}
    </motion.div>
  );
}

function TargetBadge({ overwrite, role, member }: { overwrite: Overwrite; role?: Role; member?: Member }) {
  const { t } = useI18n();
  if (overwrite.member)
    return (
      <>
        {member ? <UserAvatar user={member.user} className="size-5" /> : <UserIcon className="size-4" />}
        <span className="truncate">{member ? memberName(member) : t("serversettings.channelPermissions.someoneLeft")}</span>
      </>
    );
  return (
    <>
      <RoleDot role={role ?? {}} />
      <span className="truncate">{role?.name ?? t("serversettings.channelPermissions.deletedRole")}</span>
    </>
  );
}

function AddTarget({
  roles,
  members,
  onAdd,
  label,
  compact = false,
}: {
  roles: Role[];
  members: Member[];
  onAdd: (id: string, member: boolean) => void;
  label: string;
  compact?: boolean;
}) {
  const { t } = useI18n();
  if (!roles.length && !members.length) return null;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        {compact ? (
          <button
            type="button"
            aria-label={label}
            className="grid size-8 place-items-center rounded-full border border-dashed text-muted-foreground transition hover:rotate-90 hover:border-primary/50 hover:text-primary data-[state=open]:rotate-45 data-[state=open]:text-primary"
          >
            <PlusIcon className="size-4" />
          </button>
        ) : (
          <Button type="button" size="sm" variant="outline" className="shrink-0 rounded-xl font-bold">
            <PlusIcon /> {t("serversettings.channelPermissions.add")}
          </Button>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="max-h-80 w-60 overflow-y-auto">
        {roles.length > 0 && <DropdownMenuLabel className="text-xs text-muted-foreground">{t("serversettings.nav.roles")}</DropdownMenuLabel>}
        {roles.map((r) => (
          <DropdownMenuItem key={r.id} onSelect={() => onAdd(r.id, false)}>
            <RoleDot role={r} /> <span className="truncate">{r.name}</span>
          </DropdownMenuItem>
        ))}
        {roles.length > 0 && members.length > 0 && <DropdownMenuSeparator />}
        {members.length > 0 && <DropdownMenuLabel className="text-xs text-muted-foreground">{t("serversettings.nav.people")}</DropdownMenuLabel>}
        {members.slice(0, 50).map((m) => (
          <DropdownMenuItem key={m.user?.id} onSelect={() => onAdd(m.user?.id ?? "", true)}>
            <UserAvatar user={m.user} className="size-5" /> <span className="truncate">{memberName(m)}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Deny, follow their roles, or allow, with the choice sliding between the three. */
function TriState({ value, onChange, disabled, label }: { value: State; onChange: (v: State) => void; disabled: boolean; label: string }) {
  const { t } = useI18n();
  const group = useId();
  const options: { value: State; icon: typeof CheckIcon; name: string; tint: string }[] = [
    { value: "deny", icon: XIcon, name: t("serversettings.channelPermissions.deny"), tint: "bg-destructive text-white" },
    { value: "inherit", icon: SlashIcon, name: t("serversettings.channelPermissions.inherit"), tint: "bg-muted-foreground/25 text-foreground" },
    { value: "allow", icon: CheckIcon, name: t("serversettings.channelPermissions.allow"), tint: "bg-emerald-500 text-white" },
  ];
  return (
    <div role="radiogroup" aria-label={label} className={cn("flex shrink-0 rounded-lg border bg-muted/40 p-0.5", disabled && "opacity-50")}>
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            aria-label={o.name}
            title={disabled ? t("serversettings.channelPermissions.notYours") : o.name}
            disabled={disabled}
            onClick={() => onChange(o.value)}
            className={cn("relative grid size-7 place-items-center rounded-md transition-colors", !on && "text-muted-foreground enabled:hover:text-foreground")}
          >
            {on && <motion.span layoutId={`tri-${group}`} transition={SPRING} className={cn("absolute inset-0 rounded-md", o.tint)} />}
            <motion.span className="relative" animate={on ? { scale: [0.6, 1.15, 1] } : { scale: 1 }} transition={{ duration: 0.3 }}>
              <o.icon className={cn("size-3.5", on && o.value !== "inherit" && "text-white")} strokeWidth={3} />
            </motion.span>
          </button>
        );
      })}
    </div>
  );
}
