import { CheckIcon, ExternalLinkIcon, Flower2Icon, KeyRoundIcon, LogOutIcon, MonitorSmartphoneIcon, ShieldCheckIcon, UserPenIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useState, type FormEvent } from "react";
import { AccountKind, type User } from "@/gen/fuwa/v1/types_pb";
import { changePassword, forget, signOut } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { SPRING, SwapText } from "@/components/motion";
import { PASSWORD_MAX, PasswordInput, Row, Warn } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { issuerName, WAIFU_DEV_ISSUER } from "@/lib/linked";
import { cn } from "@/lib/utils";

/** Accounts made on the instance sign in with a password; linked ones sign in through waifu.dev. */
export const hasPassword = (user: User | undefined) => !!user && user.kind !== AccountKind.LINKED;

// ───────────────────────── Linked sign-in ─────────────────────────

/** For a linked account, which has no password here: where its sign-in lives instead. */
export function LinkedSignIn({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const issuer = inst?.node?.auth?.linkedIssuer || WAIFU_DEV_ISSUER;
  const name = issuerName(issuer);
  const where = inst?.node?.name ?? "this instance";
  const lines = [
    { icon: ShieldCheckIcon, text: `Your password and two-step sign-in live with ${name}, so there's nothing to set up here.` },
    { icon: MonitorSmartphoneIcon, text: `Signing out of ${name} doesn't sign you out of ${where}. Devices shows where you're signed in.` },
    { icon: UserPenIcon, text: `Your name and picture here started from ${name}, and they're yours to change.` },
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
          <p className="font-extrabold">Linked to {name}</p>
          <p className="text-sm text-muted-foreground">You sign in with your {name} account. There's no password here.</p>
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
          Your waifu.dev settings <ExternalLinkIcon className="size-3.5 transition group-hover:-translate-y-0.5 group-hover:translate-x-0.5" />
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

const STRENGTH = [
  { label: "Too short", tone: "bg-muted-foreground/40" },
  { label: "Weak", tone: "bg-destructive" },
  { label: "Fair", tone: "bg-amber-500" },
  { label: "Good", tone: "bg-emerald-500" },
  { label: "Strong", tone: "bg-emerald-500" },
];

export function Password({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [show, setShow] = useState(false);
  const [done, setDone] = useState(false);
  const change = useAction(changePassword);
  const shake = useAnimationControls();
  const where = inst?.node?.name ?? "this instance";

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
          <p className="text-lg font-extrabold">Password changed</p>
          <p className="max-w-sm text-sm text-muted-foreground">Every other device signed in to {where} was signed out. This one stays signed in.</p>
          <Button type="button" variant="outline" className="mt-2 rounded-xl" onClick={() => setDone(false)}>
            Done
          </Button>
        </motion.div>
      ) : (
        <motion.form key="form" onSubmit={submit} animate={shake} className="flex max-w-md flex-col">
          <Row id="current-password" label="Current password" htmlFor="current-password">
            <PasswordInput id="current-password" autoComplete="current-password" value={current} onChange={setCurrent} show={show} onShow={setShow} />
          </Row>
          <Row
            id="new-password"
            label="New password"
            htmlFor="new-password"
            hint={same ? <Warn>That's the password you have now.</Warn> : tooShort ? <Warn>At least {MIN} characters.</Warn> : `${MIN} to ${MAX} characters. Changing it signs out your other devices.`}
          >
            <PasswordInput id="new-password" autoComplete="new-password" value={next} onChange={setNext} show={show} onShow={setShow} />
            <div className="flex items-center gap-3" aria-live="polite">
              <div className="grid flex-1 grid-cols-4 gap-1">
                {[1, 2, 3, 4].map((n) => (
                  <span key={n} className="h-1.5 overflow-hidden rounded-full bg-muted">
                    <motion.span
                      className={cn("block h-full rounded-full", STRENGTH[level]!.tone)}
                      initial={false}
                      animate={{ width: level >= n ? "100%" : "0%" }}
                      transition={{ ...SPRING, delay: (n - 1) * 0.04 }}
                    />
                  </span>
                ))}
              </div>
              <span className="w-16 text-right text-xs font-bold text-muted-foreground">
                <SwapText>{next ? STRENGTH[level]!.label : " "}</SwapText>
              </span>
            </div>
          </Row>
          <Row id="confirm-password" label="Type it again" htmlFor="confirm-password" hint={mismatch ? <Warn>The two don't match yet.</Warn> : undefined}>
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
            <KeyRoundIcon /> {change.pending ? "Changing…" : "Change password"}
          </Button>
        </motion.form>
      )}
    </AnimatePresence>
  );
}

// ───────────────────────── Signing out ─────────────────────────

export function Session({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const leave = useAction(signOut);
  const drop = useAction(forget);
  const where = inst?.node?.name ?? "this instance";
  return (
    <div className="flex flex-col gap-3">
      <div data-setting="sign-out" className="flex flex-col gap-3 rounded-2xl border p-4 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-bold">Sign out of {where}</p>
          <p className="text-xs text-muted-foreground">It stays in your list, so signing back in is one step.</p>
        </div>
        <Button variant="outline" className="group rounded-xl" disabled={leave.pending} onClick={() => leave.go(instanceKey)}>
          <LogOutIcon className="transition-transform group-hover:translate-x-0.5" /> Sign out
        </Button>
      </div>
      <div data-setting="remove-instance" className="flex flex-col gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4 sm:flex-row sm:items-center">
        <div className="min-w-0 flex-1">
          <p className="text-sm font-bold text-destructive">Remove from this browser</p>
          <p className="text-xs text-muted-foreground">Signs out and takes {where} off your server list here. Your account stays on the instance.</p>
        </div>
        <Button variant="destructive" className="rounded-xl" disabled={drop.pending} onClick={() => drop.go(instanceKey)}>
          Remove
        </Button>
      </div>
    </div>
  );
}
