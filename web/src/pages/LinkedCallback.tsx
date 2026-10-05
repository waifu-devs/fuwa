import { useNavigate } from "@tanstack/react-router";
import { ArrowRightIcon, Flower2Icon, LoaderCircleIcon, ShieldAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion, useReducedMotion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import { finishLinkedSignIn, linkedSignInOrigin, probe, run } from "@/fuwa/actions";
import { FuwaMark } from "@/components/Icons";
import { Petals } from "@/components/Petals";
import { Private } from "@/components/Private";
import { Button } from "@/components/ui/button";
import { Cancelled, Check, Failed, Title } from "@/pages/CallbackParts";
import type { Key } from "@/i18n/i18n";
import { T, useI18n } from "@/i18n/react";
import { CALLBACK, issuerName, takePending } from "@/lib/linked";

const EASE = [0.22, 1, 0.36, 1] as const;

type Phase =
  /** Trading the code for a session on the instance this tab started with. */
  | { kind: "finishing" }
  /** `name` is empty when the account has none to show. */
  | { kind: "done"; name: string; created: boolean }
  /** Another fuwa app started this sign-in; it goes back there if the person says so. */
  | { kind: "handoff"; origin: string; code: string; state: string }
  /** `message` is what the server or provider said, if anything; `note` is ours, shown when it said nothing. */
  | { kind: "error"; title: Key; message?: string; note?: Key }
  | { kind: "cancelled" };

/**
 * Where waifu.dev sends the browser after signing in, at /auth/waifu/callback.
 * A sign-in this tab started finishes here. One another fuwa app started (on
 * another address, signing in to this instance) is handed back to that app,
 * once the person confirms it's theirs: a sign-in link someone else made
 * would otherwise give them the account.
 */
export function LinkedCallback() {
  const navigate = useNavigate();
  const { t } = useI18n();
  const [phase, setPhase] = useState<Phase>({ kind: "finishing" });
  const [here, setHere] = useState<{ name: string; issuer: string } | null>(null);
  const once = useRef(false);

  useEffect(() => {
    if (once.current) return;
    once.current = true;
    const query = new URLSearchParams(window.location.search);
    const fragment = new URLSearchParams(window.location.hash.slice(1));
    const read = (name: string) => query.get(name) ?? fragment.get(name) ?? "";
    const state = read("state");
    const code = read("code");
    const problem = read("error_description") || read("error");
    // The code works once, but it has no business in the address bar or the history.
    window.history.replaceState(null, "", CALLBACK);

    const pending = state ? takePending(state) : null;
    if (pending) {
      if (problem || !code) {
        setPhase({ kind: "error", title: "connect.callback.didntSignIn", message: problem, note: "connect.callback.nothingBackLinked" });
        return;
      }
      run(finishLinkedSignIn(pending, state, code)).then(
        ({ key, user, created }) => {
          setPhase({ kind: "done", name: user?.displayName || user?.username || "", created });
          window.setTimeout(() => {
            if (pending.next) navigate({ to: pending.next, replace: true });
            else navigate({ to: "/$instance", params: { instance: key }, replace: true });
          }, 1500);
        },
        (err: Error) => setPhase({ kind: "error", title: "connect.callback.couldntSignIn", message: err.message }),
      );
      return;
    }

    if (!state) {
      setPhase({ kind: "error", title: "connect.callback.nothingToFinish", note: "connect.callback.nothingWaiting" });
      return;
    }
    run(probe(window.location.origin)).then(
      (found) => setHere({ name: found.node.name, issuer: issuerName(found.node.auth?.linkedIssuer) }),
      () => {},
    );
    run(linkedSignInOrigin(window.location.origin, state)).then(
      (origin) => {
        if (origin === window.location.origin) {
          setPhase({ kind: "error", title: "connect.callback.elsewhere", note: "connect.callback.elsewhereNote" });
        } else if (problem || !code) {
          setPhase({ kind: "error", title: "connect.callback.didntSignIn", message: problem, note: "connect.callback.nothingBackLinked" });
        } else {
          setPhase({ kind: "handoff", origin, code, state });
        }
      },
      () => setPhase({ kind: "error", title: "connect.callback.ranOut", note: "connect.callback.ranOutNote" }),
    );
  }, [navigate]);

  function handOff(p: Extract<Phase, { kind: "handoff" }>) {
    const fragment = new URLSearchParams({ code: p.code, state: p.state });
    window.location.replace(`${p.origin}${CALLBACK}#${fragment}`);
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
        data-testid="linked-callback"
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
                <Bridge />
                <Title>{t("connect.callback.signingIn")}</Title>
                <p className="text-sm text-muted-foreground">{t("connect.callback.checking")}</p>
              </>
            )}
            {phase.kind === "done" && (
              <>
                <Check />
                <Title>{t(phase.created ? "connect.callback.welcome" : "connect.callback.welcomeBack", { name: phase.name || t("connect.callback.you") })}</Title>
                <p className="flex items-center gap-2 text-sm text-muted-foreground">
                  <LoaderCircleIcon className="size-4 animate-spin" /> {t("connect.callback.takingYouIn")}
                </p>
              </>
            )}
            {phase.kind === "handoff" && (
              <>
                <Bridge still />
                <Title>{t("connect.callback.finishTitle")}</Title>
                <p className="text-sm text-muted-foreground">
                  <T
                    k="connect.callback.linkedHandoff"
                    values={{ issuer: here?.issuer ?? "waifu.dev", server: <b className="text-foreground">{here?.name ?? t("connect.callback.thisServer")}</b> }}
                  />
                </p>
                <span className="max-w-full truncate rounded-xl border bg-muted/60 px-3 py-1.5 font-mono text-sm font-bold">
                  <Private text={phase.origin} />
                </span>
                <p className="flex items-start gap-2 rounded-2xl bg-amber-500/10 p-3 text-left text-xs text-amber-700 dark:text-amber-300">
                  <ShieldAlertIcon className="mt-0.5 size-4 shrink-0" />
                  {t("connect.callback.linkedWarning")}
                </p>
                <div className="flex w-full flex-col gap-2 sm:flex-row-reverse">
                  <Button size="lg" className="btn h-11 rounded-xl font-bold sm:flex-1" onClick={() => handOff(phase)}>
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

/** The waifu.dev flower and fuwa's mark, with petals drifting between them while it works. */
function Bridge({ still = false }: { still?: boolean }) {
  const reduce = useReducedMotion();
  const moving = !still && !reduce;
  return (
    <div className="flex items-center gap-3">
      <motion.span
        animate={moving ? { rotate: 360 } : { rotate: 0 }}
        transition={moving ? { repeat: Infinity, duration: 6, ease: "linear" } : { duration: 0 }}
        className="grid size-14 place-items-center rounded-2xl bg-foreground text-primary shadow-lg"
      >
        <Flower2Icon className="size-7" />
      </motion.span>
      <span className="relative flex w-16 justify-between">
        {[0, 1, 2].map((n) => (
          <motion.span
            key={n}
            className="size-2 rounded-full bg-primary"
            animate={moving ? { opacity: [0.2, 1, 0.2], y: [0, -4, 0] } : { opacity: 0.5 }}
            transition={moving ? { repeat: Infinity, duration: 1.2, delay: n * 0.2 } : { duration: 0 }}
          />
        ))}
      </span>
      <FuwaMark className="float size-14" />
    </div>
  );
}
