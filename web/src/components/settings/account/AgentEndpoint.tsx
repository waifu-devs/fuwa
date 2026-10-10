import { CheckIcon, ChevronDownIcon, CopyIcon, EyeIcon, EyeOffIcon, InfinityIcon, KeyRoundIcon, ListChecksIcon, LoaderCircleIcon, PowerIcon, RefreshCwIcon, TriangleAlertIcon, WebhookIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useState, type FormEvent } from "react";
import type { AgentEndpoint } from "@/gen/fuwa/v1/agent_pb";
import { EventSchema } from "@/gen/fuwa/v1/types_pb";
import { getAgentEndpoint, resetAgentEndpointSecret, run, setAgentEndpoint } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { usePrivateField } from "@/components/Private";
import { Count, SwapText } from "@/components/motion";
import { Choice } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { type Key, T, useI18n } from "@/i18n/react";
import { type EventGroup, grouped, offered, readable, sameEvents, toggle, toggleAll } from "@/lib/agent-events";
import { ago, toDate } from "@/lib/format";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { usePrefs } from "@/lib/prefs";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Every event an endpoint can ask for: the fields of `Event.payload`, straight from the protocol. */
const EVENT_NAMES: string[] = EventSchema.oneofs.find((o) => o.name === "payload")?.fields.map((f) => f.name) ?? [];

const GROUP_LABEL: Record<EventGroup, Key> = {
  messages: "accountsettings.agents.endpoint.group.messages",
  interactions: "accountsettings.agents.endpoint.group.interactions",
  members: "accountsettings.agents.endpoint.group.members",
  reactions: "accountsettings.agents.endpoint.group.reactions",
  other: "accountsettings.agents.endpoint.group.other",
};

/** Friendly names for the events most agents want; the rest read as their protocol name. */
const EVENT_LABEL: Partial<Record<string, Key>> = {
  message_created: "accountsettings.agents.endpoint.event.messageCreated",
  message_updated: "accountsettings.agents.endpoint.event.messageUpdated",
  message_deleted: "accountsettings.agents.endpoint.event.messageDeleted",
  interaction_created: "accountsettings.agents.endpoint.event.interactionCreated",
  member_joined: "accountsettings.agents.endpoint.event.memberJoined",
  member_left: "accountsettings.agents.endpoint.event.memberLeft",
  member_updated: "accountsettings.agents.endpoint.event.memberUpdated",
  reaction_updated: "accountsettings.agents.endpoint.event.reactionUpdated",
  reactions_cleared: "accountsettings.agents.endpoint.event.reactionsCleared",
};

const SHAKE = { x: [0, -8, 6, -4, 0], transition: { duration: 0.35 } };

/**
 * An agent's endpoint (docs/agent-endpoints.md): the URL the instance posts
 * its events to, which ones, the secret they're signed with, and how
 * deliveries are going. Loaded when the agent's card opens.
 */
