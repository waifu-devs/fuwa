import * as DialogPrimitive from "@radix-ui/react-dialog";
import { ArrowLeftIcon, ArrowRightIcon, CheckIcon, LoaderCircleIcon, MessageCircleHeartIcon, PartyPopperIcon, SendIcon, SparklesIcon } from "lucide-react";
import { AnimatePresence, LayoutGroup, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import {
  OnboardingStepKind,
  type Channel,
  type Emoji,
  type Onboarding,
  type OnboardingStep,
  type Role,
  type Server,
  type WelcomeScreen,
} from "@/gen/fuwa/v1/types_pb";
import { agreeToRules, finishOnboarding, getJoinForm, getOnboarding, getWelcomeScreen, run, sendMessage } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useInstance, useMyMember, useRoles } from "@/fuwa/hooks";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { BannerHero } from "@/components/join/Banner";
import { AgreeCheck, RulesList } from "@/components/join/Rules";
import { StartHere, suggestedChannels, type Suggested } from "@/components/join/StartHere";
import { InlineMarkdown } from "@/components/Markdown";
import { SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";
import { i18n } from "@/i18n/i18n";
import { T, useI18n } from "@/i18n/react";
import { accentVars, type BannerServer } from "@/lib/banner";
import { canSee, cssColor } from "@/lib/permissions";
import { reportTiming } from "@/lib/reports";
import { cn } from "@/lib/utils";

/** What the flow shows: the admin's steps the person can do, then "all set". */
type Shown = OnboardingStep | { kind: "done"; id: "done" };

/** Steps slide in from the side they're going to, and the old one slides out the other way. */
const SLIDE = {
  enter: (dir: number) => ({ opacity: 0, x: dir * 48, filter: "blur(4px)" }),
  center: { opacity: 1, x: 0, filter: "blur(0px)" },
  exit: (dir: number) => ({ opacity: 0, x: dir * -48, filter: "blur(4px)" }),
};

/**
 * The steps someone goes through, as they'll see them: rules only while they
 * still have to agree (added when the admin left them out), hello only where
 * they can see the channel.
 */
export function stepsFor(onboarding: Pick<Onboarding, "steps">, opts: { mustAgree: boolean; canSeeChannel: (id: string) => boolean }): OnboardingStep[] {
  const steps = onboarding.steps.filter((s) => {
    if (s.kind === OnboardingStepKind.RULES) return opts.mustAgree;
    if (s.kind === OnboardingStepKind.HELLO) return opts.canSeeChannel(s.channelId);
    return s.kind === OnboardingStepKind.PICK && s.options.length > 0;
  });
  if (opts.mustAgree && !steps.some((s) => s.kind === OnboardingStepKind.RULES)) {
    const hello = steps.findIndex((s) => s.kind === OnboardingStepKind.HELLO);
    const rules = { kind: OnboardingStepKind.RULES, id: "rules", title: i18n().t("join.onboarding.rulesTitle"), description: "", skippable: false } as OnboardingStep;
    steps.splice(hello === -1 ? steps.length : hello, 0, rules);
  }
  return steps;
}

/**
 * A server's onboarding, one step at a time, under its banner: progress
 * dots that stretch to the current step, picks that spring into place, the
 * rules to agree to, a hello to send, then where to go. `preview` draws it
 * without talking to the server, for the settings page.
 */
export function OnboardingFlow({
  server,
  steps,
  rules,
  channels,
  roles,
  emojis,
  welcome,
  preview = false,
  compact = false,
  onAgree,
  onHello,
  onFinish,
  onPick,
  onClose,
}: {
  server: BannerServer & { memberCount?: bigint };
  steps: OnboardingStep[];
  rules: string[];
  channels: Channel[];
  roles: Role[];
  emojis: Emoji[] | undefined;
  welcome: WelcomeScreen | null;
  preview?: boolean;
  compact?: boolean;
  /** Agrees to the rules; false when it didn't go through. */
  onAgree?: () => Promise<boolean>;
  onHello?: (channelId: string, text: string) => Promise<boolean>;
  onFinish?: (optionIds: string[]) => Promise<boolean>;
  onPick?: (channelId: string) => void;
  onClose?: () => void;
}) {
  const shown: Shown[] = useMemo(() => [...steps, { kind: "done", id: "done" }], [steps]);
  const [at, setAt] = useState(0);
  const [dir, setDir] = useState(1);
  const [picked, setPicked] = useState<string[]>([]);
  const [agreed, setAgreed] = useState(false);
  const [hello, setHello] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nudge = useAnimationControls();
  const { t } = useI18n();
  /** What the hello says until they change it, when the admin didn't write one. */
  const greeting = (s: OnboardingStep) => s.hello || t("join.onboarding.hello");
  const [started] = useState(() => performance.now());
  const index = Math.min(at, shown.length - 1);
  const step = shown[index]!;
  const last = index === shown.length - 2;

  useEffect(() => {
    if (index >= shown.length) setAt(shown.length - 1);
  }, [index, shown.length]);

  function move(to: number) {
    setError(null);
    setDir(to > index ? 1 : -1);
    setAt(to);
  }

  const optionIds = (s: OnboardingStep) => s.options.map((o) => o.id);
  const pickedIn = (s: OnboardingStep) => picked.filter((id) => optionIds(s).includes(id));

  function toggle(s: OnboardingStep, id: string) {
    setError(null);
    setPicked((list) => {
      const on = list.includes(id);
      if (s.multiple) return on ? list.filter((x) => x !== id) : [...list, id];
      const others = list.filter((x) => !optionIds(s).includes(x));
      return on ? others : [...others, id];
    });
  }

  function refuse(message: string) {
    setError(message);
    void nudge.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
  }

  async function finish() {
    if (!preview && onFinish && !(await onFinish(picked))) return false;
    reportTiming("onboarding.duration", performance.now() - started);
    return true;
  }

  async function next(skip = false) {
    if (step.kind === "done") return onClose?.();
    if (busy) return;
    if (!skip && step.kind === OnboardingStepKind.PICK && !step.skippable && pickedIn(step).length === 0) return refuse(t("join.onboarding.pickOne"));
    if (step.kind === OnboardingStepKind.RULES && !agreed) return refuse(t("join.onboarding.agreeFirst"));
    setBusy(true);
    try {
      if (step.kind === OnboardingStepKind.RULES && !preview && onAgree && !(await onAgree())) return;
      if (step.kind === OnboardingStepKind.HELLO && !skip && !preview && onHello) {
        const text = (hello[step.id] ?? greeting(step)).trim();
        if (text && !(await onHello(step.channelId, text))) return;
      }
      if (last && !(await finish())) return;
      move(index + 1);
    } catch (err) {
      setError((err as FuwaError).message ?? t("join.onboarding.failed"));
    } finally {
      setBusy(false);
    }
  }

  const suggested: Suggested[] = useMemo(() => {
    const fromPicks = steps
      .flatMap((s) => s.options)
      .filter((o) => picked.includes(o.id))
      .flatMap((o) => o.channelIds.map((channelId) => ({ channelId, description: t("join.onboarding.becausePicked", { option: o.label }), emoji: o.emoji })));
    const all = [...fromPicks, ...(welcome?.enabled ? welcome.channels : [])];
    const once = all.filter((w, n) => all.findIndex((x) => x.channelId === w.channelId) === n).slice(0, 6);
    return suggestedChannels({ channels: once as WelcomeScreen["channels"] }, channels);
  }, [steps, picked, welcome, channels, t]);

  const skippable = step.kind !== "done" && step.kind !== OnboardingStepKind.RULES && step.skippable;
  const empty = step.kind === OnboardingStepKind.PICK && pickedIn(step).length === 0;
  return (
    <div style={accentVars(server)} className="flex flex-col">
      <BannerHero
        server={server}
        compact={compact}
        bleed={!compact}
        eyebrow={step.kind === "done" ? t("join.onboarding.allSet") : t("join.onboarding.stepOf", { step: index + 1, total: shown.length - 1 })}
        badge={<SparklesIcon className="size-3.5" />}
      />
      <Dots count={shown.length - 1} at={index} onJump={preview ? (n) => move(n) : undefined} className={compact ? "px-4" : ""} />
      <motion.div layout transition={SPRING} className={cn("relative overflow-hidden", compact ? "px-4" : "")}>
        <AnimatePresence mode="popLayout" initial={false} custom={dir}>
          <motion.div key={step.id} custom={dir} variants={SLIDE} initial="enter" animate="center" exit="exit" transition={SPRING} className="flex flex-col gap-3 pt-3 pb-1">
            {step.kind === "done" ? (
              <Done server={server} suggested={suggested} emojis={emojis} onPick={onPick} />
            ) : (
              <>
                <div>
                  <h3 className="text-lg font-extrabold tracking-tight break-words">{step.title}</h3>
                  {step.description && (
                    <p className="text-sm break-words text-muted-foreground">
                      <InlineMarkdown>{step.description}</InlineMarkdown>
                    </p>
                  )}
                </div>
                {step.kind === OnboardingStepKind.PICK && (
                  <Picks step={step} picked={picked} roles={roles} emojis={emojis} onToggle={(id) => toggle(step, id)} compact={compact} />
                )}
                {step.kind === OnboardingStepKind.RULES && (
                  <>
                    <RulesList rules={rules} className="scroll-thin max-h-60 overflow-y-auto pr-1" />
                    <AgreeCheck checked={agreed} onChange={setAgreed}>
                      {t("join.rules.agree")}
                    </AgreeCheck>
                  </>
                )}
                {step.kind === OnboardingStepKind.HELLO && (
                  <Hello
                    channel={channels.find((c) => c.id === step.channelId)}
                    value={hello[step.id] ?? greeting(step)}
                    onChange={(text) => setHello((h) => ({ ...h, [step.id]: text }))}
                  />
                )}
              </>
            )}
          </motion.div>
        </AnimatePresence>
      </motion.div>
      <AnimatePresence initial={false}>
        {error && (
          <motion.p
            role="alert"
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            className={cn("text-sm font-bold text-destructive first-letter:uppercase", compact && "px-4")}
          >
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <motion.div animate={nudge} className={cn("mt-4 flex items-center gap-2", compact && "px-4 pb-4")}>
        <AnimatePresence initial={false} mode="popLayout">
          {index > 0 && step.kind !== "done" && (
            <motion.span key="back" initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.8 }} transition={SPRING}>
              <Button type="button" variant="ghost" size="icon" aria-label={t("common.back")} onClick={() => move(index - 1)} className="group size-10 rounded-xl">
                <ArrowLeftIcon className="transition-transform group-hover:-translate-x-0.5" />
              </Button>
            </motion.span>
          )}
        </AnimatePresence>
        <span className="flex-1" />
        {skippable && (
          <Button type="button" variant="ghost" disabled={busy} onClick={() => void next(true)} className="h-10 rounded-xl font-bold text-muted-foreground">
            {t("join.onboarding.skip")}
          </Button>
        )}
        <Button
          type="button"
          disabled={busy}
          onClick={() => void next()}
          style={{ background: "var(--accent-server)" }}
          className={cn("group h-10 min-w-32 rounded-xl font-bold text-white shadow-md transition-[filter,opacity] hover:brightness-110", empty && !step.skippable && "opacity-70")}
          data-burst={step.kind === "done" || last ? "" : undefined}
        >
          <AnimatePresence mode="wait" initial={false}>
            <motion.span
              key={busy ? "busy" : `${step.id}:${step.kind === OnboardingStepKind.HELLO}`}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={SPRING}
              className="flex items-center gap-2"
            >
              {busy ? <LoaderCircleIcon className="animate-spin" /> : null}
              {step.kind === "done" ? (
                <>
                  {t("join.onboarding.startExploring")} <PartyPopperIcon />
                </>
              ) : step.kind === OnboardingStepKind.HELLO ? (
                <>
                  {t("join.onboarding.send")} <SendIcon className="transition-transform group-hover:translate-x-0.5 group-hover:-translate-y-0.5" />
                </>
              ) : step.kind === OnboardingStepKind.RULES ? (
                <>
                  {t("join.onboarding.agree")} <CheckIcon />
                </>
              ) : (
                <>
                  {last ? t("join.onboarding.finish") : t("join.onboarding.next")} <ArrowRightIcon className="transition-transform group-hover:translate-x-0.5" />
                </>
              )}
            </motion.span>
          </AnimatePresence>
        </Button>
      </motion.div>
    </div>
  );
}

/** Where you are: a dot per step, the current one stretched into a pill that glides along. */
function Dots({ count, at, onJump, className }: { count: number; at: number; onJump?: (n: number) => void; className?: string }) {
  const { t } = useI18n();
  if (count <= 1) return null;
  return (
    <LayoutGroup>
      <div role="group" aria-label={t("join.onboarding.stepOf", { step: Math.min(at + 1, count), total: count })} className={cn("mt-3 flex items-center gap-1.5", className)}>
        {Array.from({ length: count }, (_, n) => (
          <button
            key={n}
            type="button"
            tabIndex={onJump ? 0 : -1}
            aria-label={t("join.onboarding.step", { step: n + 1 })}
            aria-current={n === at ? "step" : undefined}
            disabled={!onJump}
            onClick={() => onJump?.(n)}
            className={cn("relative h-1.5 overflow-hidden rounded-full bg-muted", n === at ? "w-7" : "w-1.5")}
          >
            {n < at && <motion.span layout className="absolute inset-0 rounded-full bg-[color-mix(in_srgb,var(--accent-server)_55%,transparent)]" />}
            {n === at && <motion.span layoutId="onboarding-dot" transition={SPRING} className="absolute inset-0 rounded-full bg-[var(--accent-server)]" />}
          </button>
        ))}
      </div>
    </LayoutGroup>
  );
}

/** A step's options as cards: picked ones fill with the server's color and get a tick that springs in. */
function Picks({
  step,
  picked,
  roles,
  emojis,
  onToggle,
  compact,
}: {
  step: OnboardingStep;
  picked: string[];
  roles: Role[];
  emojis: Emoji[] | undefined;
  onToggle: (id: string) => void;
  compact: boolean;
}) {
  return (
    <div role={step.multiple ? "group" : "radiogroup"} className={cn("grid gap-2", !compact && "sm:grid-cols-2")}>
      {step.options.map((o, n) => {
        const on = picked.includes(o.id);
        const given = o.roleIds.flatMap((id) => roles.filter((r) => r.id === id));
        return (
          <motion.button
            key={o.id || n}
            type="button"
            role={step.multiple ? "checkbox" : "radio"}
            aria-checked={on}
            onClick={() => onToggle(o.id)}
            initial={{ opacity: 0, y: 14, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            transition={{ ...SPRING, delay: Math.min(n, 8) * 0.04 }}
            whileTap={{ scale: 0.97 }}
            className={cn(
              "relative flex items-center gap-3 rounded-2xl border p-3 text-left transition-colors",
              on
                ? "border-[var(--accent-server)] bg-[color-mix(in_srgb,var(--accent-server)_12%,transparent)]"
                : "bg-background/60 hover:border-[color-mix(in_srgb,var(--accent-server)_45%,transparent)]",
            )}
          >
            <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-muted/70 text-xl">
              {o.emoji ? <EmojiGlyph value={o.emoji} emojis={emojis} className="size-6" /> : <SparklesIcon className="size-4 text-muted-foreground" />}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm font-bold">{o.label}</span>
              {o.description && <span className="block text-xs text-muted-foreground">{o.description}</span>}
              {given.length > 0 && (
                <span className="mt-1 flex flex-wrap gap-1">
                  {given.map((r) => (
                    <span key={r.id} className="rounded-full bg-muted px-1.5 py-px text-[0.65rem] font-bold" style={r.color ? { color: cssColor(r.color) } : undefined}>
                      @{r.name}
                    </span>
                  ))}
                </span>
              )}
            </span>
            <span className={cn("grid size-6 shrink-0 place-items-center border-2 transition-colors", step.multiple ? "rounded-lg" : "rounded-full", on ? "border-[var(--accent-server)] bg-[var(--accent-server)] text-white" : "border-muted-foreground/30")}>
              <AnimatePresence initial={false}>
                {on && (
                  <motion.span key="tick" initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={{ type: "spring", stiffness: 600, damping: 15 }}>
                    <CheckIcon className="size-3.5" strokeWidth={3.5} />
                  </motion.span>
                )}
              </AnimatePresence>
            </span>
          </motion.button>
        );
      })}
    </div>
  );
}

/** Saying hello: the channel it goes to and the message, ready to send. */
function Hello({ channel, value, onChange }: { channel: Channel | undefined; value: string; onChange: (text: string) => void }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-2">
      <p className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground">
        <MessageCircleHeartIcon className="size-4 text-[var(--accent-server)]" />{" "}
        {t("join.onboarding.goesTo", { channel: channel ? `#${channel.name}` : t("join.onboarding.aChannel") })}
      </p>
      <div className="relative">
        <Textarea value={value} maxLength={2000} rows={3} onChange={(e) => onChange(e.target.value)} aria-label={t("join.onboarding.yourHello")} className="rounded-2xl pr-4 text-base" />
        <motion.span
          aria-hidden
          animate={{ rotate: [0, 18, -8, 14, 0] }}
          transition={{ duration: 1.4, delay: 0.5, repeat: Infinity, repeatDelay: 2.6 }}
          className="pointer-events-none absolute -top-3 -right-2 text-2xl"
        >
          👋
        </motion.span>
      </div>
    </div>
  );
}

/** All set: a little celebration, and the channels to go to now. */
function Done({ server, suggested, emojis, onPick }: { server: BannerServer; suggested: Suggested[]; emojis: Emoji[] | undefined; onPick?: (channelId: string) => void }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-center gap-3">
        <motion.span
          initial={{ scale: 0, rotate: -90 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 12 }}
          className="relative grid size-12 shrink-0 place-items-center rounded-full bg-[color-mix(in_srgb,var(--accent-server)_18%,transparent)] text-[var(--accent-server)]"
        >
          <motion.span aria-hidden animate={{ scale: [1, 1.6], opacity: [0.5, 0] }} transition={{ duration: 1.6, repeat: 2, ease: "easeOut" }} className="absolute inset-0 rounded-full bg-[color-mix(in_srgb,var(--accent-server)_30%,transparent)]" />
          <PartyPopperIcon className="size-6" />
        </motion.span>
        <p className="text-sm text-muted-foreground">
          <T k="join.onboarding.haveFun" values={{ server: <b className="text-foreground">{server.name}</b> }} />
        </p>
      </div>
      <StartHere suggested={suggested} emojis={emojis} onPick={onPick} label={t("join.onboarding.goHereFirst")} />
    </div>
  );
}

/**
 * The onboarding for a member, in a dialog: loads the steps, the rules and
 * the welcome screen, and does what each step says (agree, say hello, hand
 * out roles) as they go.
 */
export function OnboardingDialog({
  instanceKey,
  server,
  open,
  onOpenChange,
  onPick,
}: {
  instanceKey: string;
  server: Server;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onPick: (channelId: string) => void;
}) {
  const inst = useInstance(instanceKey);
  const me = useMyMember(instanceKey, server.id);
  const access = useAccess(instanceKey, server.id);
  const roles = useRoles(instanceKey, server.id);
  const [loaded, setLoaded] = useState<{ onboarding: Onboarding; rules: string[]; welcome: WelcomeScreen | null } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // As it was when it opened, so agreeing partway doesn't take the rules step away under them.
  const [mustAgree] = useState(!!me?.pending && server.hasRules);
  const { t } = useI18n();

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    Promise.all([
      run(getOnboarding(instanceKey, server.id)),
      server.hasRules ? run(getJoinForm(instanceKey, server.id)).then((f) => f.rules) : Promise.resolve([] as string[]),
      server.hasWelcomeScreen ? run(getWelcomeScreen(instanceKey, server.id)).catch(() => null) : Promise.resolve(null),
    ]).then(
      ([onboarding, rules, welcome]) => !cancelled && setLoaded({ onboarding, rules, welcome }),
      (e: FuwaError) => !cancelled && setProblem(e.message),
    );
    return () => {
      cancelled = true;
    };
  }, [open, instanceKey, server.id, server.hasRules, server.hasWelcomeScreen]);

  const steps = useMemo(
    () => (loaded ? stepsFor(loaded.onboarding, { mustAgree, canSeeChannel: (id) => canSee(access, id) }) : []),
    [loaded, mustAgree, access],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent wide className="overflow-x-hidden pb-5">
        <DialogPrimitive.Title className="sr-only">{t("join.onboarding.title", { server: server.name })}</DialogPrimitive.Title>
        <DialogPrimitive.Description className="sr-only">{t("join.onboarding.description", { server: server.name })}</DialogPrimitive.Description>
        {problem ? (
          <p className="text-sm text-muted-foreground first-letter:uppercase">{problem}</p>
        ) : !loaded ? (
          <div className="-m-6 flex flex-col gap-3 pb-6">
            <div className="shimmer h-40 rounded-t-3xl" />
            <div className="flex flex-col gap-3 px-6">
              <div className="shimmer h-6 w-1/2 rounded-lg" />
              <div className="grid gap-2 sm:grid-cols-2">
                {[0, 1, 2, 3].map((n) => (
                  <div key={n} className="shimmer h-16 rounded-2xl" />
                ))}
              </div>
            </div>
          </div>
        ) : (
          <OnboardingFlow
            server={server}
            steps={steps}
            rules={loaded.rules}
            channels={inst?.channels[server.id] ?? []}
            roles={roles}
            emojis={inst?.emojis[server.id]}
            welcome={loaded.welcome}
            onAgree={async () => {
              await run(agreeToRules(instanceKey, server.id));
              return true;
            }}
            onHello={async (channelId, text) => {
              await run(sendMessage(instanceKey, server.id, channelId, text));
              return true;
            }}
            onFinish={async (ids) => {
              await run(finishOnboarding(instanceKey, server.id, ids));
              return true;
            }}
            onPick={(channel) => {
              onOpenChange(false);
              onPick(channel);
            }}
            onClose={() => onOpenChange(false)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
