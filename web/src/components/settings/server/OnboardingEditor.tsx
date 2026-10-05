import { create, equals } from "@bufbuild/protobuf";
import {
  ChevronDownIcon,
  GripVerticalIcon,
  HashIcon,
  ListChecksIcon,
  MessageCircleHeartIcon,
  PlusIcon,
  ScrollTextIcon,
  ShieldIcon,
  SmilePlusIcon,
  Trash2Icon,
  XIcon,
  type LucideIcon,
} from "lucide-react";
import { AnimatePresence, m as motion, Reorder, useDragControls } from "motion/react";
import { useState } from "react";
import {
  ChannelType,
  OnboardingOptionSchema,
  OnboardingSchema,
  OnboardingStepKind,
  OnboardingStepSchema,
  Permission,
  type Channel,
  type Emoji,
  type Onboarding,
  type OnboardingOption,
  type OnboardingStep,
  type Role,
  type Server,
} from "@/gen/fuwa/v1/types_pb";
import { useAccess, useInstance, useRoles } from "@/fuwa/hooks";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { EmojiPicker } from "@/components/EmojiPicker";
import { SPRING } from "@/lib/motion";
import { Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { type I18n, type Key, useI18n } from "@/i18n/react";
import { above, bit, cssColor, fromList } from "@/lib/permissions";
import { cn } from "@/lib/utils";

/** As on the server. */
const MAX_STEPS = 6;
const MAX_OPTIONS = 12;
const MAX_PICKS = 5;

/** What a role handed out by onboarding may never carry, as the server checks it. */
const POWERS = [
  Permission.ADMINISTRATOR,
  Permission.MANAGE_SERVER,
  Permission.MANAGE_ROLES,
  Permission.MANAGE_CHANNELS,
  Permission.MANAGE_EMOJI,
  Permission.MANAGE_WEBHOOKS,
  Permission.MANAGE_MESSAGES,
  Permission.MENTION_EVERYONE,
  Permission.VIEW_AUDIT_LOG,
  Permission.KICK_MEMBERS,
  Permission.BAN_MEMBERS,
  Permission.TIME_OUT_MEMBERS,
  Permission.MANAGE_NICKNAMES,
  Permission.MUTE_MEMBERS,
  Permission.MOVE_MEMBERS,
  Permission.RECORD,
].reduce((bits, p) => bits | bit(p), 0);

/** Whether anyone may get the role by picking it. */
export const harmless = (role: Role) => (fromList(role.permissions) & POWERS) === 0;

/** A step being edited, with a key that survives reordering. */
type Draft = { key: string; step: OnboardingStep };

/** The onboarding as it's being edited. */
export type OnboardingDraft = { enabled: boolean; steps: Draft[] };

let made = 0;
const fresh = () => `new-${++made}`;

export const onboardingDraft = (o: Onboarding): OnboardingDraft => ({
  enabled: o.enabled,
  steps: o.steps.map((step) => ({ key: step.id || fresh(), step })),
});

/** The draft as the server takes it. */
export const onboardingOf = (d: OnboardingDraft): Onboarding =>
  create(OnboardingSchema, {
    enabled: d.enabled,
    steps: d.steps.map(({ step }) =>
      create(OnboardingStepSchema, {
        ...step,
        title: step.title.trim(),
        description: step.description.trim(),
        hello: step.hello.trim(),
        options: step.options.map((o) => create(OnboardingOptionSchema, { ...o, label: o.label.trim(), description: o.description.trim() })),
      }),
    ),
  });

/** 1 when the onboarding differs from what's saved. */
export const onboardingChanges = (d: OnboardingDraft, saved: Onboarding) => (equals(OnboardingSchema, onboardingOf(d), saved) ? 0 : 1);

/** Each kind of step: its name and what it does (catalog keys), and its icon. */
const KINDS: Record<Exclude<OnboardingStepKind, OnboardingStepKind.UNSPECIFIED>, { label: Key; hint: Key; icon: LucideIcon }> = {
  [OnboardingStepKind.PICK]: { label: "serversettings.onboarding.kindPick", hint: "serversettings.onboarding.kindPickHint", icon: ListChecksIcon },
  [OnboardingStepKind.RULES]: { label: "serversettings.onboarding.kindRules", hint: "serversettings.onboarding.kindRulesHint", icon: ScrollTextIcon },
  [OnboardingStepKind.HELLO]: { label: "serversettings.onboarding.kindHello", hint: "serversettings.onboarding.kindHelloHint", icon: MessageCircleHeartIcon },
};

/** A new step of a kind, its title and hello in the app's language. */
function newStep(t: I18n["t"], kind: OnboardingStepKind, channels: Channel[]): OnboardingStep {
  const text = channels.find((c) => c.type === ChannelType.TEXT);
  return create(OnboardingStepSchema, {
    kind,
    title: t(
      kind === OnboardingStepKind.PICK
        ? "serversettings.onboarding.titlePick"
        : kind === OnboardingStepKind.RULES
          ? "serversettings.onboarding.titleRules"
          : "serversettings.onboarding.kindHello",
    ),
    skippable: kind === OnboardingStepKind.HELLO,
    multiple: true,
    channelId: kind === OnboardingStepKind.HELLO ? (text?.id ?? "") : "",
    hello: kind === OnboardingStepKind.HELLO ? t("join.onboarding.hello") : "",
    options: kind === OnboardingStepKind.PICK ? [create(OnboardingOptionSchema, { label: "", emoji: "✨" })] : [],
  });
}

/**
 * Onboarding: steps new members go through after joining, in order. Each
 * step is a card that folds open; picks are a list of choices with the roles
 * they hand out and the channels they suggest.
 */
export function OnboardingFields({
  instanceKey,
  server,
  draft,
  onChange,
  onFocusStep,
}: {
  instanceKey: string;
  server: Server;
  draft: OnboardingDraft;
  onChange: (draft: OnboardingDraft) => void;
  /** The step being edited, for the preview to show. */
  onFocusStep?: (index: number) => void;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const channels = (inst?.channels[server.id] ?? []).filter((c) => c.type !== ChannelType.CATEGORY);
  const emojis = inst?.emojis[server.id];
  const access = useAccess(instanceKey, server.id);
  const roles = useRoles(instanceKey, server.id).filter((r) => r.id !== server.id);
  const [open, setOpen] = useState<string | null>(draft.steps[0]?.key ?? null);
  const setSteps = (steps: Draft[]) => onChange({ ...draft, steps });
  const patch = (key: string, p: Partial<OnboardingStep>) => setSteps(draft.steps.map((d) => (d.key === key ? { ...d, step: { ...d.step, ...p } } : d)));
  const hasRules = draft.steps.some((d) => d.step.kind === OnboardingStepKind.RULES);

  function add(kind: OnboardingStepKind) {
    const key = fresh();
    setSteps([...draft.steps, { key, step: newStep(t, kind, channels) }]);
    setOpen(key);
    onFocusStep?.(draft.steps.length);
  }

  return (
    <div className="flex flex-col">
      <div data-setting="onboarding-enabled" className="border-b border-border/70 pb-5">
        <Toggle
          checked={draft.enabled}
          onChange={(on) => onChange({ ...draft, enabled: on })}
          label={t("serversettings.onboarding.enabled")}
          hint={t("serversettings.onboarding.enabledHint")}
        />
      </div>
      <div data-setting="onboarding-steps" className="flex flex-col gap-3 py-5">
        <span>
          <span className="block font-extrabold">{t("serversettings.onboarding.steps")}</span>
          <span className="block text-sm text-muted-foreground">{t("serversettings.onboarding.stepsHint", { max: MAX_STEPS })}</span>
        </span>
        <Reorder.Group axis="y" values={draft.steps} onReorder={setSteps} className="flex flex-col gap-2">
          <AnimatePresence initial={false}>
            {draft.steps.map((d, n) => (
              <StepCard
                key={d.key}
                draft={d}
                index={n}
                open={open === d.key}
                onToggle={() => {
                  setOpen(open === d.key ? null : d.key);
                  onFocusStep?.(n);
                }}
                onChange={(p) => patch(d.key, p)}
                onRemove={() => setSteps(draft.steps.filter((x) => x.key !== d.key))}
                channels={channels}
                roles={roles}
                emojis={emojis}
                server={server}
                canHandOut={(r) => above(access, r.position) && harmless(r)}
              />
            ))}
          </AnimatePresence>
        </Reorder.Group>
        {draft.steps.length < MAX_STEPS && (
          <motion.div layout transition={SPRING} className="flex flex-wrap gap-2">
            {[OnboardingStepKind.PICK, OnboardingStepKind.RULES, OnboardingStepKind.HELLO]
              .filter((k) => k !== OnboardingStepKind.RULES || (!hasRules && server.hasRules))
              .map((k) => {
                const { label, icon: Icon } = KINDS[k as keyof typeof KINDS];
                return (
                  <Button key={k} type="button" variant="outline" className="group rounded-xl border-dashed" onClick={() => add(k)}>
                    <PlusIcon className="transition-transform group-hover:rotate-90" />
                    <Icon className="text-muted-foreground" /> {t(label)}
                  </Button>
                );
              })}
          </motion.div>
        )}
      </div>
    </div>
  );
}

function StepCard({
  draft,
  index,
  open,
  onToggle,
  onChange,
  onRemove,
  channels,
  roles,
  emojis,
  server,
  canHandOut,
}: {
  draft: Draft;
  index: number;
  open: boolean;
  onToggle: () => void;
  onChange: (p: Partial<OnboardingStep>) => void;
  onRemove: () => void;
  channels: Channel[];
  roles: Role[];
  emojis: Emoji[] | undefined;
  server: Server;
  canHandOut: (role: Role) => boolean;
}) {
  const { t } = useI18n();
  const drag = useDragControls();
  const { step } = draft;
  const kind = KINDS[step.kind as keyof typeof KINDS] ?? KINDS[OnboardingStepKind.PICK];
  const Icon = kind.icon;
  const setOption = (n: number, p: Partial<OnboardingOption>) => onChange({ options: step.options.map((o, i) => (i === n ? { ...o, ...p } : o)) });
  return (
    <Reorder.Item
      value={draft}
      dragListener={false}
      dragControls={drag}
      initial={{ opacity: 0, y: -8, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, x: 24, scale: 0.95 }}
      transition={SPRING}
      whileDrag={{ scale: 1.02, boxShadow: "0 12px 30px -10px rgb(0 0 0 / 0.35)" }}
      className={cn("overflow-hidden rounded-2xl border bg-background/70 transition-colors", open && "border-primary/40")}
    >
      <div className="flex items-center gap-2 p-2">
        <button
          type="button"
          aria-label={t("serversettings.shared.dragToReorder")}
          onPointerDown={(e) => drag.start(e)}
          className="grid h-9 w-5 shrink-0 cursor-grab touch-none place-items-center text-muted-foreground active:cursor-grabbing"
        >
          <GripVerticalIcon className="size-4" />
        </button>
        <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary/10 text-primary">
          <Icon className="size-4" />
        </span>
        <button type="button" onClick={onToggle} aria-expanded={open} className="group flex min-w-0 flex-1 items-center gap-2 text-left">
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-bold">
              {index + 1}. {step.title || t(kind.label)}
            </span>
            <span className="block truncate text-xs text-muted-foreground">
              {step.kind === OnboardingStepKind.PICK ? t("serversettings.onboarding.choices", { count: step.options.length }) : t(kind.label)}
              {step.skippable && step.kind !== OnboardingStepKind.RULES ? ` · ${t("serversettings.onboarding.skippableTag")}` : ""}
            </span>
          </span>
          <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
        </button>
        <Button type="button" variant="ghost" size="icon" aria-label={t("serversettings.onboarding.removeStep")} onClick={onRemove} className="size-9 shrink-0 rounded-full text-muted-foreground hover:text-destructive">
          <Trash2Icon className="size-4" />
        </Button>
      </div>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div initial={{ opacity: 0, y: -8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={SPRING}>
            <div className="flex flex-col gap-3 border-t p-3">
              <p className="text-xs text-muted-foreground">{t(kind.hint)}</p>
              <Input value={step.title} maxLength={80} placeholder={t("serversettings.onboarding.title")} aria-label={t("serversettings.onboarding.titleLabel")} onChange={(e) => onChange({ title: e.target.value })} className="h-10 rounded-xl font-bold" />
              <Input
                value={step.description}
                maxLength={200}
                placeholder={t("serversettings.onboarding.description")}
                aria-label={t("serversettings.onboarding.descriptionLabel")}
                onChange={(e) => onChange({ description: e.target.value })}
                className="h-10 rounded-xl"
              />
              {step.kind !== OnboardingStepKind.RULES && (
                <Toggle checked={step.skippable} onChange={(on) => onChange({ skippable: on })} label={t("serversettings.onboarding.skippable")} hint={t("serversettings.onboarding.skippableHint")} />
              )}
              {step.kind === OnboardingStepKind.PICK && (
                <>
                  <Toggle checked={step.multiple} onChange={(on) => onChange({ multiple: on })} label={t("serversettings.onboarding.multiple")} hint={t("serversettings.onboarding.multipleHint")} />
                  <div className="flex flex-col gap-2">
                    <AnimatePresence initial={false}>
                      {step.options.map((o, n) => (
                        <OptionRow
                          key={o.id || `o${n}`}
                          option={o}
                          channels={channels}
                          roles={roles}
                          emojis={emojis}
                          server={server}
                          canHandOut={canHandOut}
                          onChange={(p) => setOption(n, p)}
                          onRemove={step.options.length > 1 ? () => onChange({ options: step.options.filter((_, i) => i !== n) }) : undefined}
                        />
                      ))}
                    </AnimatePresence>
                  </div>
                  {step.options.length < MAX_OPTIONS && (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      className="group w-fit rounded-xl border-dashed"
                      onClick={() => onChange({ options: [...step.options, create(OnboardingOptionSchema, { label: "", emoji: "" })] })}
                    >
                      <PlusIcon className="transition-transform group-hover:rotate-90" /> {t("serversettings.onboarding.addChoice")}
                    </Button>
                  )}
                </>
              )}
              {step.kind === OnboardingStepKind.HELLO && (
                <>
                  <ChannelPick channels={channels.filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT)} value={step.channelId} onChange={(channelId) => onChange({ channelId })} />
                  <Input value={step.hello} maxLength={200} placeholder={t("join.onboarding.hello")} aria-label={t("serversettings.onboarding.helloLabel")} onChange={(e) => onChange({ hello: e.target.value })} className="h-10 rounded-xl" />
                </>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </Reorder.Item>
  );
}

function OptionRow({
  option,
  channels,
  roles,
  emojis,
  server,
  canHandOut,
  onChange,
  onRemove,
}: {
  option: OnboardingOption;
  channels: Channel[];
  roles: Role[];
  emojis: Emoji[] | undefined;
  server: Server;
  canHandOut: (role: Role) => boolean;
  onChange: (p: Partial<OnboardingOption>) => void;
  onRemove?: () => void;
}) {
  const { t } = useI18n();
  const flip = (list: string[], id: string) => (list.includes(id) ? list.filter((x) => x !== id) : [...list, id].slice(0, MAX_PICKS));
  const given = roles.filter((r) => option.roleIds.includes(r.id));
  const suggested = channels.filter((c) => option.channelIds.includes(c.id));
  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, x: 20 }}
      transition={SPRING}
      className="flex flex-col gap-2 rounded-xl border bg-card/60 p-2"
    >
      <div className="flex items-center gap-2">
        <EmojiPicker emojis={emojis} server={server} placement="bottom-start" onPick={(e) => onChange({ emoji: e.text })}>
          {(open) => (
            <motion.button
              type="button"
              whileHover={{ scale: 1.08, rotate: -6 }}
              whileTap={{ scale: 0.9 }}
              aria-label={option.emoji ? t("serversettings.shared.changeEmoji") : t("serversettings.shared.pickEmoji")}
              className={cn("grid size-9 shrink-0 place-items-center rounded-xl border text-xl transition-colors", open ? "border-primary/60 bg-primary/10" : "hover:border-primary/40")}
            >
              {option.emoji ? <EmojiGlyph value={option.emoji} emojis={emojis} className="size-6" /> : <SmilePlusIcon className="size-4 text-muted-foreground" />}
            </motion.button>
          )}
        </EmojiPicker>
        <Input value={option.label} maxLength={50} placeholder={t("serversettings.onboarding.choicePlaceholder")} aria-label={t("serversettings.onboarding.choiceLabel")} onChange={(e) => onChange({ label: e.target.value })} className="h-9 min-w-0 flex-1 rounded-xl font-bold" />
        {onRemove && (
          <Button type="button" variant="ghost" size="icon" aria-label={t("serversettings.onboarding.removeChoice")} onClick={onRemove} className="size-9 shrink-0 rounded-full text-muted-foreground hover:text-destructive">
            <XIcon />
          </Button>
        )}
      </div>
      <Input value={option.description} maxLength={100} placeholder={t("serversettings.onboarding.choiceAbout")} aria-label={t("serversettings.onboarding.choiceAboutLabel")} onChange={(e) => onChange({ description: e.target.value })} className="h-9 rounded-xl" />
      <div className="flex flex-wrap items-center gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {given.map((r) => (
            <Chip key={r.id} onRemove={() => onChange({ roleIds: flip(option.roleIds, r.id) })}>
              <span className="size-2 rounded-full" style={{ background: r.color ? cssColor(r.color) : "var(--muted-foreground)" }} /> @{r.name}
            </Chip>
          ))}
          {suggested.map((c) => (
            <Chip key={c.id} onRemove={() => onChange({ channelIds: flip(option.channelIds, c.id) })}>
              <HashIcon className="size-3 text-muted-foreground" /> {c.name}
            </Chip>
          ))}
        </AnimatePresence>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" size="sm" className="h-7 rounded-lg border-dashed text-xs" disabled={!roles.length}>
              <ShieldIcon /> {t("serversettings.nav.roles")}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-72 w-64 overflow-y-auto">
            {roles.map((r) => {
              const ok = canHandOut(r);
              return (
                <DropdownMenuCheckboxItem
                  key={r.id}
                  disabled={!ok && !option.roleIds.includes(r.id)}
                  checked={option.roleIds.includes(r.id)}
                  onSelect={(e) => e.preventDefault()}
                  onCheckedChange={() => onChange({ roleIds: flip(option.roleIds, r.id) })}
                >
                  <span className="size-2 rounded-full" style={{ background: r.color ? cssColor(r.color) : "var(--muted-foreground)" }} />
                  <span className="min-w-0 flex-1 truncate">{r.name}</span>
                  {!ok && <span className="text-[0.65rem] text-muted-foreground">{harmless(r) ? t("serversettings.onboarding.aboveYou") : t("serversettings.onboarding.moderates")}</span>}
                </DropdownMenuCheckboxItem>
              );
            })}
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" size="sm" className="h-7 rounded-lg border-dashed text-xs">
              <HashIcon /> {t("serversettings.nav.channels")}
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-72 w-56 overflow-y-auto">
            {channels.map((c) => (
              <DropdownMenuCheckboxItem key={c.id} checked={option.channelIds.includes(c.id)} onSelect={(e) => e.preventDefault()} onCheckedChange={() => onChange({ channelIds: flip(option.channelIds, c.id) })}>
                <HashIcon /> {c.name}
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </motion.div>
  );
}

function Chip({ children, onRemove }: { children: React.ReactNode; onRemove: () => void }) {
  const { t } = useI18n();
  return (
    <motion.span layout initial={{ opacity: 0, scale: 0.6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.6 }} transition={SPRING} className="flex h-7 items-center gap-1 rounded-lg bg-muted pr-0.5 pl-2 text-xs font-bold">
      {children}
      <button type="button" aria-label={t("serversettings.channelPermissions.remove")} onClick={onRemove} className="grid size-6 place-items-center rounded-md text-muted-foreground transition hover:bg-background hover:text-foreground">
        <XIcon className="size-3" />
      </button>
    </motion.span>
  );
}

function ChannelPick({ channels, value, onChange }: { channels: Channel[]; value: string; onChange: (id: string) => void }) {
  const { t } = useI18n();
  const channel = channels.find((c) => c.id === value);
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className="group flex h-10 items-center gap-1.5 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60">
          <HashIcon className="size-4 shrink-0 text-muted-foreground" />
          <span className="flex-1 truncate font-bold">{channel?.name ?? t("serversettings.shared.pickChannel")}</span>
          <ChevronDownIcon className="size-4 shrink-0 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-72 w-56 overflow-y-auto">
        {channels.map((c) => (
          <DropdownMenuItem key={c.id} onSelect={() => onChange(c.id)}>
            <HashIcon /> {c.name}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