export function AgentEndpointSection({ instanceKey, agentId }: { instanceKey: string; agentId: string }) {
  const lang = useI18n();
  const { t } = lang;
  const [endpoint, setEndpoint] = useState<AgentEndpoint | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [url, setUrl] = useState("");
  const [some, setSome] = useState(false);
  const [chosen, setChosen] = useState<string[]>([]);
  const [busy, setBusy] = useState<null | "save" | "off" | "secret">(null);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);
  const [howTo, setHowTo] = useState(false);
  const shake = useAnimationControls();
  const privateField = usePrivateField();

  const take = (e: AgentEndpoint) => {
    // The secret comes only in the answer that made it: it stays here while the card is open, and never again.
    setEndpoint((was) => (!e.secret && was?.secret && was.secretHint === e.secretHint ? { ...e, secret: was.secret } : e));
    setUrl(e.url);
    setSome(e.events.length > 0);
    setChosen([...e.events]);
  };

  useEffect(() => {
    let live = true;
    run(getAgentEndpoint(instanceKey, agentId)).then(
      (e) => live && take(e),
      (err: FuwaError) => live && setLoadError(err.message),
    );
    return () => {
      live = false;
    };
  }, [instanceKey, agentId]);

  const groups = useMemo(() => grouped(offered(EVENT_NAMES, endpoint?.events)), [endpoint?.events]);
  const events = some ? chosen : [];
  const dirty = !!endpoint && (url.trim() !== endpoint.url || !sameEvents(events, endpoint.events));
  const needsOne = some && chosen.length === 0;
  const canSave = !!endpoint && !!url.trim() && !needsOne && (dirty || !!endpoint.disabledAt) && !busy;

  async function save(e?: FormEvent, off = false) {
    e?.preventDefault();
    if (!endpoint) return;
    if (!off && !canSave) {
      if (!url.trim() || needsOne) void shake.start(SHAKE);
      return;
    }
    setBusy(off ? "off" : "save");
    setError(null);
    try {
      take(await run(setAgentEndpoint(instanceKey, agentId, off ? "" : url, off ? endpoint.events : events)));
      if (off) toast(t("accountsettings.agents.endpoint.turnedOff"));
      else {
        setDone(true);
        setTimeout(() => setDone(false), 1600);
      }
    } catch (err) {
      setError((err as FuwaError).message);
      void shake.start(SHAKE);
    }
    setBusy(null);
  }

  async function newSecret() {
    setBusy("secret");
    try {
      take(await run(resetAgentEndpointSecret(instanceKey, agentId)));
    } catch (err) {
      toast((err as FuwaError).message);
    }
    setBusy(null);
  }

  const label = (name: string) => {
    const key = EVENT_LABEL[name];
    return key ? t(key) : readable(name);
  };

  return (
    <motion.section layout="position" transition={SPRING} className="flex flex-col gap-3 rounded-2xl border bg-background/40 p-3">
      <div className="flex items-start gap-2.5">
        <span className="grid size-8 shrink-0 place-items-center rounded-xl bg-violet-500/15 text-violet-500">
          <WebhookIcon className="size-4" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="text-sm font-extrabold">{t("accountsettings.agents.endpoint.title")}</p>
          <p className="text-xs text-muted-foreground">{t("accountsettings.agents.endpoint.intro")}</p>
        </div>
      </div>

      {loadError ? (
        <p className="rounded-xl bg-destructive/10 px-3 py-2 text-xs font-bold text-destructive">{t("accountsettings.agents.endpoint.loadFailed", { error: loadError })}</p>
      ) : !endpoint ? (
        <div className="flex flex-col gap-2">
          <div className="shimmer h-10 rounded-xl" />
          <div className="shimmer h-16 rounded-xl" />
        </div>
      ) : (
        <motion.div initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col gap-4">
          <Status endpoint={endpoint} />

          <form onSubmit={(e) => void save(e)} className="flex flex-col gap-1">
            <span className="text-xs font-bold text-muted-foreground uppercase">{t("accountsettings.agents.endpoint.url")}</span>
            <motion.div animate={shake} className="flex flex-wrap items-center gap-2">
              <Input
                type="url"
                value={url}
                spellCheck={false}
                placeholder="https://example.com/fuwa"
                aria-label={t("accountsettings.agents.endpoint.url")}
                onChange={(e) => {
                  setUrl(e.target.value);
                  setError(null);
                }}
                className={cn("h-9 min-w-0 flex-1 basis-56 rounded-xl font-mono text-xs", privateField, error && "border-destructive/60")}
              />
              <Button type="submit" size="sm" className="btn h-9 rounded-xl font-bold" disabled={!canSave && !done}>
                <AnimatePresence mode="popLayout" initial={false}>
                  <motion.span
                    key={done ? "done" : busy === "save" ? "checking" : "save"}
                    initial={{ y: 10, opacity: 0 }}
                    animate={{ y: 0, opacity: 1 }}
                    exit={{ y: -10, opacity: 0 }}
                    transition={SPRING}
                    className="flex items-center gap-1.5"
                  >
                    {done ? <CheckIcon strokeWidth={3} /> : busy === "save" ? <LoaderCircleIcon className="animate-spin" /> : <WebhookIcon />}
                    {done ? t("accountsettings.agents.endpoint.saved") : busy === "save" ? t("accountsettings.agents.endpoint.checking") : t("accountsettings.agents.endpoint.save")}
                  </motion.span>
                </AnimatePresence>
              </Button>
              <AnimatePresence mode="popLayout" initial={false}>
                {endpoint.url && (
                  <motion.span key="off" initial={{ opacity: 0, scale: 0.9 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.9 }} transition={SPRING}>
                    <Button type="button" variant="ghost" size="sm" className="h-9 rounded-xl" disabled={!!busy} onClick={() => void save(undefined, true)}>
                      {busy === "off" ? <LoaderCircleIcon className="animate-spin" /> : <PowerIcon />} {t("accountsettings.shared.turnOff")}
                    </Button>
                  </motion.span>
                )}
              </AnimatePresence>
            </motion.div>
            <AnimatePresence mode="popLayout" initial={false}>
              {error && (
                <motion.p key={error} {...SLIDE_IN} transition={SPRING} role="alert" className="mt-1 flex items-start gap-1.5 rounded-xl bg-destructive/10 px-3 py-2 text-xs font-bold text-destructive first-letter:uppercase">
                  <TriangleAlertIcon className="mt-px size-3.5 shrink-0" />
                  <span className="min-w-0 break-words">{error}</span>
                </motion.p>
              )}
            </AnimatePresence>
          </form>

          <div className="flex flex-col gap-2">
            <span className="text-xs font-bold text-muted-foreground uppercase">{t("accountsettings.agents.endpoint.events")}</span>
            <Choice
              value={some ? "some" : "all"}
              onChange={(v) => {
                setSome(v === "some");
                setError(null);
              }}
              options={[
                { value: "all", label: t("accountsettings.agents.endpoint.allEvents"), hint: t("accountsettings.agents.endpoint.allEventsHint"), icon: <InfinityIcon className="size-4" /> },
                { value: "some", label: t("accountsettings.agents.endpoint.chosenEvents"), hint: t("accountsettings.agents.endpoint.chosenEventsHint"), icon: <ListChecksIcon className="size-4" /> },
              ]}
            />
            <AnimatePresence mode="popLayout" initial={false}>
              {some && (
                <motion.div key="picker" {...SLIDE_IN} transition={SPRING} className="flex flex-col gap-3 pt-1">
                  {groups.map(({ group, names }) => (
                    <EventGroupPicker
                      key={group}
                      title={t(GROUP_LABEL[group])}
                      names={names}
                      chosen={chosen}
                      label={label}
                      onToggle={(name) => setChosen((c) => toggle(c, name))}
                      onToggleAll={() => setChosen((c) => toggleAll(c, names))}
                    />
                  ))}
                  <AnimatePresence initial={false}>
                    {needsOne && (
                      <motion.p {...SLIDE_IN} transition={SPRING} className="text-xs font-bold text-amber-600 dark:text-amber-400">
                        {t("accountsettings.agents.endpoint.pickOne")}
                      </motion.p>
                    )}
                  </AnimatePresence>
                </motion.div>
              )}
            </AnimatePresence>
          </div>

          <SecretField secret={endpoint.secret} hint={endpoint.secretHint} busy={busy === "secret"} disabled={!!busy} onReset={() => void newSecret()} />

          <HowTo open={howTo} onToggle={() => setHowTo((h) => !h)} />
        </motion.div>
      )}
    </motion.section>
  );
}

