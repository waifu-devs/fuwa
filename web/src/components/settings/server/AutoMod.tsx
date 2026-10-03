import { create, equals } from "@bufbuild/protobuf";
import {
  AtSignIcon,
  BanIcon,
  BellRingIcon,
  CheckIcon,
  ChevronDownIcon,
  FlaskConicalIcon,
  GlobeLockIcon,
  HashIcon,
  LinkIcon,
  LoaderCircleIcon,
  PlusIcon,
  ShieldCheckIcon,
  ShieldIcon,
  SparklesIcon,
  TimerIcon,
  Trash2Icon,
  TypeIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useState, type ClipboardEvent, type KeyboardEvent, type ReactNode } from "react";
import type { AutoModProvider } from "@/gen/fuwa/v1/automod_pb";
import {
  AutoModActionKind,
  AutoModActionSchema,
  AutoModLabelRuleSchema,
  AutoModLevel,
  AutoModRuleSchema,
  AutoModTrigger,
  ChannelType,
  type AutoModAction,
  type AutoModRule,
} from "@/gen/fuwa/v1/types_pb";
import { deleteAutoModRule, listAutoModRules, run, saveAutoModRule, testAutoModRule } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction, useInstance, useRoles } from "@/fuwa/hooks";
import { RoleDot } from "@/components/chat/mentions";
import { SPRING } from "@/components/motion";
import { Chips } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { formatDuration } from "@/lib/format";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const MAX_KEYWORD_RULES = 6;
const MAX_WORDS = 1000;
const MAX_ALLOWED = 100;

/** The three kinds of rule, as the page lists them. */
const KINDS = [
  {
    trigger: AutoModTrigger.KEYWORDS,
    label: "Blocked words",
    blurb: "Words and phrases you don't want said. Up to six lists, each with its own actions.",
    icon: TypeIcon,
    tint: "from-rose-500/20 text-rose-500",
    max: MAX_KEYWORD_RULES,
  },
  {
    trigger: AutoModTrigger.MENTION_SPAM,
    label: "Mention spam",
    blurb: "Messages that ping too many people or roles at once, the usual sign of a raid.",
    icon: AtSignIcon,
    tint: "from-amber-500/20 text-amber-500",
    max: 1,
  },
  {
    trigger: AutoModTrigger.LINKS,
    label: "Links",
    blurb: "Links to sites you haven't allowed. Allowing a site allows its subdomains too.",
    icon: LinkIcon,
    tint: "from-sky-500/20 text-sky-500",
    max: 1,
  },
  {
    trigger: AutoModTrigger.PROVIDER,
    label: "Smart filter",
    blurb: "A moderation service reads each message and says what kind it is: hate, harassment, scams, spam and more. You pick what happens for each.",
    icon: SparklesIcon,
    tint: "from-violet-500/20 text-violet-500",
    max: 1,
  },
] as const;

/** What a smart filter can do about one kind of message, mildest first. */
const LEVELS = [
  { level: AutoModLevel.OFF, label: "Nothing", short: "Off" },
  { level: AutoModLevel.FLAG, label: "Flag", short: "Flag" },
  { level: AutoModLevel.BLOCK, label: "Block", short: "Block" },
  { level: AutoModLevel.TIME_OUT, label: "Block + time out", short: "Time out" },
] as const;

/** The level a label is at (unset reads as nothing). */
const levelOf = (level: AutoModLevel) => (level === AutoModLevel.UNSPECIFIED ? AutoModLevel.OFF : level);

const TIME_OUTS = [
  { value: 60, label: "1 minute" },
  { value: 300, label: "5 minutes" },
  { value: 600, label: "10 minutes" },
  { value: 3600, label: "1 hour" },
  { value: 86_400, label: "1 day" },
  { value: 604_800, label: "1 week" },
];

function fresh(trigger: AutoModTrigger, provider?: AutoModProvider, alertChannelId = ""): AutoModRule {
  if (trigger === AutoModTrigger.PROVIDER && provider)
    return create(AutoModRuleSchema, {
      enabled: true,
      trigger,
      provider: provider.id,
      labels: provider.labels.map((l) => create(AutoModLabelRuleSchema, { label: l.id, level: l.defaultLevel, threshold: 80 })),
      actions: [create(AutoModActionSchema, { kind: AutoModActionKind.ALERT, channelId: alertChannelId })],
    });
  return create(AutoModRuleSchema, {
    enabled: true,
    trigger,
    mentionLimit: trigger === AutoModTrigger.MENTION_SPAM ? 5 : 0,
    actions: [create(AutoModActionSchema, { kind: AutoModActionKind.BLOCK })],
  });
}

/**
 * AutoMod: rules that read every message as it's sent or edited, and block
 * it, tell the mods, or time its author out. People who can manage the
 * server are never caught.
 */
