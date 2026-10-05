import { CheckIcon, ExternalLinkIcon, Flower2Icon, KeyRoundIcon, LogOutIcon, MonitorSmartphoneIcon, ShieldCheckIcon, UserPenIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useState, type FormEvent } from "react";
import { AccountKind, type User } from "@/gen/fuwa/v1/types_pb";
import { changePassword, forget, signOut } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { SPRING, SwapText } from "@/components/motion";
import { PASSWORD_MAX, PasswordInput, Row, Warn } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { type Key, useI18n } from "@/i18n/react";
import { issuerName, WAIFU_DEV_ISSUER } from "@/lib/linked";
import { cn } from "@/lib/utils";

/** Accounts made on the instance sign in with a password; linked ones sign in through waifu.dev, and agents with a token. */
export const hasPassword = (user: User | undefined) => !!user && user.kind === AccountKind.LOCAL;

// ───────────────────────── Linked sign-in ─────────────────────────

/** For a linked account, which has no password here: where its sign-in lives instead. */
export function LinkedSignIn({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const issuer = inst?.node?.auth?.linkedIssuer || WAIFU_DEV_ISSUER;
  const name = issuerName(issuer);
  const where = inst?.node?.name ?? t("settings.nav.thisInstance");
  const lines = [
    { icon: ShieldCheckIcon, text: t("accountsettings.linked.passwordLives", { issuer: name }) },
    { icon: MonitorSmartphoneIcon, text: t("accountsettings.linked.signOut", { issuer: name, instance: where }) },
    { icon: UserPenIcon, text: t("accountsettings.linked.picture", { issuer: name }) },
  ];
  return (
    <div className="flex flex-col gap-5">
      <motion.div
        initial={{ opacity: 0, y: 12 }}
        animate={{ opacity: 1, y: 0 }}
        transition={SPRING}
        className="group flex items-center gap-4 rounded-2xl border bg-card p-4"
      >
        <span className="grid size-12 shrink-0 place-items-center rounded-2xl bg-foreground text-primary shadow-lg">
          <Flower2Icon className="size-6 transition-transform duration-700 group-hover:rotate-[144deg]" />
        </span>
        <div className="min-w-0">
          <p className="font-extrabold">{t("accountsettings.linked.title", { issuer: name })}</p>
          <p className="text-sm text-muted-foreground">{t("accountsettings.linked.hint", { issuer: name })}</p>
        </div>
      </motion.div>
      <ul className="flex flex-col gap-2.5">
        {lines.map((line, n) => (
          <motion.li
            key={line.text}
            initial={{ opacity: 0, x: -8 }}
            animate={{ opacity: 1, x: 0 }}
            transition={{ ...SPRING, delay: 0.08 + n * 0.05 }}
            className="flex items-start gap-2.5 text-sm text-muted-foreground"
          >
            <line.icon className="mt-0.5 size-4 shrink-0 text-primary" /> {line.text}
          </motion.li>
        ))}
      </ul>
      {issuer === WAIFU_DEV_ISSUER && (
        <a
          href="https://www.waifu.dev/settings"
          target="_blank"
          rel="noreferrer"
          className="group inline-flex items-center gap-1.5 self-start text-sm font-bold text-primary underline-offset-4 hover:underline"
        >
          {t("accountsettings.linked.settings")} <ExternalLinkIcon className="size-3.5 transition group-hover:-translate-y-0.5 group-hover:translate-x-0.5" />
        </a>
      )}
    </div>
  );
}

// ───────────────────────── Password ─────────────────────────

const MIN = 8;
const MAX = PASSWORD_MAX;

/** A rough read of a password's strength, from 0 to 4, for the meter. */
function strength(password: string) {
  if (password.length < MIN) return 0;
  const kinds = [/[a-z]/, /[A-Z]/, /\d/, /[^A-Za-z0-9]/].filter((r) => r.test(password)).length;
  const long = password.length >= 16 ? 2 : password.length >= 12 ? 1 : 0;
  return Math.min(4, Math.max(1, kinds - 1 + long));
}

const STRENGTH: { label: Key; tone: string }[] = [
  { label: "accountsettings.password.tooShort", tone: "bg-muted-foreground/40" },
  { label: "accountsettings.password.weak", tone: "bg-destructive" },
  { label: "accountsettings.password.fair", tone: "bg-amber-500" },
  { label: "accountsettings.password.good", tone: "bg-emerald-500" },
  { label: "accountsettings.password.strong", tone: "bg-emerald-500" },
];

export function Password({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [show, setShow] = useState(false);
  const [done, setDone] = useState(false);
  const change = useAction(changePassword);
  const shake = useAnimationControls();
  const where = inst?.node?.name ?? t("settings.nav.thisInstance");

  const tooShort = next.length > 0 && next.length < MIN;
  const mismatch = confirm.length > 0 && confirm !== next;
  const same = next.length > 0 && next === current;
  const ready = current.length > 0 && next.length >= MIN && next.length <= MAX && confirm === next && !same;
  const level = strength(next);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!ready) {
      void shake.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
      return;
    }
    const ok = await change.go(instanceKey, current, next);
    if (!ok) {
      void shake.start({ x: [0, -8, 8, -5, 5, 0], transition: { duration: 0.4 } });
      return;
    }
    setCurrent("");
    setNext("");
    setConfirm("");
    setDone(true);
  }

  return (
    <AnimatePresence mode="wait" initial={false}>
      {done ? (
        <motion.div
          key="done"
          initial={{ opacity: 0, scale: 0.95 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.95 }}
          transition={SPRING}
          className="flex flex-col items-center gap-3 rounded-3xl border bg-card px-6 py-10 text-center"
        >
          <motion.span
            initial={{ scale: 0, rotate: -45 }}
            animate={{ scale: 1, rotate: 0 }}
            transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.1 }}
            className="grid size-14 place-items-center rounded-full bg-emerald-500 text-white shadow-lg shadow-emerald-500/30"
          >
            <CheckIcon className="size-7" strokeWidth={3} />
          </motion.span>
          <p className="text-lg font-extrabold">{t("accountsettings.password.changed")}</p>
          <p className="max-w-sm text-sm text-muted-foreground">{t("accountsettings.password.changedHint", { instance: where })}</p>
          <Button type="button" variant="outline" className="mt-2 rounded-xl" onClick={() => setDone(false)}>
            {t("accountsettings.shared.done")}
          </Button>
        </motion.div>
      ) : (
        <motion.form key="form" onSubmit={submit} animate={shake} className="flex max-w-md flex-col">
          <Row id="current-password" label={t("accountsettings.password.current")} htmlFor="current-password">
            <PasswordInput id="current-password" autoComplete="current-password" value={current} onChange={setCurrent} show={show} onShow={setShow} />
          </Row>
          <Row
            id="new-password"
            label={t("accountsettings.password.new")}
            htmlFor="new-password"
            hint={
              same ? (
                <Warn>{t("accountsettings.password.same")}</Warn>
              ) : tooShort ? (
                <Warn>{t("accountsettings.password.min", { count: MIN })}</Warn>
              ) : (
                t("accountsettings.password.range", { min: MIN, max: MAX })
              )
            }
          >
            <PasswordInput id="new-password" autoComplete="new-password" value={next} onChange={setNext} show={show} onShow={setShow} />
            <div className="flex items-center gap-3" aria-live="polite">
              <div className="grid flex-1 grid-cols-4 gap-1">
                {[1, 2, 3, 4].map((n) => (
                  <span key={n} className="h-1.5 overflow-hidden rounded-full bg-muted">
                    <motion.span
                      className={cn("block h-full rounded-full", STRENGTH[level]!.tone)}
                      initial={false}
                      animate={{ x: level >= n ? "0%" : "-100%" }}
                      transition={{ ...SPRING, delay: (n - 1) * 0.04 }}
                    />
                  </span>
                ))}
              </div>
              <span className="w-16 text-right text-xs font-bold text-muted-foreground">
                <SwapText>{next ? t(STRENGTH[level]!.label) : " "}</SwapText>
              </span>
            </div>
          </Row>
          <Row id="confirm-password" label={t("accountsettings.password.again")} htmlFor="confirm-password" hint={mismatch ? <Warn>{t("accountsettings.password.mismatch")}</Warn> : undefined}>
            <div className="relative">
              <PasswordInput id="confirm-password" autoComplete="new-password" value={confirm} onChange={setConfirm} show={show} onShow={setShow} />
              <AnimatePresence>
                {confirm && confirm === next && (
                  <motion.span
                    initial={{ scale: 0, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    exit={{ scale: 0, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 600, damping: 18 }}
                    className="pointer-events-none absolute top-1/2 right-12 grid size-5 -translate-y-1/2 place-items-center rounded-full bg-emerald-500 text-white"
                  >
                    <CheckIcon className="size-3.5" strokeWidth={3} />
                  </motion.span>
                )}
              </AnimatePresence>
            </div>
          </Row>
          <AnimatePresence initial={false}>
            {change.error && (
              <motion.p
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                className="overflow-hidden pb-3 text-sm font-bold text-destructive first-letter:uppercase"
              >
                {change.error}
              </motion.p>
            )}
          </AnimatePresence>
          <Button type="submit" className="btn self-start rounded-xl px-5 font-bold" disabled={change.pending || !ready}>
            <KeyRoundIcon /> {change.pending ? t("accountsettings.password.changing") : t("accountsettings.password.change")}
          </Button>
        </motion.form>
      )}
    </AnimatePresence>
  );
}