/** How deliveries are going, with a dot that says it at a glance. */
function Status({ endpoint: e }: { endpoint: AgentEndpoint }) {
  const lang = useI18n();
  const { t } = lang;
  const kind = e.disabledAt ? "disabled" : !e.url ? "off" : e.failingSince ? "failing" : e.lastDeliveredAt ? "delivered" : "waiting";
  const text =
    kind === "disabled"
      ? t("accountsettings.agents.endpoint.statusDisabled")
      : kind === "off"
        ? t("accountsettings.agents.endpoint.statusOff")
        : kind === "failing"
          ? t("accountsettings.agents.endpoint.statusFailing", { when: ago(lang, toDate(e.failingSince)), error: e.lastError })
          : kind === "delivered"
            ? t("accountsettings.agents.endpoint.statusDelivered", { when: ago(lang, toDate(e.lastDeliveredAt)) })
            : t("accountsettings.agents.endpoint.statusWaiting");
  const dot = { disabled: "bg-destructive", off: "bg-muted-foreground/50", failing: "bg-amber-500", delivered: "bg-emerald-500", waiting: "bg-sky-500" }[kind];
  return (
    <div
      className={cn(
        "flex items-start gap-2 rounded-xl px-3 py-2 text-xs font-bold transition-colors",
        kind === "disabled" ? "bg-destructive/10 text-destructive" : kind === "failing" ? "bg-amber-500/10 text-amber-700 dark:text-amber-300" : "bg-muted/60 text-muted-foreground",
      )}
    >
      <span className="relative mt-1 grid size-2 shrink-0 place-items-center">
        {(kind === "delivered" || kind === "failing") && (
          <motion.span
            aria-hidden
            animate={{ opacity: [0.6, 0, 0.6], scale: [1, 2.4, 1] }}
            transition={{ duration: 2, repeat: Infinity }}
            className={cn("absolute inset-0 rounded-full", dot)}
          />
        )}
        <motion.span key={kind} initial={{ scale: 0 }} animate={{ scale: 1 }} transition={SPRING} className={cn("relative size-2 rounded-full", dot)} />
      </span>
      <span className="min-w-0 flex-1 break-words">
        <SwapText>{text}</SwapText>
        {kind === "disabled" && e.lastError && <span className="block font-normal opacity-80">{t("accountsettings.agents.endpoint.lastError", { error: e.lastError })}</span>}
      </span>
    </div>
  );
}