export function AutoMod({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const [rules, setRules] = useState<AutoModRule[] | null>(null);
  const [providers, setProviders] = useState<AutoModProvider[]>([]);
  const [drafts, setDrafts] = useState<{ key: string; rule: AutoModRule }[]>([]);
  const [problem, setProblem] = useState<string | null>(null);
  const inst = useInstance(instanceKey);
  const firstTextChannel = (inst?.channels[serverId] ?? []).find((c) => c.type === ChannelType.TEXT)?.id ?? "";

  useEffect(() => {
    run(listAutoModRules(instanceKey, serverId)).then(
      (r) => {
        setRules(r.rules);
        setProviders(r.providers);
      },
      (e: FuwaError) => setProblem(e.message),
    );
  }, [instanceKey, serverId]);

  if (problem) return <p className="text-sm text-muted-foreground first-letter:uppercase">{problem}</p>;
  if (!rules)
    return (
      <div className="flex flex-col gap-3">
        {[0, 1, 2].map((n) => (
          <div key={n} className="shimmer h-28 rounded-3xl" />
        ))}
      </div>
    );

  return (
    <div className="flex flex-col gap-6">
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        className="flex items-start gap-3 rounded-2xl border border-emerald-500/30 bg-emerald-500/5 p-3 text-sm"
      >
        <motion.span
          animate={{ rotate: [0, -8, 8, 0] }}
          transition={{ duration: 1.2, repeat: Infinity, repeatDelay: 4 }}
          className="grid size-8 shrink-0 place-items-center rounded-xl bg-emerald-500/15 text-emerald-600 dark:text-emerald-400"
        >
          <ShieldCheckIcon className="size-4" />
        </motion.span>
        <p className="text-muted-foreground">
          AutoMod reads each message as it's sent or edited, before anyone sees it. People who can manage the server are never caught, so test a rule with the
          box under it rather than in chat.
        </p>
      </motion.div>
      {KINDS.map((kind, n) => {
        const mine = rules.filter((r) => r.trigger === kind.trigger);
        const pending = drafts.filter((d) => d.rule.trigger === kind.trigger);
        const unavailable = kind.trigger === AutoModTrigger.PROVIDER && providers.length === 0;
        const room = mine.length + pending.length < kind.max && !unavailable;
        return (
          <motion.section
            key={kind.trigger}
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...SPRING, delay: n * 0.06 }}
            className="flex flex-col gap-3"
          >
            <div className="flex flex-wrap items-center gap-3">
              <span className={cn("grid size-10 shrink-0 place-items-center rounded-2xl bg-gradient-to-br to-transparent", kind.tint)}>
                <kind.icon className="size-5" />
              </span>
              <div className="min-w-48 flex-1">
                <h3 className="font-extrabold">{kind.label}</h3>
                <p className="text-sm text-muted-foreground">{kind.blurb}</p>
              </div>
              {room && (
                <Button
                  type="button"
                  variant={mine.length + pending.length ? "ghost" : "default"}
                  size="sm"
                  className="group ml-auto shrink-0 rounded-xl font-bold"
                  onClick={() =>
                    setDrafts((d) => [...d, { key: `new-${Date.now()}`, rule: fresh(kind.trigger, providers[0], firstTextChannel) }])
                  }
                >
                  <PlusIcon className="transition-transform group-hover:rotate-90" />
                  {mine.length + pending.length ? "Another list" : "Set up"}
                </Button>
              )}
            </div>
            {unavailable && mine.length === 0 && (
              <motion.p
                initial={{ opacity: 0, y: 6 }}
                animate={{ opacity: 1, y: 0 }}
                transition={SPRING}
                className="rounded-2xl border border-dashed p-3 text-sm text-muted-foreground"
              >
                This instance hasn't turned on a moderation service yet. Its admins can, under Instance settings, Moderation: TypeSafe Jev and Cloudflare Clef take
                a key and one switch.
              </motion.p>
            )}
            <AnimatePresence initial={false}>
              {mine.map((rule) => (
                <RuleCard
                  key={rule.id}
                  instanceKey={instanceKey}
                  serverId={serverId}
                  providers={providers}
                  saved={rule}
                  onSaved={(next) => setRules((list) => list?.map((r) => (r.id === next.id ? next : r)) ?? null)}
                  onDeleted={() => setRules((list) => list?.filter((r) => r.id !== rule.id) ?? null)}
                />
              ))}
              {pending.map((d) => (
                <RuleCard
                  key={d.key}
                  instanceKey={instanceKey}
                  serverId={serverId}
                  providers={providers}
                  saved={d.rule}
                  isNew
                  onSaved={(next) => {
                    setDrafts((list) => list.filter((x) => x.key !== d.key));
                    setRules((list) => [...(list ?? []), next]);
                  }}
                  onDeleted={() => setDrafts((list) => list.filter((x) => x.key !== d.key))}
                />
              ))}
            </AnimatePresence>
          </motion.section>
        );
      })}
    </div>
  );
}

const actionOf = (rule: AutoModRule, kind: AutoModActionKind) => rule.actions.find((a) => a.kind === kind);

