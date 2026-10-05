import { useNavigate } from "@tanstack/react-router";
import { LoaderCircleIcon, PartyPopperIcon, ScrollTextIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls, useScroll } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { Permission, type Channel, type Emoji, type Server, type WelcomeScreen } from "@/gen/fuwa/v1/types_pb";
import { agreeToRules, getJoinForm, getWelcomeScreen, run } from "@/fuwa/actions";
import { useAccess, useAction, useInstance, useMyMember } from "@/fuwa/hooks";
import { BannerHero } from "@/components/join/Banner";
import { AgreeCheck, RulesList } from "@/components/join/Rules";
import { StartHere, suggestedChannels } from "@/components/join/StartHere";
import { InlineMarkdown } from "@/components/Markdown";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import * as DialogPrimitive from "@radix-ui/react-dialog";
import { accentVars, type BannerServer } from "@/lib/banner";
import { toDate } from "@/lib/format";
import { has } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { lazyComponent } from "@/components/lazy";
import { useI18n } from "@/i18n/react";

/** Onboarding opens once per server at most, so its steps come in their own file. */
const OnboardingDialog = lazyComponent(() => import("@/components/join/Onboarding").then((m) => m.OnboardingDialog));

/**
 * A server's welcome screen: its banner with its icon and name over it, how
 * many are in it, a few words, and the channels it suggests. The live view
 * and the settings preview both draw this.
 */
export function WelcomeCard({
  server,
  screen,
  channels,
  emojis,
  onPick,
  bleed = false,
  compact = false,
  wide = false,
  className,
  scrollY,
}: {
  server: BannerServer & { memberCount?: bigint };
  screen: WelcomeScreen;
  channels: Channel[];
  emojis: Emoji[] | undefined;
  onPick?: (channelId: string) => void;
  /** Out to a dialog's edges. */
  bleed?: boolean;
  compact?: boolean;
  wide?: boolean;
  className?: string;
  scrollY?: Parameters<typeof BannerHero>[0]["scrollY"];
}) {
  const suggested = suggestedChannels(screen, channels);
  const { t } = useI18n();
  return (
    <div style={accentVars(server)} className={cn("flex flex-col gap-4", className)}>
      <BannerHero server={server} eyebrow={t("join.welcome.eyebrow")} bleed={bleed} compact={compact} scrollY={scrollY} badge={<span className="text-sm">👋</span>}>
        {screen.description.trim() && (
          <motion.p
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ ...SPRING, delay: 0.18 }}
            className="mt-1 text-sm break-words text-muted-foreground"
          >
            <InlineMarkdown>{screen.description}</InlineMarkdown>
          </motion.p>
        )}
      </BannerHero>
      <div className={cn(compact ? "px-4" : bleed ? "" : "px-6")}>
        <StartHere suggested={suggested} emojis={emojis} onPick={onPick} wide={wide} />
      </div>
    </div>
  );
}

const SEEN = (instanceKey: string, serverId: string) => `fuwa.welcomed.${instanceKey}.${serverId}`;
/** New members are people who joined in the last week. */
const NEW_FOR = 7 * 86_400_000;

function seen(instanceKey: string, serverId: string) {
  try {
    return localStorage.getItem(SEEN(instanceKey, serverId)) !== null;
  } catch {
    return true;
  }
}

function markSeen(instanceKey: string, serverId: string) {
  try {
    localStorage.setItem(SEEN(instanceKey, serverId), "1");
  } catch {
    // Without storage it shows again next time, which is harmless.
  }
}

/**
 * Greets new members, once. With onboarding turned on they go through its
 * steps (which end on the welcome screen); otherwise they get the welcome
 * screen, with the rules to agree to when they haven't yet. People who can
 * change these never get them unasked, and anyone can open them again from
 * the server menu.
 */
