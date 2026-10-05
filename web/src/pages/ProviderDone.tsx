import { useNavigate } from "@tanstack/react-router";
import { ArrowRightIcon, CheckIcon, CloudOffIcon, LoaderCircleIcon, ShieldAlertIcon, UserPlusIcon } from "lucide-react";
import { AnimatePresence, m as motion, useReducedMotion } from "motion/react";
import { useEffect, useRef, useState, type FormEvent } from "react";
import type { NewProviderAccount } from "@/gen/fuwa/v1/auth_pb";
import { finishProviderLink, finishProviderSignIn, providerSignInInfo, run } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { normalizeUrl } from "@/fuwa/saved";
import { TwoFactorStep } from "@/components/Connect";
import { FuwaMark } from "@/components/Icons";
import { Petals } from "@/components/Petals";
import { Private } from "@/components/Private";
import { ProviderMark } from "@/components/ProviderMarks";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import type { Key } from "@/i18n/i18n";
import { T, useI18n } from "@/i18n/react";
import { takePendingSso, type PendingSso } from "@/lib/sso";
import { cn } from "@/lib/utils";
import { Cancelled, Check, Failed, Title } from "@/pages/CallbackParts";

const EASE = [0.22, 1, 0.36, 1] as const;
const DONE = "/auth/provider/done";

type Answer = { pending: PendingSso; state: string; code: string };

type Phase =
  | { kind: "finishing" }
  | { kind: "done"; title: Key; line: Key; name?: string }
  /** Someone new: which username, and whether to bring their name and picture. */
  | { kind: "choose"; answer: Answer; account: NewProviderAccount }
  | { kind: "twoStep"; url: string; ticket: string; next: string | null }
  /** Another fuwa app started this; it goes back there if the person says so. */
  | { kind: "handoff"; origin: string; provider: string; fragment: string }
  | { kind: "error"; title: Key; message?: string; note?: Key }
  | { kind: "cancelled" };

/**
 * Where an instance sends the browser once Google, X or Twitch answers, at
 * /auth/provider/done, with the answer in the fragment. A sign-in or link
 * this tab started finishes here; someone new picks a username first. One
 * another fuwa app started goes back there once the person confirms it's theirs.
 */