/** One rule: a header that toggles it on and off, and opens to everything it does. */
function RuleCard({
  instanceKey,
  serverId,
  providers,
  saved,
  isNew = false,
  onSaved,
  onDeleted,
}: {
  instanceKey: string;
  serverId: string;
  providers: AutoModProvider[];
  saved: AutoModRule;
  isNew?: boolean;
  onSaved: (rule: AutoModRule) => void;
  onDeleted: () => void;
}) {
  const [draft, setDraft] = useState(saved);
  const [open, setOpen] = useState(isNew);
  const [confirm, setConfirm] = useState(false);
  const save = useAction(saveAutoModRule);
  const remove = useAction(deleteAutoModRule);
  const shake = useAnimationControls();
  const kind = KINDS.find((k) => k.trigger === saved.trigger)!;
  const dirty = isNew || !equals(AutoModRuleSchema, draft, saved);
  useEffect(() => setDraft(saved), [saved]);

  const set = (patch: Partial<AutoModRule>) => {
    setDraft((d) => create(AutoModRuleSchema, { ...d, ...patch }));
    save.setError(null);
  };
  const setAction = (kindOf: AutoModActionKind, action: Partial<AutoModAction> | null) => {
    const rest = draft.actions.filter((a) => a.kind !== kindOf);
    const current = actionOf(draft, kindOf);
    const next = action
      ? [
          ...rest,
          create(AutoModActionSchema, {
            kind: kindOf,
            message: action.message ?? current?.message ?? "",
            channelId: action.channelId ?? current?.channelId ?? "",
            durationSeconds: action.durationSeconds ?? current?.durationSeconds ?? 0,
          }),
        ]
      : rest;
    set({ actions: next.sort((a, b) => a.kind - b.kind) });
  };

  async function submit() {
    const next = await save.go(instanceKey, serverId, draft);
    if (next) {
      onSaved(next);
      toast(isNew ? `${next.name} is on guard` : `Saved ${next.name}`);
    } else
      void shake.start({
        x: [0, -8, 7, -5, 3, 0],
        transition: { duration: 0.4 },
      });
  }

  async function toggle(enabled: boolean) {
    if (isNew || dirty) return set({ enabled });
    const next = await save.go(instanceKey, serverId, create(AutoModRuleSchema, { ...saved, enabled }));
    if (next) onSaved(next);
  }

  async function destroy() {
    if (isNew) return onDeleted();
    if ((await remove.go(instanceKey, serverId, saved.id)) !== undefined) onDeleted();
  }

  const smart = draft.trigger === AutoModTrigger.PROVIDER;
  const provider = providers.find((p) => p.id === draft.provider);
  const watching = draft.labels.filter((l) => levelOf(l.level) !== AutoModLevel.OFF).length;
  const doing = smart
    ? []
    : [
        actionOf(draft, AutoModActionKind.BLOCK) && "blocks",
        actionOf(draft, AutoModActionKind.ALERT) && "alerts",
        actionOf(draft, AutoModActionKind.TIME_OUT) && "times out",
      ].filter(Boolean);
  const summary = smart
    ? `${provider?.name ?? "Provider turned off on this instance"} · watching ${watching} ${watching === 1 ? "kind" : "kinds"}`
    : draft.trigger === AutoModTrigger.KEYWORDS
      ? `${draft.keywords.length} ${draft.keywords.length === 1 ? "word" : "words"}`
      : draft.trigger === AutoModTrigger.MENTION_SPAM
        ? `More than ${draft.mentionLimit} pings`
        : `${draft.allowed.length} allowed ${draft.allowed.length === 1 ? "site" : "sites"}`;

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: -8, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.95, height: 0 }}
      transition={SPRING}
    >
      <motion.div
        animate={shake}
        className={cn(
          "overflow-hidden rounded-3xl border bg-card transition-colors",
          open && "border-primary/30 shadow-lg",
          !draft.enabled && !open && "opacity-70",
        )}
      >
        <div className="flex items-center gap-3 p-3 pl-4">
          <button type="button" onClick={() => setOpen((o) => !o)} className="group flex min-w-0 flex-1 items-center gap-3 text-left" aria-expanded={open}>
            <span className="min-w-0 flex-1">
              <span className="block truncate font-bold">{draft.name || kind.label}</span>
              <span className="block truncate text-xs text-muted-foreground">
                {summary}
                {doing.length > 0 && ` · ${doing.join(", ")}`}
              </span>
            </span>
            <AnimatePresence initial={false}>
              {dirty && !isNew && (
                <motion.span
                  initial={{ scale: 0 }}
                  animate={{ scale: 1 }}
                  exit={{ scale: 0 }}
                  transition={SPRING}
                  className="rounded-full bg-primary/15 px-2 py-0.5 text-[0.65rem] font-bold text-primary uppercase"
                >
                  Unsaved
                </motion.span>
              )}
            </AnimatePresence>
            <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
          </button>
          <Switch checked={draft.enabled} onCheckedChange={(on) => void toggle(on)} aria-label={draft.enabled ? "Turn the rule off" : "Turn the rule on"} />
        </div>
        <AnimatePresence initial={false}>
          {open && (
            <motion.div
              key="body"
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: "auto", opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={{ ...SPRING, opacity: { duration: 0.15 } }}
            >
              <div className="flex flex-col gap-5 border-t p-4">
                <Field label="Name">
                  <Input
                    value={draft.name}
                    maxLength={100}
                    placeholder={kind.label}
                    onChange={(e) => set({ name: e.target.value })}
                    className="h-10 rounded-xl"
                  />
                </Field>
                {smart ? (
                  <>
                    <ProviderPicker providers={providers} draft={draft} set={set} />
                    <Labels provider={provider} draft={draft} set={set} />
                    <Tester instanceKey={instanceKey} serverId={serverId} rule={draft} />
                    <SmartActions instanceKey={instanceKey} serverId={serverId} draft={draft} setAction={setAction} />
                  </>
                ) : (
                  <>
                    <Trigger draft={draft} set={set} />
                    <Tester instanceKey={instanceKey} serverId={serverId} rule={draft} />
                    <Actions instanceKey={instanceKey} serverId={serverId} draft={draft} setAction={setAction} />
                  </>
                )}
                <Exemptions instanceKey={instanceKey} serverId={serverId} draft={draft} set={set} />
                <div className="flex flex-wrap items-center gap-2 border-t pt-4">
                  <AnimatePresence mode="popLayout" initial={false}>
                    {confirm ? (
                      <motion.span
                        key="sure"
                        initial={{ opacity: 0, x: -8 }}
                        animate={{ opacity: 1, x: 0 }}
                        exit={{ opacity: 0, x: -8 }}
                        transition={SPRING}
                        className="flex items-center gap-1"
                      >
                        <Button
                          type="button"
                          variant="destructive"
                          size="sm"
                          className="rounded-xl font-bold"
                          disabled={remove.pending}
                          onClick={() => void destroy()}
                        >
                          {remove.pending && <LoaderCircleIcon className="animate-spin" />}
                          Delete {draft.name || kind.label}
                        </Button>
                        <Button type="button" variant="ghost" size="sm" className="rounded-xl" onClick={() => setConfirm(false)}>
                          Keep it
                        </Button>
                      </motion.span>
                    ) : (
                      <motion.span key="ask" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={SPRING}>
                        <Button
                          type="button"
                          variant="ghost"
                          size="sm"
                          className="rounded-xl text-muted-foreground hover:text-destructive"
                          onClick={() => (isNew ? onDeleted() : setConfirm(true))}
                        >
                          <Trash2Icon /> {isNew ? "Cancel" : "Delete"}
                        </Button>
                      </motion.span>
                    )}
                  </AnimatePresence>
                  <span className="min-w-0 flex-1 text-right text-sm text-destructive first-letter:uppercase">{save.error ?? remove.error}</span>
                  {dirty && !isNew && (
                    <Button type="button" variant="ghost" size="sm" className="rounded-xl" onClick={() => setDraft(saved)} disabled={save.pending}>
                      Discard
                    </Button>
                  )}
                  <Button type="button" size="sm" className="btn rounded-xl px-4 font-bold" disabled={!dirty || save.pending} onClick={() => void submit()}>
                    {save.pending ? <LoaderCircleIcon className="animate-spin" /> : <CheckIcon />}
                    {isNew ? "Create rule" : "Save"}
                  </Button>
                </div>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </motion.div>
    </motion.div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <span>
        <span className="block text-sm font-extrabold">{label}</span>
        {hint && <span className="block text-xs text-muted-foreground">{hint}</span>}
      </span>
      {children}
    </div>
  );
}

