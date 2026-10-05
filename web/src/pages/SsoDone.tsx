import { useNavigate } from "@tanstack/react-router";
import { ArrowRightIcon, BuildingIcon, CheckIcon, CloudOffIcon, LoaderCircleIcon, ShieldAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion, useReducedMotion } from "motion/react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import type { SsoIdentity } from "@/gen/fuwa/v1/sso_pb";
import { finishServerSso, finishSsoSignIn, run, ssoSignInInfo } from "@/fuwa/actions";
import { normalizeUrl } from "@/fuwa/saved";
import { FuwaMark } from "@/components/Icons";
import { Petals } from "@/components/Petals";
import { Private } from "@/components/Private";
import { Button } from "@/components/ui/button";
import type { Key } from "@/i18n/i18n";
import { T, useI18n } from "@/i18n/react";
import { DONE, takePendingSso } from "@/lib/sso";

const EASE = [0.22, 1, 0.36, 1] as const;

type Phase =
  | { kind: "finishing" }
  /** Signed in to the instance, or through a server's provider. */
  /** `name` fills the title's {name}, when it has one. */
  | { kind: "done"; title: Key; line: Key; name?: string }
  /** An admin's test: who the provider signed in, and nothing else happened. */
  | { kind: "tested"; identity: SsoIdentity | undefined; next: string | null; key: string }
  /** Another fuwa app started this sign-in; it goes back there if the person says so. */
  | { kind: "handoff"; origin: string; provider: string; place: string; fragment: string }
  /** `message` is what the server or provider said, if anything; `note` is ours, shown when it said nothing. */
  | { kind: "error"; title: Key; message?: string; note?: Key }
  | { kind: "cancelled" };

/**
 * Where an instance sends the browser once an identity provider answers, at
 * /auth/sso/done, with the answer in the fragment. A sign-in this tab started
 * finishes here: the instance's (a session, or an admin's test) or a
 * server's (recorded, then joined when that's what it was for). One another
 * fuwa app started goes back to that app once the person confirms it's
 * theirs, as with waifu.dev sign-ins.
 */