/** One group of events: its name (which picks them all), and a chip per event. */
function EventGroupPicker({
  title,
  names,
  chosen,
  label,
  onToggle,
  onToggleAll,
}: {
  title: string;
  names: string[];
  chosen: string[];
  label: (name: string) => string;
  onToggle: (name: string) => void;
  onToggleAll: () => void;
}) {
  const picked = names.filter((n) => chosen.includes(n)).length;
  return (
    <div className="flex flex-col gap-1.5">
      <button type="button" onClick={onToggleAll} className="flex items-center gap-2 self-start text-xs font-bold text-muted-foreground transition-colors hover:text-foreground">
        <span
          className={cn(
            "grid size-4 place-items-center rounded-md border transition-colors",
            picked === names.length ? "border-primary bg-primary text-primary-foreground" : picked > 0 ? "border-primary/60 bg-primary/20" : "",
          )}
        >
          <AnimatePresence initial={false}>
            {picked === names.length && (
              <motion.span key="all" initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={SPRING} className="flex">
                <CheckIcon className="size-3" strokeWidth={3} />
              </motion.span>
            )}
          </AnimatePresence>
        </span>
        {title}
        <span className="tabular-nums opacity-70">
          <Count value={picked} />/{names.length}
        </span>
      </button>
      <div className="flex flex-wrap gap-1.5">
        {names.map((name) => {
          const on = chosen.includes(name);
          return (
            <motion.button
              key={name}
              type="button"
              aria-pressed={on}
              title={name}
              whileHover={{ y: -1 }}
              whileTap={{ scale: 0.94 }}
              transition={SPRING}
              onClick={() => onToggle(name)}
              className={cn(
                "relative flex items-center gap-1 rounded-full border px-2.5 py-1 text-xs font-bold transition-colors",
                on ? "border-primary/60 text-foreground" : "text-muted-foreground hover:border-primary/30",
              )}
            >
              <AnimatePresence initial={false}>
                {on && <motion.span key="bg" initial={{ opacity: 0, scale: 0.8 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.8 }} transition={SPRING} className="absolute inset-0 rounded-full bg-primary/15" />}
              </AnimatePresence>
              <AnimatePresence mode="popLayout" initial={false}>
                {on && (
                  <motion.span key="check" initial={{ scale: 0, width: 0 }} animate={{ scale: 1, width: "auto" }} exit={{ scale: 0, width: 0 }} transition={SPRING} className="relative flex">
                    <CheckIcon className="size-3" strokeWidth={3} />
                  </motion.span>
                )}
              </AnimatePresence>
              <span className="relative">{label(name)}</span>
            </motion.button>
          );
        })}
      </div>
    </div>
  );
}

/**
 * The signing secret: dotted out until shown, always blurred in streamer mode, with copy and a new one. The
 * instance gives it out once, when it's made; after that only its last four characters show.
 */