// ───────────────────────── Signing out ─────────────────────────

export function Session({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const leave = useAction(signOut);
  const drop = useAction(forget);
  const where = inst?.node?.name ?? t("settings.nav.thisInstance");
  return (
    <div className="flex flex-col gap-3">
      <div data-setting="sign-out" className="flex flex-col gap-3 rounded-2xl border p-4 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-bold">{t("accountsettings.session.signOutOf", { instance: where })}</p>
          <p className="text-xs text-muted-foreground">{t("accountsettings.session.signOutHint")}</p>
        </div>
        <Button variant="outline" className="group rounded-xl" disabled={leave.pending} onClick={() => leave.go(instanceKey)}>
          <LogOutIcon className="transition-transform group-hover:translate-x-0.5" /> {t("accountsettings.shared.signOut")}
        </Button>
      </div>
      <div data-setting="remove-instance" className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-bold text-destructive">{t("accountsettings.session.remove")}</p>
          <p className="text-xs text-muted-foreground">{t("accountsettings.session.removeHint", { instance: where })}</p>
        </div>
        <Button variant="destructive" className="rounded-xl" disabled={drop.pending} onClick={() => drop.go(instanceKey)}>
          {t("accountsettings.session.removeButton")}
        </Button>
      </div>
    </div>
  );
}