/** What the rule looks for: words, a ping limit, or which sites are fine. */
function Trigger({ draft, set }: { draft: AutoModRule; set: (patch: Partial<AutoModRule>) => void }) {
  if (draft.trigger === AutoModTrigger.KEYWORDS)
    return (
      <>
        <Field
          label="Words and phrases"
          hint={
            <>
              Matched whole, ignoring case. A <b>*</b> lets a word run on: <code>*cat</code> catches “bobcat”, <code>cat*</code> catches “catapult”.
            </>
          }
        >
          <WordList words={draft.keywords} max={MAX_WORDS} placeholder="Type a word, then Enter" onChange={(keywords) => set({ keywords })} wild />
        </Field>
        <Field label="Allowed anyway" hint="Words the list would catch that are fine, like “class” under *ass*.">
          <WordList words={draft.allowed} max={MAX_ALLOWED} placeholder="Type a word, then Enter" onChange={(allowed) => set({ allowed })} tone="ok" />
        </Field>
      </>
    );
  if (draft.trigger === AutoModTrigger.MENTION_SPAM)
    return (
      <Field label="Ping limit" hint="Each person or role counts once; @everyone and @here count as one together.">
        <div className="flex items-center gap-3">
          <input
            type="range"
            min={1}
            max={50}
            value={draft.mentionLimit}
            onChange={(e) => set({ mentionLimit: Number(e.target.value) })}
            aria-label="Ping limit"
            className="h-2 flex-1 cursor-pointer accent-primary"
          />
          <motion.span
            key={draft.mentionLimit}
            initial={{ scale: 1.3 }}
            animate={{ scale: 1 }}
            transition={SPRING}
            className="w-28 shrink-0 text-sm font-bold tabular-nums"
          >
            More than {draft.mentionLimit}
          </motion.span>
        </div>
      </Field>
    );
  return (
    <Field label="Allowed sites" hint="Every other link is caught. Leave it empty to catch them all.">
      <WordList words={draft.allowed} max={MAX_ALLOWED} placeholder="example.com, then Enter" onChange={(allowed) => set({ allowed })} tone="ok" />
    </Field>
  );
}

/** A list of words as chips: type and press Enter or a comma, paste many lines at once, Backspace takes the last one off. */
function WordList({
  words,
  max,
  placeholder,
  onChange,
  wild = false,
  tone = "block",
}: {
  words: string[];
  max: number;
  placeholder: string;
  onChange: (words: string[]) => void;
  wild?: boolean;
  tone?: "block" | "ok";
}) {
  const [text, setText] = useState("");
  const add = (raw: string[]) => {
    const fresh = raw.map((w) => w.trim().toLowerCase()).filter((w) => w && !words.includes(w));
    if (fresh.length) onChange([...words, ...new Set(fresh)].slice(0, max));
  };
  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter" || e.key === ",") {
      e.preventDefault();
      add([text]);
      setText("");
    } else if (e.key === "Backspace" && !text && words.length) onChange(words.slice(0, -1));
  }
  function onPaste(e: ClipboardEvent<HTMLInputElement>) {
    const pasted = e.clipboardData.getData("text");
    if (!/[\n,]/.test(pasted)) return;
    e.preventDefault();
    add(pasted.split(/[\n,]/));
  }
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex min-h-11 flex-wrap items-center gap-1.5 rounded-xl border p-1.5 focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/15">
        <AnimatePresence initial={false}>
          {words.map((w) => (
            <motion.span
              key={w}
              layout
              initial={{ opacity: 0, scale: 0.5 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.5 }}
              transition={{ type: "spring", stiffness: 600, damping: 24 }}
              className={cn(
                "flex items-center gap-1 rounded-lg py-0.5 pr-1 pl-2 text-sm font-bold",
                tone === "ok" ? "bg-emerald-500/12 text-emerald-700 dark:text-emerald-300" : "bg-rose-500/12 text-rose-700 dark:text-rose-300",
              )}
            >
              <span>
                {wild
                  ? w.split(/(\*)/).map((part, n) =>
                      part === "*" ? (
                        <span key={n} className="opacity-50">
                          *
                        </span>
                      ) : (
                        part
                      ),
                    )
                  : w}
              </span>
              <button
                type="button"
                aria-label={`Remove ${w}`}
                onClick={() => onChange(words.filter((x) => x !== w))}
                className="grid size-4 place-items-center rounded opacity-60 hover:opacity-100"
              >
                <XIcon className="size-3" />
              </button>
            </motion.span>
          ))}
        </AnimatePresence>
        <input
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onBlur={() => {
            add([text]);
            setText("");
          }}
          placeholder={words.length ? "" : placeholder}
          maxLength={253}
          className="h-7 min-w-24 flex-1 bg-transparent px-1.5 text-sm outline-none"
        />
      </div>
      {words.length > 0 && (
        <p className="text-right text-xs text-muted-foreground tabular-nums">
          {words.length}/{max}
        </p>
      )}
    </div>
  );
}