export function ProviderDone() {
  const navigate = useNavigate();
  const { t } = useI18n();
  const [phase, setPhase] = useState<Phase>({ kind: "finishing" });
  const [provider, setProvider] = useState("");
  const once = useRef(false);

  useEffect(() => {
    if (once.current) return;
    once.current = true;
    const answer = new URLSearchParams(window.location.hash.slice(1));
    const id = answer.get("provider") ?? "";
    const state = answer.get("state") ?? "";
    const code = answer.get("code") ?? "";
    const link = answer.get("link") === "1";
    const problem = answer.get("error") ?? "";
    setProvider(id);
    // The code works once, but it has no business in the address bar or the history.
    window.history.replaceState(null, "", DONE);

    const pending = state ? takePendingSso(state) : null;
    if (pending && pending.provider === id && !!pending.link === link) {
      if (problem || !code) {
        setPhase({ kind: "error", title: "connect.callback.didntSignIn", message: problem, note: "connect.callback.nothingBackSso" });
        return;
      }
      const leave = (to: () => void, ms: number) => window.setTimeout(to, ms);
      if (pending.link) {
        run(finishProviderLink(pending, state, code)).then(
          ({ key, method }) => {
            setPhase({ kind: "done", title: "connect.providerDone.linked", line: "connect.callback.takingYouBack", name: method?.name });
            leave(() => navigate({ to: pending.next ?? "/$instance", params: { instance: key }, replace: true }), 1400);
          },
          (err: Error) => setPhase({ kind: "error", title: "connect.providerDone.couldntLink", message: err.message }),
        );
        return;
      }
      run(finishProviderSignIn(pending, state, code)).then(
        (res) => {
          if ("newAccount" in res) {
            setPhase({ kind: "choose", answer: { pending, state, code }, account: res.newAccount! });
          } else if ("ticket" in res) {
            setPhase({ kind: "twoStep", url: pending.url, ticket: res.ticket!, next: pending.next });
          } else {
            setPhase({
              kind: "done",
              title: res.created ? "connect.callback.welcome" : "connect.callback.welcomeBack",
              line: "connect.callback.takingYouIn",
              name: res.user?.displayName || res.user?.username || "",
            });
            leave(() => {
              if (pending.next) navigate({ to: pending.next, replace: true });
              else navigate({ to: "/$instance", params: { instance: res.key }, replace: true });
            }, 1500);
          }
        },
        (err: Error) => setPhase({ kind: "error", title: "connect.callback.couldntSignIn", message: err.message }),
      );
      return;
    }

    if (!state || !id) {
      setPhase({ kind: "error", title: "connect.callback.nothingToFinish", note: "connect.callback.nothingWaiting" });
      return;
    }
    run(providerSignInInfo(normalizeUrl(window.location.origin), id, state)).then(
      (info) => {
        if (info.origin === window.location.origin) {
          setPhase({ kind: "error", title: "connect.callback.elsewhere", note: "connect.callback.elsewhereNote" });
        } else if (problem || !code) {
          setPhase({ kind: "error", title: "connect.callback.didntSignIn", message: problem, note: "connect.callback.nothingBackSso" });
        } else {
          const fragment = new URLSearchParams({ provider: id, state, code, ...(link ? { link: "1" } : {}) });
          setPhase({ kind: "handoff", origin: info.origin, provider: info.provider, fragment: fragment.toString() });
        }
      },
      () => setPhase({ kind: "error", title: "connect.callback.ranOut", note: "connect.callback.ranOutNote" }),
    );
  }, [navigate]);

  function signedIn(key: string, next: string | null, name: string, created: boolean) {
    setPhase({
      kind: "done",
      title: created ? "connect.callback.welcome" : "connect.callback.welcomeBack",
      line: "connect.callback.takingYouIn",
      name,
    });
    window.setTimeout(() => {
      if (next) navigate({ to: next, replace: true });
      else navigate({ to: "/$instance", params: { instance: key }, replace: true });
    }, 1500);
  }

  return (
    <div className="relative isolate grid min-h-full place-items-center overflow-hidden px-4 py-12">
      <div aria-hidden className="dot-grid fixed inset-0 -z-10" />
      <Petals />
      <motion.div
        layout
        initial={{ opacity: 0, y: 24, scale: 0.97 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        transition={{ duration: 0.6, ease: EASE }}
        className="w-full max-w-md rounded-3xl border bg-card/90 p-6 text-center shadow-[0_30px_80px_-40px_var(--primary)] backdrop-blur-xl sm:p-8"
        data-testid="provider-done"
        data-phase={phase.kind}
      >
        <AnimatePresence mode="wait" initial={false}>
          <motion.div
            key={phase.kind}
            initial={{ opacity: 0, y: 12 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -12 }}
            transition={{ duration: 0.3, ease: EASE }}
            className="flex flex-col items-center gap-4"
          >
            {phase.kind === "finishing" && (
              <>
                <Bridge provider={provider} />
                <Title>{t("connect.callback.signingIn")}</Title>
                <p className="text-sm text-muted-foreground">{t("connect.callback.checking")}</p>
              </>
            )}
            {phase.kind === "done" && (
              <>
                <Check />
                <Title>{t(phase.title, { name: phase.name || t("connect.callback.you") })}</Title>
                <p className="flex items-center gap-2 text-sm text-muted-foreground">
                  <LoaderCircleIcon className="size-4 animate-spin" /> {t(phase.line)}
                </p>
              </>
            )}
            {phase.kind === "choose" && (
              <NewAccount
                provider={provider}
                answer={phase.answer}
                account={phase.account}
                onSignedIn={(key, name) => signedIn(key, phase.answer.pending.next, name, true)}
                onTwoStep={(ticket) => setPhase({ kind: "twoStep", url: phase.answer.pending.url, ticket, next: phase.answer.pending.next })}
              />
            )}
            {phase.kind === "twoStep" && (
              <div className="w-full text-left">
                <TwoFactorStep
                  url={phase.url}
                  ticket={phase.ticket}
                  onBack={(problem) =>
                    setPhase({ kind: "error", title: "connect.callback.couldntSignIn", message: problem, note: "connect.callback.ranOutNote" })
                  }
                  onDone={(key) => signedIn(key, phase.next, "", false)}
                />
              </div>
            )}
            {phase.kind === "handoff" && (
              <>
                <Bridge provider={provider} still />
                <Title>{t("connect.callback.finishTitle")}</Title>
                <p className="text-sm text-muted-foreground">
                  <T k="connect.callback.ssoHandoff" values={{ provider: phase.provider }} />
                </p>
                <span className="max-w-full truncate rounded-xl border bg-muted/60 px-3 py-1.5 font-mono text-sm font-bold">
                  <Private text={phase.origin} />
                </span>
                <p className="flex items-start gap-2 rounded-2xl bg-amber-500/10 p-3 text-left text-xs text-amber-700 dark:text-amber-300">
                  <ShieldAlertIcon className="mt-0.5 size-4 shrink-0" />
                  {t("connect.callback.ssoWarning")}
                </p>
                <div className="flex w-full flex-col gap-2 sm:flex-row-reverse">
                  <Button
                    size="lg"
                    className="btn h-11 rounded-xl font-bold sm:flex-1"
                    onClick={() => window.location.replace(`${phase.origin}${DONE}#${phase.fragment}`)}
                  >
                    {t("common.continue")} <ArrowRightIcon className="transition group-hover:translate-x-0.5" />
                  </Button>
                  <Button size="lg" variant="ghost" className="h-11 rounded-xl font-bold sm:flex-1" onClick={() => setPhase({ kind: "cancelled" })}>
                    {t("common.cancel")}
                  </Button>
                </div>
              </>
            )}
            {phase.kind === "cancelled" && <Cancelled />}
            {phase.kind === "error" && <Failed title={phase.title} message={phase.message} note={phase.note} />}
          </motion.div>
        </AnimatePresence>
      </motion.div>
    </div>
  );
}

/**
 * Someone new picks a username (the provider's handle, free here, filled
 * in), and only brings their name and picture from the provider if they
 * turn that on: a profile here doesn't point at their account there unless
 * they choose it.
 */
function NewAccount({
  provider,
  answer,
  account,
  onSignedIn,
  onTwoStep,
}: {
  provider: string;
  answer: Answer;
  account: NewProviderAccount;
  onSignedIn: (key: string, name: string) => void;
  onTwoStep: (ticket: string) => void;
}) {
  const { t } = useI18n();
  // Until it's edited, the name is the one the instance suggested.
  const [edited, setUsername] = useState<string | null>(null);
  const username = edited ?? account.suggestedUsername;
  const [useProfile, setUseProfile] = useState(false);
  const [shake, setShake] = useState(0);
  const create = useAction(finishProviderSignIn);
  const brings = account.displayName || account.hasPicture;

  async function submit(e: FormEvent) {
    e.preventDefault();
    const res = await create.go(answer.pending, answer.state, answer.code, { username: username.trim().toLowerCase(), useProfile });
    if (!res) return setShake((n) => n + 1);
    if ("ticket" in res) return onTwoStep(res.ticket!);
    if ("key" in res) onSignedIn(res.key!, res.user?.displayName || res.user?.username || "");
  }

  return (
    <form onSubmit={submit} className="flex w-full flex-col gap-4 text-left" data-testid="provider-new-account">
      <div className="flex flex-col items-center gap-3 text-center">
        <motion.span
          initial={{ scale: 0, rotate: -20 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 16 }}
          className="grid size-16 place-items-center rounded-3xl bg-primary/15 text-primary"
        >
          <UserPlusIcon className="size-7" />
        </motion.span>
        <Title>{t("connect.providerDone.newTitle")}</Title>
        <p className="text-sm text-muted-foreground">{t("connect.providerDone.newNote", { provider: account.providerName })}</p>
      </div>
      <div key={shake} className={cn("flex flex-col gap-2", shake > 0 && "shake")}>
        <Label htmlFor="provider-username" className="font-bold">
          {t("connect.account.username")}
        </Label>
        <Input
          id="provider-username"
          autoFocus
          autoComplete="username"
          autoCapitalize="none"
          spellCheck={false}
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          className="h-11 rounded-xl"
        />
      </div>
      {brings && (
        <label
          htmlFor="provider-use-profile"
          className="flex cursor-pointer items-center gap-3 rounded-2xl border bg-muted/40 p-3 transition hover:bg-muted/70"
        >
          <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-foreground text-primary">
            <ProviderMark id={provider} className="size-4" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="block text-sm font-bold">{t("connect.providerDone.useProfile", { provider: account.providerName })}</span>
            <span className="block truncate text-xs text-muted-foreground">
              {account.displayName ? <Private text={account.displayName} /> : t("connect.providerDone.pictureOnly")}
            </span>
          </span>
          <Switch id="provider-use-profile" checked={useProfile} onCheckedChange={setUseProfile} />
        </label>
      )}
      <AnimatePresence>
        {create.error && (
          <motion.p
            initial={{ opacity: 0, y: -4 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -4 }}
            className="text-center text-sm text-destructive first-letter:uppercase"
          >
            {create.error}
          </motion.p>
        )}
      </AnimatePresence>
      <Button type="submit" size="lg" disabled={create.pending || !username.trim()} className="btn h-11 rounded-xl font-bold">
        {create.pending ? <LoaderCircleIcon className="animate-spin" /> : <ArrowRightIcon />}
        {t("connect.providerDone.create")}
      </Button>
    </form>
  );
}

/** The provider's mark and fuwa's, with dots travelling between them while it works. */
function Bridge({ provider, still = false }: { provider: string; still?: boolean }) {
  const reduce = useReducedMotion();
  const moving = !still && !reduce;
  return (
    <div className="flex items-center gap-3">
      <motion.span
        animate={moving ? { y: [0, -3, 0] } : { y: 0 }}
        transition={moving ? { repeat: Infinity, duration: 1.6, ease: "easeInOut" } : { duration: 0 }}
        className="grid size-14 place-items-center rounded-2xl bg-foreground text-primary shadow-lg"
      >
        <ProviderMark id={provider} className="size-7" />
      </motion.span>
      <span className="relative flex w-16 justify-between">
        {[0, 1, 2].map((n) => (
          <motion.span
            key={n}
            className="size-2 rounded-full bg-primary"
            animate={moving ? { opacity: [0.2, 1, 0.2], scale: [0.8, 1.2, 0.8] } : { opacity: 0.5 }}
            transition={moving ? { repeat: Infinity, duration: 1.2, delay: n * 0.2 } : { duration: 0 }}
          />
        ))}
      </span>
      <FuwaMark className="float size-14" />
    </div>
  );
}