export function WelcomeGate({
  instanceKey,
  server,
  open: asked,
  onOpenChange,
}: {
  instanceKey: string;
  server: Server;
  /** Opened from the server menu. */
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const inst = useInstance(instanceKey);
  const me = useMyMember(instanceKey, server.id);
  const access = useAccess(instanceKey, server.id);
  const navigate = useNavigate();
  const [screen, setScreen] = useState<WelcomeScreen | null>(null);
  const [greeting, setGreeting] = useState(false);
  // Closed partway: it comes back next time, not straight away.
  const [dismissed, setDismissed] = useState(false);
  const channels = inst?.channels[server.id] ?? [];

  const fresh = !!me && !has(access, Permission.MANAGE_SERVER) && Date.now() - toDate(me.joinedAt).getTime() < NEW_FOR;
  const due = server.hasOnboarding && (asked || (fresh && !me?.onboardedAt && !dismissed));
  // Once it opens it stays until it's closed: finishing sets onboardedAt
  // before its last step (where to start) has been seen.
  const [started, setStarted] = useState(false);
  if (due && !started) setStarted(true);
  const onboarding = due || started;
  // Onboarding ends with the welcome screen's channels, so it isn't shown again after.
  const newcomer = fresh && !seen(instanceKey, server.id) && !(server.hasOnboarding && me?.onboardedAt);
  const wanted = !onboarding && (asked || (server.hasWelcomeScreen && newcomer));

  useEffect(() => {
    if (!wanted) return;
    let cancelled = false;
    run(getWelcomeScreen(instanceKey, server.id)).then(
      (s) => {
        if (cancelled) return;
        setScreen(s);
        if (!asked) {
          markSeen(instanceKey, server.id);
          if (s.enabled) setGreeting(true);
        }
      },
      () => {},
    );
    return () => {
      cancelled = true;
    };
  }, [wanted, asked, instanceKey, server.id]);

  function go(channel: string) {
    void navigate({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: server.id, channel } });
  }

  if (onboarding) {
    return (
      <OnboardingDialog
        instanceKey={instanceKey}
        server={server}
        open
        onOpenChange={(o) => {
          if (!o) {
            setStarted(false);
            setDismissed(true);
            markSeen(instanceKey, server.id);
            onOpenChange(false);
          }
        }}
        onPick={go}
      />
    );
  }

  const open = (asked || greeting) && !!screen?.enabled;
  function close() {
    setGreeting(false);
    onOpenChange(false);
  }
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent wide className="overflow-x-hidden pb-5">
        {screen && (
          <WelcomeBody
            instanceKey={instanceKey}
            server={server}
            screen={screen}
            channels={channels}
            emojis={inst?.emojis[server.id]}
            agree={!!me?.pending && server.hasRules}
            onPick={(channel) => {
              close();
              go(channel);
            }}
            onClose={close}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}

/** The welcome screen in its dialog: scrolls under the banner, with the rules to agree to at the end when there are. */
function WelcomeBody({
  instanceKey,
  server,
  screen,
  channels,
  emojis,
  agree,
  onPick,
  onClose,
}: {
  instanceKey: string;
  server: Server;
  screen: WelcomeScreen;
  channels: Channel[];
  emojis: Emoji[] | undefined;
  agree: boolean;
  onPick: (channelId: string) => void;
  onClose: () => void;
}) {
  const scroller = useRef<HTMLDivElement>(null);
  const { scrollY } = useScroll({ container: scroller });
  const [agreeing] = useState(agree);
  const { t } = useI18n();
  return (
    <div ref={scroller} className="scroll-thin -m-6 max-h-[92svh] overflow-y-auto p-6 pb-5">
      <DialogPrimitive.Title className="sr-only">{t("join.welcome.title", { server: server.name })}</DialogPrimitive.Title>
      <DialogPrimitive.Description className="sr-only">{t("join.welcome.description", { server: server.name })}</DialogPrimitive.Description>
      <WelcomeCard server={server} screen={screen} channels={channels} emojis={emojis} onPick={agreeing ? undefined : onPick} bleed wide scrollY={scrollY} />
      {agreeing ? (
        <AgreeRules instanceKey={instanceKey} server={server} onDone={onClose} />
      ) : (
        <button type="button" onClick={onClose} className="mx-auto mt-5 block w-fit text-sm font-bold text-muted-foreground transition hover:text-foreground">
          {t("join.welcome.lookAround")}
        </button>
      )}
    </div>
  );
}

/** The server's rules and "I agree", for a newcomer who hasn't agreed yet. */
function AgreeRules({ instanceKey, server, onDone }: { instanceKey: string; server: Server; onDone: () => void }) {
  const [rules, setRules] = useState<string[] | null>(null);
  const [checked, setChecked] = useState(false);
  const accept = useAction(agreeToRules);
  const nudge = useAnimationControls();
  const { t } = useI18n();
  useEffect(() => {
    let cancelled = false;
    run(getJoinForm(instanceKey, server.id)).then(
      (form) => !cancelled && setRules(form.rules),
      () => !cancelled && setRules([]),
    );
    return () => {
      cancelled = true;
    };
  }, [instanceKey, server.id]);

  async function submit() {
    if (!checked) {
      void nudge.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
      return;
    }
    if ((await accept.go(instanceKey, server.id)) === undefined) return;
    toast(t("join.rules.welcomeToast", { server: server.name }));
    onDone();
  }

  return (
    <motion.section initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={{ ...SPRING, delay: 0.3 }} className="mt-5 flex flex-col gap-3">
      <p className="flex items-center gap-1.5 text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">
        <ScrollTextIcon className="size-3.5" /> {t("join.welcome.beforeYouTalk")}
      </p>
      {rules === null ? (
        <div className="flex flex-col gap-2">
          {[80, 60].map((w) => (
            <div key={w} className="shimmer h-12 rounded-2xl" style={{ width: `${w + 20}%` }} />
          ))}
        </div>
      ) : (
        <RulesList rules={rules} className="scroll-thin max-h-56 overflow-y-auto pr-1" />
      )}
      <motion.div animate={nudge} className="flex flex-col gap-3">
        <AgreeCheck checked={checked} onChange={setChecked}>
          {t("join.rules.agree")}
        </AgreeCheck>
        {accept.error && <p className="text-sm text-destructive first-letter:uppercase">{accept.error}</p>}
        <Button size="lg" onClick={() => void submit()} disabled={accept.pending} className={cn("h-11 rounded-xl font-bold transition-opacity", checked ? "btn" : "opacity-60")}>
          <AnimatePresence mode="wait" initial={false}>
            <motion.span
              key={accept.pending ? "busy" : "agree"}
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -6 }}
              transition={SPRING}
              className="flex items-center gap-2"
            >
              {accept.pending ? <LoaderCircleIcon className="animate-spin" /> : <PartyPopperIcon />}
              {t("join.rules.agreeAndTalk")}
            </motion.span>
          </AnimatePresence>
        </Button>
      </motion.div>
    </motion.section>
  );
}