/** Try a message against the rule as it stands, saved or not. Matches light up. */
function Tester({ instanceKey, serverId, rule }: { instanceKey: string; serverId: string; rule: AutoModRule }) {
  const [text, setText] = useState("");
  const [result, setResult] = useState<{
    matched: boolean;
    matches: string[];
    error: string;
    elapsedMs: number;
  } | null>(null);
  const [checking, setChecking] = useState(false);
  const smart = rule.trigger === AutoModTrigger.PROVIDER;
  const key = useMemo(
    () =>
      JSON.stringify([
        rule.trigger,
        rule.keywords,
        rule.allowed,
        rule.mentionLimit,
        rule.provider,
        rule.labels.map((l) => [l.label, l.level, l.threshold]),
        text,
      ]),
    [rule, text],
  );

  useEffect(() => {
    if (!text.trim()) return setResult(null);
    let live = true;
    setChecking(true);
    const t = setTimeout(() => {
      run(testAutoModRule(instanceKey, serverId, rule, text))
        .then((r) => live && setResult({ matched: r.matched, matches: r.matches, error: r.error, elapsedMs: r.elapsedMs }))
        .catch(() => live && setResult(null))
        .finally(() => live && setChecking(false));
      // A smart filter asks its provider each time, so wait for a pause in typing.
    }, smart ? 700 : 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, instanceKey, serverId]);

  return (
    <div className="flex flex-col gap-2 rounded-2xl bg-muted/50 p-3">
      <span className="flex items-center gap-1.5 text-sm font-extrabold">
        <FlaskConicalIcon className="size-4 text-primary" /> Try a message
      </span>
      <Textarea
        value={text}
        rows={2}
        placeholder={smart ? "Write something it should flag or block, or let through. It goes to the provider." : "Write something the rule should catch, or let through."}
        onChange={(e) => setText(e.target.value)}
        className="rounded-xl bg-background"
      />
      <AnimatePresence mode="popLayout" initial={false}>
        {text.trim() && (
          <motion.div
            key={checking ? "checking" : result?.error ? "error" : result?.matched ? `caught-${result.matches.join()}` : "fine"}
            initial={{ opacity: 0, y: 6, scale: 0.97 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -6 }}
            transition={SPRING}
            className="flex flex-wrap items-center gap-1.5 text-sm"
          >
            {checking || !result ? (
              <span className="flex items-center gap-1.5 text-muted-foreground">
                <LoaderCircleIcon className="size-3.5 animate-spin" /> {smart ? "Asking the provider…" : "Checking…"}
              </span>
            ) : result.error ? (
              <span className="text-xs text-amber-700 first-letter:uppercase dark:text-amber-400">
                The provider didn't answer: {result.error}. Messages go through this rule unchecked until it does; your other rules still apply.
              </span>
            ) : result.matched ? (
              <>
                <span className="flex items-center gap-1 rounded-full bg-destructive/15 px-2 py-0.5 text-xs font-bold text-destructive">
                  <BanIcon className="size-3" /> Caught
                </span>
                {result.matches.map((m, n) => (
                  <motion.code
                    key={m}
                    initial={{ scale: 0.6, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    transition={{
                      type: "spring",
                      stiffness: 600,
                      damping: 16,
                      delay: n * 0.05,
                    }}
                    className="rounded-md bg-destructive/10 px-1.5 py-0.5 text-xs text-destructive"
                  >
                    {m}
                  </motion.code>
                ))}
              </>
            ) : (
              <span className="flex items-center gap-1 rounded-full bg-emerald-500/15 px-2 py-0.5 text-xs font-bold text-emerald-600 dark:text-emerald-400">
                <CheckIcon className="size-3" strokeWidth={3} /> Gets through
              </span>
            )}
            {smart && result && !checking && <span className="ml-auto text-xs text-muted-foreground tabular-nums">{result.elapsedMs} ms</span>}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** What happens to a message the rule catches: any of block, alert and time out. */
function Actions({
  instanceKey,
  serverId,
  draft,
  setAction,
}: {
  instanceKey: string;
  serverId: string;
  draft: AutoModRule;
  setAction: (kind: AutoModActionKind, action: Partial<AutoModAction> | null) => void;
}) {
  const inst = useInstance(instanceKey);
  const textChannels = (inst?.channels[serverId] ?? []).filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT);
  const block = actionOf(draft, AutoModActionKind.BLOCK);
  const alert = actionOf(draft, AutoModActionKind.ALERT);
  const timeOut = actionOf(draft, AutoModActionKind.TIME_OUT);
  const alertChannel = textChannels.find((c) => c.id === alert?.channelId);
  const durations =
    timeOut && !TIME_OUTS.some((t) => t.value === timeOut.durationSeconds)
      ? [
          ...TIME_OUTS,
          {
            value: timeOut.durationSeconds,
            label: formatDuration(timeOut.durationSeconds),
          },
        ].sort((a, b) => a.value - b.value)
      : TIME_OUTS;
  return (
    <Field label="When it catches one" hint="Pick any. Without blocking, the message is still sent.">
      <div className="flex flex-col gap-2">
        <ActionCard
          on={!!block}
          icon={<BanIcon className="size-4" />}
          title="Block the message"
          hint="It never reaches the channel. Its author sees why."
          onToggle={(on) => setAction(AutoModActionKind.BLOCK, on ? {} : null)}
        >
          <Input
            value={block?.message ?? ""}
            maxLength={150}
            placeholder="What they're told (optional): “Keep it friendly, please.”"
            onChange={(e) => setAction(AutoModActionKind.BLOCK, { message: e.target.value })}
            className="h-9 rounded-xl"
          />
        </ActionCard>
        <ActionCard
          on={!!alert}
          icon={<BellRingIcon className="size-4" />}
          title="Alert a channel"
          hint="Posts what was caught, who said it and where, for your mods."
          onToggle={(on) => setAction(AutoModActionKind.ALERT, on ? { channelId: alert?.channelId || textChannels[0]?.id || "" } : null)}
        >
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="group flex h-9 w-full items-center gap-2 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60"
              >
                <HashIcon className="size-4 text-muted-foreground" />
                <span className="flex-1 truncate font-bold">{alertChannel?.name ?? "Pick a channel"}</span>
                <ChevronDownIcon className="size-4 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start" className="max-h-72 w-60 overflow-y-auto">
              {textChannels.map((c) => (
                <DropdownMenuItem key={c.id} onSelect={() => setAction(AutoModActionKind.ALERT, { channelId: c.id })}>
                  <HashIcon /> {c.name}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        </ActionCard>
        <ActionCard
          on={!!timeOut}
          icon={<TimerIcon className="size-4" />}
          title="Time them out"
          hint="They can read but not talk for a while."
          onToggle={(on) => setAction(AutoModActionKind.TIME_OUT, on ? { durationSeconds: timeOut?.durationSeconds || 60 } : null)}
        >
          <Chips
            label="Time-out length"
            value={timeOut?.durationSeconds ?? 60}
            options={durations}
            onChange={(durationSeconds) => setAction(AutoModActionKind.TIME_OUT, { durationSeconds })}
          />
        </ActionCard>
      </div>
    </Field>
  );
}

function ActionCard({
  on,
  icon,
  title,
  hint,
  onToggle,
  children,
}: {
  on: boolean;
  icon: ReactNode;
  title: string;
  hint: string;
  onToggle: (on: boolean) => void;
  children: ReactNode;
}) {
  return (
    <div className={cn("rounded-2xl border p-3 transition-colors", on ? "border-primary/40 bg-primary/5" : "hover:border-primary/20")}>
      <label className="flex cursor-pointer items-center gap-3">
        <motion.span
          animate={on ? { scale: [1, 1.2, 1], rotate: [0, -10, 0] } : { scale: 1 }}
          transition={{ duration: 0.35 }}
          className={cn(
            "grid size-8 shrink-0 place-items-center rounded-xl transition-colors",
            on ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground",
          )}
        >
          {icon}
        </motion.span>
        <span className="min-w-0 flex-1">
          <span className="block text-sm font-bold">{title}</span>
          <span className="block text-xs text-muted-foreground">{hint}</span>
        </span>
        <Switch checked={on} onCheckedChange={onToggle} />
      </label>
      <AnimatePresence initial={false}>
        {on && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={SPRING}
            className="overflow-hidden"
          >
            <div className="pt-3">{children}</div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Roles and channels the rule leaves alone. */
function Exemptions({
  instanceKey,
  serverId,
  draft,
  set,
}: {
  instanceKey: string;
  serverId: string;
  draft: AutoModRule;
  set: (patch: Partial<AutoModRule>) => void;
}) {
  const inst = useInstance(instanceKey);
  const roles = useRoles(instanceKey, serverId).filter((r) => r.id !== serverId);
  const channels = (inst?.channels[serverId] ?? []).filter((c) => c.type !== ChannelType.CATEGORY && c.type !== ChannelType.VOICE);
  const flip = (list: string[], id: string) => (list.includes(id) ? list.filter((x) => x !== id) : [...list, id]);
  const chosenRoles = roles.filter((r) => draft.exemptRoleIds.includes(r.id));
  const chosenChannels = channels.filter((c) => draft.exemptChannelIds.includes(c.id));
  return (
    <Field label="Leave out" hint="Roles and channels this rule never looks at.">
      <div className="flex flex-wrap items-center gap-1.5">
        <AnimatePresence initial={false}>
          {chosenRoles.map((r) => (
            <Pill key={r.id} onRemove={() => set({ exemptRoleIds: flip(draft.exemptRoleIds, r.id) })}>
              <RoleDot role={r} /> {r.name}
            </Pill>
          ))}
          {chosenChannels.map((c) => (
            <Pill key={c.id} onRemove={() => set({ exemptChannelIds: flip(draft.exemptChannelIds, c.id) })}>
              <HashIcon className="size-3.5 text-muted-foreground" /> {c.name}
            </Pill>
          ))}
        </AnimatePresence>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" size="sm" className="h-7 rounded-lg border-dashed text-xs" disabled={!roles.length}>
              <ShieldIcon /> Roles
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-72 w-56 overflow-y-auto">
            {roles.map((r) => (
              <DropdownMenuCheckboxItem
                key={r.id}
                checked={draft.exemptRoleIds.includes(r.id)}
                onSelect={(e) => e.preventDefault()}
                onCheckedChange={() => set({ exemptRoleIds: flip(draft.exemptRoleIds, r.id) })}
              >
                <RoleDot role={r} /> {r.name}
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button type="button" variant="outline" size="sm" className="h-7 rounded-lg border-dashed text-xs">
              <HashIcon /> Channels
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-72 w-56 overflow-y-auto">
            {channels.map((c) => (
              <DropdownMenuCheckboxItem
                key={c.id}
                checked={draft.exemptChannelIds.includes(c.id)}
                onSelect={(e) => e.preventDefault()}
                onCheckedChange={() => set({ exemptChannelIds: flip(draft.exemptChannelIds, c.id) })}
              >
                #{c.name}
              </DropdownMenuCheckboxItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </Field>
  );
}

/** Which of the instance's moderation services reads messages, and what it gets to see. */
function ProviderPicker({
  providers,
  draft,
  set,
}: {
  providers: AutoModProvider[];
  draft: AutoModRule;
  set: (patch: Partial<AutoModRule>) => void;
}) {
  const chosen = providers.find((p) => p.id === draft.provider);
  return (
    <Field label="Provider" hint="The services this instance's admins turned on.">
      {providers.length > 1 && (
        <div className="flex flex-wrap gap-1 rounded-2xl bg-muted/60 p-1" role="radiogroup" aria-label="Provider">
          {providers.map((p) => {
            const on = p.id === draft.provider;
            return (
              <button
                key={p.id}
                type="button"
                role="radio"
                aria-checked={on}
                onClick={() => set({ provider: p.id })}
                className={cn("relative flex-1 rounded-xl px-3 py-1.5 text-sm font-bold transition-colors", on ? "text-foreground" : "text-muted-foreground hover:text-foreground")}
              >
                {on && <motion.span layoutId={`provider-${draft.id || "new"}`} transition={SPRING} className="absolute inset-0 rounded-xl bg-background shadow-sm" />}
                <span className="relative">{p.name}</span>
              </button>
            );
          })}
        </div>
      )}
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.div
          key={chosen?.id ?? "none"}
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -6 }}
          transition={SPRING}
          className={cn(
            "flex items-start gap-2.5 rounded-2xl border p-3 text-sm",
            chosen ? "border-violet-500/30 bg-violet-500/5" : "border-amber-500/40 bg-amber-500/5",
          )}
        >
          <GlobeLockIcon className={cn("mt-0.5 size-4 shrink-0", chosen ? "text-violet-500" : "text-amber-500")} />
          {chosen ? (
            <span className="min-w-0 text-muted-foreground">
              <b className="text-foreground">{chosen.name}</b> reads the messages this rule checks, at <b className="break-all text-foreground">{chosen.host}</b>. It
              gets the text alone: never who wrote it, where, or this server's name, and mentions and custom emoji are swapped for placeholders first. Your
              instance sends it, never anyone's app.
            </span>
          ) : (
            <span className="min-w-0 text-amber-700 dark:text-amber-400">
              This instance turned that provider off, so the rule lets every message through. Pick another, or ask the instance's admins.
            </span>
          )}
        </motion.div>
      </AnimatePresence>
    </Field>
  );
}

/** Each kind of message the provider tells apart, with what happens to it and how sure it must be. */
function Labels({ provider, draft, set }: { provider?: AutoModProvider; draft: AutoModRule; set: (patch: Partial<AutoModRule>) => void }) {
  const labels = provider?.labels ?? [];
  const ruleFor = (id: string) => draft.labels.find((l) => l.label === id);
  const change = (id: string, patch: { level?: AutoModLevel; threshold?: number }) => {
    const current = ruleFor(id);
    const next = create(AutoModLabelRuleSchema, {
      label: id,
      level: patch.level ?? current?.level ?? AutoModLevel.OFF,
      threshold: patch.threshold ?? (current?.threshold || 80),
    });
    set({ labels: [...draft.labels.filter((l) => l.label !== id), next] });
  };
  const resetAll = () =>
    set({ labels: labels.map((l) => create(AutoModLabelRuleSchema, { label: l.id, level: l.defaultLevel, threshold: 80 })) });
  if (!labels.length) return null;
  return (
    <Field
      label="What to do about each kind"
      hint={
        <>
          Flags post to your alert channel and let the message through. The bar is how sure the provider must be.{" "}
          <button type="button" onClick={resetAll} className="font-bold text-primary hover:underline">
            Back to the defaults
          </button>
        </>
      }
    >
      <div className="flex flex-col gap-2">
        {labels.map((label, n) => {
          const rule = ruleFor(label.id);
          const level = levelOf(rule?.level ?? label.defaultLevel);
          const threshold = rule?.threshold || 80;
          const off = level === AutoModLevel.OFF;
          return (
            <motion.div
              key={label.id}
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ ...SPRING, delay: n * 0.03 }}
              className={cn("rounded-2xl border p-3 transition-colors", off ? "bg-transparent" : "border-primary/25 bg-primary/[0.03]")}
            >
              <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
                <span className={cn("min-w-40 flex-1 transition-opacity", off && "opacity-60")}>
                  <span className="block text-sm font-bold">{label.name}</span>
                  <span className="block text-xs text-muted-foreground">{label.description}</span>
                </span>
                <div className="flex w-full gap-0.5 rounded-xl bg-muted/70 p-0.5 sm:w-auto" role="radiogroup" aria-label={`What to do about ${label.name}`}>
                  {LEVELS.map((option) => {
                    const on = option.level === level;
                    return (
                      <button
                        key={option.level}
                        type="button"
                        role="radio"
                        aria-checked={on}
                        title={option.label}
                        onClick={() => change(label.id, { level: option.level })}
                        className={cn(
                          "relative flex-1 rounded-[0.6rem] px-2.5 py-1 text-xs font-bold whitespace-nowrap transition-colors sm:flex-none",
                          on ? (option.level >= AutoModLevel.BLOCK ? "text-white" : "text-foreground") : "text-muted-foreground hover:text-foreground",
                        )}
                      >
                        {on && (
                          <motion.span
                            layoutId={`level-${draft.id || "new"}-${label.id}`}
                            transition={SPRING}
                            className={cn(
                              "absolute inset-0 rounded-[0.6rem] shadow-sm",
                              option.level >= AutoModLevel.BLOCK ? "bg-destructive" : option.level === AutoModLevel.FLAG ? "bg-amber-400/80" : "bg-background",
                            )}
                          />
                        )}
                        <span className="relative">{option.short}</span>
                      </button>
                    );
                  })}
                </div>
              </div>
              <AnimatePresence initial={false}>
                {!off && (
                  <motion.div
                    initial={{ height: 0, opacity: 0 }}
                    animate={{ height: "auto", opacity: 1 }}
                    exit={{ height: 0, opacity: 0 }}
                    transition={SPRING}
                    className="overflow-hidden"
                  >
                    <div className="flex items-center gap-3 pt-2.5">
                      <span className="shrink-0 text-xs text-muted-foreground">How sure</span>
                      <input
                        type="range"
                        min={50}
                        max={99}
                        value={threshold}
                        onChange={(e) => change(label.id, { threshold: Number(e.target.value) })}
                        aria-label={`How sure the provider must be about ${label.name}`}
                        className="h-1.5 flex-1 cursor-pointer accent-primary"
                      />
                      <motion.span
                        key={threshold}
                        initial={{ scale: 1.25 }}
                        animate={{ scale: 1 }}
                        transition={SPRING}
                        className="w-10 shrink-0 text-right text-xs font-bold tabular-nums"
                      >
                        {threshold}%
                      </motion.span>
                    </div>
                  </motion.div>
                )}
              </AnimatePresence>
            </motion.div>
          );
        })}
      </div>
    </Field>
  );
}

/** Where flags go, what blocked people are told and how long a time-out lasts: shown as the labels need them. */
function SmartActions({
  instanceKey,
  serverId,
  draft,
  setAction,
}: {
  instanceKey: string;
  serverId: string;
  draft: AutoModRule;
  setAction: (kind: AutoModActionKind, action: Partial<AutoModAction> | null) => void;
}) {
  const inst = useInstance(instanceKey);
  const textChannels = (inst?.channels[serverId] ?? []).filter((c) => c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT);
  const levels = draft.labels.map((l) => levelOf(l.level));
  const blocks = levels.some((l) => l >= AutoModLevel.BLOCK);
  const timesOut = levels.includes(AutoModLevel.TIME_OUT);
  const flags = levels.includes(AutoModLevel.FLAG);
  const alert = actionOf(draft, AutoModActionKind.ALERT);
  const block = actionOf(draft, AutoModActionKind.BLOCK);
  const timeOut = actionOf(draft, AutoModActionKind.TIME_OUT);
  const alertChannel = textChannels.find((c) => c.id === alert?.channelId);
  return (
    <div className="flex flex-col gap-4">
      <Field
        label="Alert channel"
        hint={flags ? "Flagged messages are posted here for your mods; blocked ones too." : "Blocked messages are posted here too, if you pick one."}
      >
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              type="button"
              className={cn(
                "group flex h-10 w-full items-center gap-2 rounded-xl border px-3 text-left text-sm transition hover:border-primary/40 data-[state=open]:border-primary/60",
                flags && !alertChannel && "border-amber-500/60",
              )}
            >
              <HashIcon className="size-4 text-muted-foreground" />
              <span className="flex-1 truncate font-bold">{alertChannel?.name ?? (flags ? "Pick a channel for flags" : "No alerts")}</span>
              <ChevronDownIcon className="size-4 text-muted-foreground transition-transform duration-300 group-data-[state=open]:rotate-180" />
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start" className="max-h-72 w-60 overflow-y-auto">
            {!flags && (
              <DropdownMenuItem onSelect={() => setAction(AutoModActionKind.ALERT, null)}>
                <XIcon /> No alerts
              </DropdownMenuItem>
            )}
            {textChannels.map((c) => (
              <DropdownMenuItem key={c.id} onSelect={() => setAction(AutoModActionKind.ALERT, { channelId: c.id })}>
                <HashIcon /> {c.name}
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
      </Field>
      <AnimatePresence initial={false}>
        {blocks && (
          <motion.div key="block" initial={{ height: 0, opacity: 0 }} animate={{ height: "auto", opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={SPRING} className="overflow-hidden">
            <Field label="What blocked people are told">
              <Input
                value={block?.message ?? ""}
                maxLength={150}
                placeholder="Optional: “That message breaks our rules.”"
                onChange={(e) => setAction(AutoModActionKind.BLOCK, e.target.value ? { message: e.target.value } : null)}
                className="h-10 rounded-xl"
              />
            </Field>
          </motion.div>
        )}
        {timesOut && (
          <motion.div key="time-out" initial={{ height: 0, opacity: 0 }} animate={{ height: "auto", opacity: 1 }} exit={{ height: 0, opacity: 0 }} transition={SPRING} className="overflow-hidden">
            <Field label="Time-out length">
              <Chips
                label="Time-out length"
                value={timeOut?.durationSeconds ?? 600}
                options={TIME_OUTS}
                onChange={(durationSeconds) => setAction(AutoModActionKind.TIME_OUT, { durationSeconds })}
              />
            </Field>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

function Pill({ children, onRemove }: { children: ReactNode; onRemove: () => void }) {
  return (
    <motion.span
      layout
      initial={{ opacity: 0, scale: 0.6 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.6 }}
      transition={SPRING}
      className="flex h-7 items-center gap-1.5 rounded-lg bg-muted pr-1 pl-2 text-xs font-bold"
    >
      {children}
      <button type="button" aria-label="Remove" onClick={onRemove} className="grid size-4 place-items-center rounded opacity-60 hover:opacity-100">
        <XIcon className="size-3" />
      </button>
    </motion.span>
  );
}