export function SsoDone() {
  const navigate = useNavigate();
  const { t } = useI18n();
  const [phase, setPhase] = useState<Phase>({ kind: "finishing" });
  const once = useRef(false);

  useEffect(() => {
    if (once.current) return;
    once.current = true;
    const answer = new URLSearchParams(window.location.hash.slice(1));
    const state = answer.get("state") ?? "";
    const code = answer.get("code") ?? "";
    const serverId = answer.get("server") ?? undefined;
    const problem = answer.get("error") ?? "";
    // The code works once, but it has no business in the address bar or the history.
    window.history.replaceState(null, "", DONE);

    const pending = state ? takePendingSso(state) : null;
    if (pending && (pending.serverId ?? "") === (serverId ?? "")) {
      if (problem || !code) {
        setPhase({ kind: "error", title: "connect.callback.didntSignIn", message: problem, note: "connect.callback.nothingBackSso" });
        return;
      }
      if (pending.serverId) {
        run(finishServerSso(pending, state, code)).then(
          ({ key, serverId, joined }) => {
            setPhase({
              kind: "done",
              title: joined ? "connect.callback.youreIn" : "connect.callback.signedIn",
              line: joined ? "connect.callback.toServer" : "connect.callback.takingYouBack",
            });
            window.setTimeout(() => {
              if (pending.next) navigate({ to: pending.next, replace: true });
              else navigate({ to: "/$instance/$server", params: { instance: key, server: serverId }, replace: true });
            }, 1400);
          },
          (err: Error) => setPhase({ kind: "error", title: "connect.callback.couldntSignIn", message: err.message }),
        );
        return;
      }
      run(finishSsoSignIn(pending, state, code)).then(
        ({ key, user, created, identity }) => {
          if (pending.test) {
            setPhase({ kind: "tested", identity, next: pending.next, key });
            return;
          }
          setPhase({
            kind: "done",
            title: created ? "connect.callback.welcome" : "connect.callback.welcomeBack",
            line: "connect.callback.takingYouIn",
            name: user?.displayName || user?.username || "",
          });
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
    const here = normalizeUrl(window.location.origin);
    run(ssoSignInInfo(here, state, serverId)).then(
      (info) => {
        if (info.origin === window.location.origin) {
          setPhase({ kind: "error", title: "connect.callback.elsewhere", note: "connect.callback.elsewhereNote" });
        } else if (problem || !code) {
          setPhase({ kind: "error", title: "connect.callback.didntSignIn", message: problem, note: "connect.callback.nothingBackSso" });
        } else {
          const fragment = new URLSearchParams({ ...(serverId ? { server: serverId } : {}), state, code });
          setPhase({ kind: "handoff", origin: info.origin, provider: info.provider, place: info.server, fragment: fragment.toString() });
        }
      },
      () => setPhase({ kind: "error", title: "connect.callback.ranOut", note: "connect.callback.ranOutNote" }),
    );
  }, [navigate]);

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
        data-testid="sso-done"
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
                <Title>{t(phase.title, { name: phase.name || t("connect.callback.you") })}</Title>
                <p className="flex items-center gap-2 text-sm text-muted-foreground">
                  <LoaderCircleIcon className="size-4 animate-spin" /> {t(phase.line)}
                </p>
              </>
            )}
            {phase.kind === "tested" && (
              <>
                <Check />
                <Title>{t("connect.callback.ssoWorks")}</Title>
                <p className="text-sm text-muted-foreground">{t("connect.callback.ssoWorksNote")}</p>
                <IdentityCard identity={phase.identity} />
                <Button
                  size="lg"
                  className="btn h-11 w-full rounded-xl font-bold"
                  onClick={() =>
                    phase.next
                      ? navigate({ to: phase.next, replace: true })
                      : navigate({ to: "/$instance", params: { instance: phase.key }, replace: true })
                  }
                >
                  {t("connect.callback.backToApp")}
                </Button>
              </>
            )}
            {phase.kind === "handoff" && (
              <>
                <Bridge still />
                <Title>{t("connect.callback.finishTitle")}</Title>
                <p className="text-sm text-muted-foreground">
                  <T
                    k={phase.place ? "connect.callback.ssoHandoffFor" : "connect.callback.ssoHandoff"}
                    values={{ provider: phase.provider || t("connect.provider.yourOrganization"), place: <b className="text-foreground">{phase.place}</b> }}
                  />
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
            {phase.kind === "cancelled" && (
              <>
                <Badge tone="muted">
                  <CheckIcon className="size-7" />
                </Badge>
                <Title>{t("connect.callback.cancelledTitle")}</Title>
                <p className="text-sm text-muted-foreground">{t("connect.callback.closeTab")}</p>
              </>
            )}
            {phase.kind === "error" && (
              <>
                <Badge tone="error">
                  <CloudOffIcon className="size-7" />
                </Badge>
                <Title>{t(phase.title)}</Title>
                <p className="text-sm text-muted-foreground first-letter:uppercase">{phase.message || (phase.note && t(phase.note))}</p>
                <Button size="lg" className="btn h-11 w-full rounded-xl font-bold" onClick={() => navigate({ to: "/", replace: true })}>
                  {t("connect.callback.backToFuwa")}
                </Button>
              </>
            )}
          </motion.div>
        </AnimatePresence>
      </motion.div>
    </div>
  );
}

/** Who the provider signed in, line by line as they arrive. */
export function IdentityCard({ identity }: { identity: SsoIdentity | undefined }) {
  const { t } = useI18n();
  const rows = [
    [t("connect.callback.identity.name"), identity?.name],
    [t("connect.callback.identity.email"), identity?.email],
    [t("connect.callback.identity.subject"), identity?.subject],
  ].filter((r): r is [string, string] => !!r[1]);
  return (
    <dl className="grid w-full gap-1.5 rounded-2xl border bg-muted/40 p-3 text-left text-sm" data-testid="sso-identity">
      {rows.map(([label, value], n) => (
        <motion.div
          key={label}
          initial={{ opacity: 0, x: -10 }}
          animate={{ opacity: 1, x: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 30, delay: 0.15 + n * 0.07 }}
          className="flex min-w-0 items-baseline justify-between gap-3"
        >
          <dt className="shrink-0 text-xs text-muted-foreground">{label}</dt>
          <dd className="min-w-0 truncate font-bold">
            <Private text={value} />
          </dd>
        </motion.div>
      ))}
    </dl>
  );
}

function Title({ children }: { children: ReactNode }) {
  return <h1 className="text-2xl font-extrabold tracking-tight">{children}</h1>;
}

function Badge({ tone, children }: { tone: "error" | "muted"; children: ReactNode }) {
  return (
    <motion.span
      initial={{ scale: 0, rotate: -20 }}
      animate={{ scale: 1, rotate: 0 }}
      transition={{ type: "spring", stiffness: 420, damping: 16 }}
      className={
        tone === "error"
          ? "grid size-16 place-items-center rounded-3xl bg-destructive/15 text-destructive"
          : "grid size-16 place-items-center rounded-3xl bg-muted text-muted-foreground"
      }
    >
      {children}
    </motion.span>
  );
}

/** The organization's building and fuwa's mark, with a key's dots travelling between them while it works. */
function Bridge({ still = false }: { still?: boolean }) {
  const reduce = useReducedMotion();
  const moving = !still && !reduce;
  return (
    <div className="flex items-center gap-3">
      <motion.span
        animate={moving ? { y: [0, -3, 0] } : { y: 0 }}
        transition={moving ? { repeat: Infinity, duration: 1.6, ease: "easeInOut" } : { duration: 0 }}
        className="grid size-14 place-items-center rounded-2xl bg-foreground text-primary shadow-lg"
      >
        <BuildingIcon className="size-7" />
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

/** A check that pops in with a ring rippling out behind it. */
function Check() {
  return (
    <span className="relative grid size-16 place-items-center">
      <motion.span
        aria-hidden
        className="absolute inset-0 rounded-full bg-emerald-500/30"
        initial={{ scale: 0.6, opacity: 0.8 }}
        animate={{ scale: 1.8, opacity: 0 }}
        transition={{ duration: 0.9, ease: "easeOut" }}
      />
      <motion.span
        initial={{ scale: 0 }}
        animate={{ scale: 1 }}
        transition={{ type: "spring", stiffness: 500, damping: 15 }}
        className="grid size-16 place-items-center rounded-full bg-emerald-500 text-white shadow-[0_12px_30px_-10px_rgb(16_185_129)]"
      >
        <motion.svg viewBox="0 0 24 24" className="size-8" fill="none" stroke="currentColor" strokeWidth={3} strokeLinecap="round" strokeLinejoin="round">
          <motion.path d="M5 12.5l4.5 4.5L19 7.5" initial={{ pathLength: 0 }} animate={{ pathLength: 1 }} transition={{ duration: 0.4, delay: 0.15 }} />
        </motion.svg>
      </motion.span>
    </span>
  );
}