function SecretField({ secret, hint, busy, disabled, onReset }: { secret: string; hint: string; busy: boolean; disabled: boolean; onReset: () => void }) {
  const { t } = useI18n();
  const streaming = usePrefs((p) => p.streamer);
  const [shown, setShown] = useState(false);
  const [copied, setCopied] = useState(false);
  const [confirm, setConfirm] = useState(false);
  // A new secret, or streamer mode turning on, puts it away again.
  const shownFor = `${secret}|${streaming}`;
  const [lastShownFor, setLastShownFor] = useState(shownFor);
  if (lastShownFor !== shownFor) {
    setLastShownFor(shownFor);
    setShown(false);
  }
  const visible = shown && !streaming;

  function copy() {
    void navigator.clipboard?.writeText(secret).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      },
      () => toast(t("accountsettings.agents.endpoint.copyFailed")),
    );
  }

  return (
    <div className="flex flex-col gap-1">
      <span className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground uppercase">
        <KeyRoundIcon className="size-3.5" /> {t("accountsettings.agents.endpoint.secret")}
      </span>
      <div className="flex items-center gap-1 rounded-xl border bg-background/70 p-1 pl-3">
        <AnimatePresence mode="wait" initial={false}>
          <motion.code
            key={visible ? `shown-${secret}` : "hidden"}
            initial={{ opacity: 0, filter: "blur(4px)" }}
            animate={{ opacity: 1, filter: "blur(0px)" }}
            exit={{ opacity: 0, filter: "blur(4px)" }}
            transition={{ duration: 0.18 }}
            title={streaming ? t("accountsettings.agents.endpoint.streamerHidden") : undefined}
            className={cn("min-w-0 flex-1 font-mono text-xs transition-[filter] duration-300", visible ? "break-all" : "truncate", streaming && "blur-sm select-none")}
          >
            {visible ? secret : `whsec_${"•".repeat(24)}${secret ? "••••" : hint}`}
          </motion.code>
        </AnimatePresence>
        {secret && (
          <>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              className="size-8 shrink-0 rounded-lg"
              disabled={streaming}
              title={streaming ? t("accountsettings.agents.endpoint.streamerHidden") : undefined}
              aria-label={visible ? t("accountsettings.agents.endpoint.hideSecret") : t("accountsettings.agents.endpoint.showSecret")}
              onClick={() => setShown((s) => !s)}
            >
              {visible ? <EyeOffIcon /> : <EyeIcon />}
            </Button>
            <Button type="button" size="sm" variant="secondary" className="h-8 shrink-0 rounded-lg px-3 font-bold" onClick={copy}>
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span key={copied ? "done" : "copy"} initial={{ y: 12, opacity: 0 }} animate={{ y: 0, opacity: 1 }} exit={{ y: -12, opacity: 0 }} transition={SPRING} className="flex items-center gap-1.5">
                  {copied ? <CheckIcon strokeWidth={3} /> : <CopyIcon />} {copied ? t("accountsettings.shared.copied") : t("accountsettings.shared.copy")}
                </motion.span>
              </AnimatePresence>
            </Button>
          </>
        )}
      </div>
      <p className="text-xs text-muted-foreground">
        {secret ? t("accountsettings.agents.endpoint.secretOnce") : t("accountsettings.agents.endpoint.secretGone")}
      </p>
      <AnimatePresence mode="popLayout" initial={false}>
        {confirm ? (
          <motion.span key="confirm" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={SPRING} className="flex flex-wrap items-center gap-1">
            <span className="flex items-center gap-1 px-1 text-xs font-bold">
              <TriangleAlertIcon className="size-3.5 text-amber-500" />
              {t("accountsettings.agents.endpoint.newSecretWarning")}
            </span>
            <Button
              type="button"
              size="sm"
              variant="destructive"
              className="h-7 rounded-full px-3 text-xs font-bold"
              onClick={() => {
                setConfirm(false);
                onReset();
              }}
            >
              {t("accountsettings.agents.endpoint.newSecret")}
            </Button>
            <Button type="button" size="icon" variant="ghost" aria-label={t("accountsettings.agents.neverMind")} className="size-7 rounded-full" onClick={() => setConfirm(false)}>
              <XIcon />
            </Button>
          </motion.span>
        ) : (
          <motion.span key="action" initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING} className="flex">
            <Button type="button" variant="ghost" size="sm" className="h-7 rounded-xl text-xs" disabled={disabled} onClick={() => setConfirm(true)}>
              <RefreshCwIcon className={cn(busy && "animate-spin")} /> {t("accountsettings.agents.endpoint.newSecret")}
            </Button>
          </motion.span>
        )}
      </AnimatePresence>
    </div>
  );
}

/** What an endpoint has to do: answer the check, check signatures, reply in the body. */
function HowTo({ open, onToggle }: { open: boolean; onToggle: () => void }) {
  const { t } = useI18n();
  const code = (text: string) => <code className="rounded bg-muted px-1 py-px font-mono text-[0.7rem] text-foreground">{text}</code>;
  return (
    <div className="rounded-xl border">
      <button type="button" onClick={onToggle} aria-expanded={open} className="flex w-full items-center gap-2 px-3 py-2 text-left text-xs font-bold">
        <WebhookIcon className="size-3.5 text-primary" />
        <span className="flex-1">{t("accountsettings.agents.endpoint.howTo")}</span>
        <ChevronDownIcon className={cn("size-3.5 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
      </button>
      <AnimatePresence mode="popLayout" initial={false}>
        {open && (
          <motion.div key="howto" {...SLIDE_IN} transition={SPRING}>
            <ul className="flex list-disc flex-col gap-1.5 px-3 pb-3 pl-7 text-xs text-muted-foreground">
              <li>
                <T k="accountsettings.agents.endpoint.howToCheck" values={{ challenge: code("challenge"), answer: code('{"challenge": "…"}') }} />
              </li>
              <li>
                <T k="accountsettings.agents.endpoint.howToVerify" values={{ signature: code("webhook-signature") }} />
              </li>
              <li>
                <T k="accountsettings.agents.endpoint.howToReply" values={{ replies: code('{"replies": [{"interactionId": "…", "content": "…"}]}') }} />
              </li>
            </ul>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}
