import {
  CheckIcon,
  CopyIcon,
  DownloadIcon,
  EyeIcon,
  KeyRoundIcon,
  RefreshCwIcon,
  ShieldCheckIcon,
  ShieldIcon,
  ShieldOffIcon,
  SmartphoneIcon,
} from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { encode } from "uqr";
import { disableTwoFactor, enableTwoFactor, getTwoFactor, regenerateBackupCodes, run, setUpTwoFactor } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction, useInstance } from "@/fuwa/hooks";
import { CodeInput } from "@/components/CodeInput";
import { Count, SPRING } from "@/components/motion";
import { PasswordInput, Row, useShake } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { usePrefs } from "@/lib/prefs";
import { copy, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

type Status = { enabled: boolean; backupCodesLeft: number };

const STEPS = ["Confirm it's you", "Scan", "Enter a code", "Save backup codes"];

/**
 * Two-step sign-in for an account with a password: after the password, a
 * code from an authenticator app (or one of ten backup codes). Setting it up
 * walks through four steps; once on, backup codes can be replaced and the
 * whole thing turned off with the password and a code.
 */
export function Security({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [setup, setSetup] = useState(false);
  const where = inst?.node?.name ?? "this instance";

  const load = useCallback(() => {
    run(getTwoFactor(instanceKey)).then(
      (s) => {
        setStatus({ enabled: s.enabled, backupCodesLeft: s.backupCodesLeft });
        setError(null);
      },
      (err: FuwaError) => setError(err.message),
    );
  }, [instanceKey]);
  useEffect(load, [load]);

  if (!status) {
    return error ? (
      <div className="flex flex-col items-start gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4">
        <p className="text-sm font-bold text-destructive first-letter:uppercase">{error}</p>
        <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={load}>
          Try again
        </Button>
      </div>
    ) : (
      <div className="shimmer h-40 rounded-3xl" />
    );
  }

  return (
    <AnimatePresence mode="wait" initial={false}>
      {setup ? (
        <motion.div key="setup" initial={{ opacity: 0, x: 24 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -24 }} transition={SPRING}>
          <SetUp
            instanceKey={instanceKey}
            where={where}
            onCancel={() => setSetup(false)}
            onDone={(left) => {
              setStatus({ enabled: true, backupCodesLeft: left });
              setSetup(false);
            }}
          />
        </motion.div>
      ) : status.enabled ? (
        <motion.div key="on" initial={{ opacity: 0, scale: 0.97 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.97 }} transition={SPRING}>
          <TurnedOn instanceKey={instanceKey} where={where} status={status} onChange={setStatus} />
        </motion.div>
      ) : (
        <motion.div key="off" initial={{ opacity: 0, scale: 0.97 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.97 }} transition={SPRING}>
          <div data-setting="two-step" className="relative overflow-hidden rounded-3xl border bg-card p-6">
            <div className="pointer-events-none absolute -top-16 -right-16 size-48 rounded-full bg-primary/10 blur-2xl" />
            <div className="relative flex flex-col items-start gap-4 sm:flex-row sm:items-center">
              <motion.span
                initial={{ rotate: -12, scale: 0.7 }}
                animate={{ rotate: 0, scale: 1 }}
                transition={{ type: "spring", stiffness: 400, damping: 14 }}
                className="grid size-14 shrink-0 place-items-center rounded-2xl bg-muted text-muted-foreground"
              >
                <ShieldIcon className="size-7" />
              </motion.span>
              <div className="min-w-0 flex-1">
                <p className="text-lg font-extrabold">Two-step sign-in is off</p>
                <p className="text-sm text-muted-foreground">
                  Ask for a code from an authenticator app after your password, so a leaked password alone can't get into your account on {where}.
                </p>
              </div>
              <Button type="button" className="btn shrink-0 rounded-xl px-5 font-bold" onClick={() => setSetup(true)}>
                Turn on
              </Button>
            </div>
          </div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}

// ───────────────────────── Setting it up ─────────────────────────

function SetUp({ instanceKey, where, onCancel, onDone }: { instanceKey: string; where: string; onCancel: () => void; onDone: (left: number) => void }) {
  const [step, setStep] = useState(0);
  const [password, setPassword] = useState("");
  const [show, setShow] = useState(false);
  const [secret, setSecret] = useState<{ secret: string; uri: string } | null>(null);
  const [codes, setCodes] = useState<string[]>([]);
  const [shakeCode, setShakeCode] = useState(0);
  const start = useAction(setUpTwoFactor);
  const enable = useAction(enableTwoFactor);
  const [shake, doShake] = useShake();

  async function confirm(e: FormEvent) {
    e.preventDefault();
    if (!password) return doShake();
    const res = await start.go(instanceKey, password);
    if (!res) return doShake();
    setSecret({ secret: res.secret, uri: res.uri });
    setPassword("");
    setStep(1);
  }

  async function verify(code: string) {
    const res = await enable.go(instanceKey, code);
    if (!res) return setShakeCode((n) => n + 1);
    setCodes(res);
    setStep(3);
  }

  return (
    <div className="flex flex-col gap-6">
      <Steps step={step} />
      <AnimatePresence mode="wait" initial={false}>
        {step === 0 && (
          <motion.form key="password" onSubmit={confirm} initial={{ opacity: 0, x: 24 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -24 }} transition={SPRING} className="max-w-md">
            <motion.div animate={shake}>
              <Row label="Your password" htmlFor="two-step-password" hint={start.error ? <span className="font-bold text-destructive first-letter:uppercase">{start.error}</span> : `The one you sign in to ${where} with.`}>
                <PasswordInput id="two-step-password" autoComplete="current-password" value={password} onChange={setPassword} show={show} onShow={setShow} />
              </Row>
            </motion.div>
            <div className="flex gap-2">
              <Button type="button" variant="ghost" className="rounded-xl" onClick={onCancel}>
                Cancel
              </Button>
              <Button type="submit" className="btn rounded-xl px-5 font-bold" disabled={start.pending}>
                {start.pending ? "Checking…" : "Continue"}
              </Button>
            </div>
          </motion.form>
        )}
        {step === 1 && secret && (
          <motion.div key="scan" initial={{ opacity: 0, x: 24 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -24 }} transition={SPRING} className="flex flex-col gap-5">
            <div className="flex flex-col items-start gap-6 md:flex-row">
              <Secret uri={secret.uri}>
                <QrCode text={secret.uri} />
              </Secret>
              <div className="flex min-w-0 flex-1 flex-col gap-4">
                <div>
                  <p className="font-extrabold">Scan this with your authenticator app</p>
                  <p className="text-sm text-muted-foreground">Any app that makes six-digit codes works, like 1Password, Bitwarden, Google Authenticator or Aegis.</p>
                </div>
                <div>
                  <p className="mb-1.5 text-xs font-bold text-muted-foreground">Or type this key in</p>
                  <SecretKey secret={secret.secret} />
                </div>
                <a
                  href={secret.uri}
                  className="inline-flex items-center gap-2 self-start rounded-xl border px-3 py-2 text-sm font-bold transition hover:border-primary/40 hover:bg-muted md:hidden"
                >
                  <SmartphoneIcon className="size-4" /> Open in an app on this phone
                </a>
              </div>
            </div>
            <div className="flex gap-2">
              <Button type="button" variant="ghost" className="rounded-xl" onClick={onCancel}>
                Cancel
              </Button>
              <Button type="button" className="btn rounded-xl px-5 font-bold" onClick={() => setStep(2)}>
                I've added it
              </Button>
            </div>
          </motion.div>
        )}
        {step === 2 && (
          <motion.div key="code" initial={{ opacity: 0, x: 24 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -24 }} transition={SPRING} className="flex flex-col gap-4">
            <div>
              <p className="font-extrabold">Type the code your app shows</p>
              <p className="text-sm text-muted-foreground">It changes every 30 seconds; any current one works.</p>
            </div>
            <CodeInput id="two-step-enable" label="Code from your app" onComplete={(code) => void verify(code)} disabled={enable.pending} shake={shakeCode} />
            <AnimatePresence initial={false}>
              {enable.error && (
                <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="overflow-hidden text-sm font-bold text-destructive first-letter:uppercase">
                  {enable.error}
                </motion.p>
              )}
            </AnimatePresence>
            <div className="flex gap-2">
              <Button type="button" variant="ghost" className="rounded-xl" onClick={() => setStep(1)}>
                Back
              </Button>
            </div>
          </motion.div>
        )}
        {step === 3 && (
          <motion.div key="codes" initial={{ opacity: 0, x: 24 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -24 }} transition={SPRING} className="flex flex-col gap-5">
            <div className="flex items-center gap-3">
              <motion.span
                initial={{ scale: 0, rotate: -45 }}
                animate={{ scale: 1, rotate: 0 }}
                transition={{ type: "spring", stiffness: 500, damping: 14 }}
                className="grid size-11 shrink-0 place-items-center rounded-full bg-emerald-500 text-white shadow-lg shadow-emerald-500/30"
              >
                <ShieldCheckIcon className="size-6" />
              </motion.span>
              <div>
                <p className="font-extrabold">Two-step sign-in is on</p>
                <p className="text-sm text-muted-foreground">Keep these somewhere safe. Each one signs you in once if your phone isn't around.</p>
              </div>
            </div>
            <BackupCodes codes={codes} where={where} />
            <div>
              <Button type="button" className="btn rounded-xl px-5 font-bold" onClick={() => onDone(codes.length)}>
                I've saved them
              </Button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** Where setting up has got to, as a row of dots joined by a filling line. */
function Steps({ step }: { step: number }) {
  return (
    <ol className="flex items-center gap-2" aria-label="Steps">
      {STEPS.map((label, n) => (
        <li key={label} className={cn("flex items-center gap-2", n < STEPS.length - 1 && "flex-1")} aria-current={n === step ? "step" : undefined}>
          <span className="flex items-center gap-2">
            <motion.span
              animate={{ scale: n === step ? 1.1 : 1 }}
              transition={SPRING}
              className={cn(
                "grid size-7 shrink-0 place-items-center rounded-full text-xs font-extrabold transition-colors duration-300",
                n < step ? "bg-primary text-primary-foreground" : n === step ? "bg-primary/15 text-primary ring-2 ring-primary" : "bg-muted text-muted-foreground",
              )}
            >
              <AnimatePresence mode="popLayout" initial={false}>
                {n < step ? (
                  <motion.span key="done" initial={{ scale: 0, rotate: -60 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 600, damping: 18 }}>
                    <CheckIcon className="size-3.5" strokeWidth={3} />
                  </motion.span>
                ) : (
                  <motion.span key="n">{n + 1}</motion.span>
                )}
              </AnimatePresence>
            </motion.span>
            <span className={cn("hidden text-xs font-bold whitespace-nowrap lg:inline", n === step ? "text-foreground" : "text-muted-foreground")}>{label}</span>
          </span>
          {n < STEPS.length - 1 && (
            <span className="h-0.5 min-w-4 flex-1 overflow-hidden rounded-full bg-muted">
              <motion.span className="block h-full rounded-full bg-primary" initial={false} animate={{ width: n < step ? "100%" : "0%" }} transition={{ duration: 0.45, ease: [0.22, 1, 0.36, 1] }} />
            </span>
          )}
        </li>
      ))}
    </ol>
  );
}

/** Something only you should see; behind a veil while streamer mode is on. */
function Secret({ uri, children }: { uri: string; children: ReactNode }) {
  const streaming = usePrefs((p) => p.streamer);
  const [revealed, setRevealed] = useState(false);
  const hidden = streaming && !revealed;
  useEffect(() => setRevealed(false), [uri, streaming]);
  return (
    <div className="relative shrink-0">
      <div className={cn("transition-[filter] duration-300", hidden && "blur-md")}>{children}</div>
      <AnimatePresence>
        {hidden && (
          <motion.button
            type="button"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={() => setRevealed(true)}
            className="absolute inset-0 flex flex-col items-center justify-center gap-1 rounded-2xl bg-background/60 p-3 text-center text-xs font-bold backdrop-blur-sm"
          >
            <EyeIcon className="size-5" />
            Hidden by streamer mode
            <span className="font-normal text-muted-foreground">Show it anyway</span>
          </motion.button>
        )}
      </AnimatePresence>
    </div>
  );
}

/** A QR code drawn as soft dots with rounded corner marks, popping in as a wave. */
function QrCode({ text }: { text: string }) {
  const qr = useMemo(() => encode(text, { ecc: "M", border: 0 }), [text]);
  const n = qr.size;
  const finder = (x: number, y: number) => (
    <g key={`${x}-${y}`} className="qr-dot" style={{ animationDelay: `${(x + y) * 10}ms`, color: "color-mix(in srgb, var(--primary) 45%, #000)" }}>
      <rect x={x + 0.5} y={y + 0.5} width={6} height={6} rx={1.8} fill="none" stroke="currentColor" strokeWidth={1} />
      <rect x={x + 2} y={y + 2} width={3} height={3} rx={0.9} fill="currentColor" />
    </g>
  );
  const inFinder = (x: number, y: number) => (x < 7 && y < 7) || (x >= n - 7 && y < 7) || (x < 7 && y >= n - 7);
  return (
    <div className="rounded-2xl bg-white p-3 shadow-lg ring-1 ring-black/5">
      <svg viewBox={`-1 -1 ${n + 2} ${n + 2}`} className="size-44 sm:size-48" role="img" aria-label="QR code for your authenticator app">
        {qr.data.flatMap((row, y) =>
          row.map((dark, x) =>
            dark && !inFinder(x, y) ? (
              <rect key={`${x}-${y}`} className="qr-dot" style={{ animationDelay: `${(x + y) * 10}ms` }} x={x + 0.08} y={y + 0.08} width={0.84} height={0.84} rx={0.32} fill="#111118" />
            ) : null,
          ),
        )}
        {finder(0, 0)}
        {finder(n - 7, 0)}
        {finder(0, n - 7)}
      </svg>
    </div>
  );
}

/** The setup key in groups of four, with a copy button. */
function SecretKey({ secret }: { secret: string }) {
  const streaming = usePrefs((p) => p.streamer);
  const groups = secret.match(/.{1,4}/g) ?? [];
  return (
    <div className="flex items-center gap-2">
      <code className={cn("min-w-0 flex-1 rounded-xl bg-muted px-3 py-2 font-mono text-sm font-bold tracking-wider break-all transition-[filter] duration-300", streaming && "blur-sm select-none")}>
        {groups.join(" ")}
      </code>
      <Button type="button" variant="outline" size="icon" className="shrink-0 rounded-xl" aria-label="Copy the key" onClick={() => copy(secret, "the key")}>
        <CopyIcon className="size-4" />
      </Button>
    </div>
  );
}

/** Backup codes in a grid, to copy or download. */
function BackupCodes({ codes, where }: { codes: string[]; where: string }) {
  const streaming = usePrefs((p) => p.streamer);
  function download() {
    const text = `Backup codes for your fuwa account on ${where}\nEach one works once.\n\n${codes.join("\n")}\n`;
    const url = URL.createObjectURL(new Blob([text], { type: "text/plain" }));
    const a = Object.assign(document.createElement("a"), { href: url, download: `fuwa-backup-codes-${where.replace(/[^\w.-]+/g, "-").toLowerCase()}.txt` });
    a.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
    toast("Downloaded your backup codes");
  }
  return (
    <div className="rounded-2xl border bg-card p-4">
      <ul className={cn("grid grid-cols-2 gap-2 transition-[filter] duration-300 sm:grid-cols-5", streaming && "blur-sm select-none")}>
        {codes.map((code, n) => (
          <motion.li
            key={code}
            initial={{ opacity: 0, y: 10, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            transition={{ ...SPRING, delay: 0.05 + n * 0.035 }}
            className="rounded-lg bg-muted px-2 py-1.5 text-center font-mono text-sm font-bold tracking-wider"
          >
            {code}
          </motion.li>
        ))}
      </ul>
      <div className="mt-3 flex flex-wrap gap-2">
        <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={() => copy(codes.join("\n"), "your backup codes")}>
          <CopyIcon className="size-4" /> Copy all
        </Button>
        <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={download}>
          <DownloadIcon className="size-4" /> Download
        </Button>
      </div>
    </div>
  );
}

// ───────────────────────── Once it's on ─────────────────────────

function TurnedOn({ instanceKey, where, status, onChange }: { instanceKey: string; where: string; status: Status; onChange: (s: Status) => void }) {
  const [mode, setMode] = useState<"idle" | "codes" | "off">("idle");
  const [codes, setCodes] = useState<string[] | null>(null);
  const low = status.backupCodesLeft <= 3;
  return (
    <div className="flex flex-col gap-6">
      <div data-setting="two-step" className="relative overflow-hidden rounded-3xl border border-emerald-500/30 bg-emerald-500/5 p-6">
        <div className="pointer-events-none absolute -top-16 -right-16 size-48 rounded-full bg-emerald-500/15 blur-2xl" />
        <div className="relative flex items-center gap-4">
          <motion.span
            initial={{ rotate: -12, scale: 0.7 }}
            animate={{ rotate: 0, scale: 1 }}
            transition={{ type: "spring", stiffness: 400, damping: 14 }}
            className="grid size-14 shrink-0 place-items-center rounded-2xl bg-emerald-500 text-white shadow-lg shadow-emerald-500/30"
          >
            <ShieldCheckIcon className="size-7" />
          </motion.span>
          <div className="min-w-0">
            <p className="text-lg font-extrabold">Two-step sign-in is on</p>
            <p className="text-sm text-muted-foreground">Signing in to {where} asks for a code from your authenticator app after your password.</p>
          </div>
        </div>
      </div>

      <div className="flex flex-col">
        <Row
          id="backup-codes"
          label="Backup codes"
          hint={
            <>
              <span className={cn("font-bold tabular-nums", low ? "text-destructive" : "text-foreground")}>
                <Count value={status.backupCodesLeft} />
              </span>{" "}
              of 10 left.{low ? " Make new ones before you run out." : " Each one signs you in once without your app."}
            </>
          }
        >
          <AnimatePresence mode="wait" initial={false}>
            {codes ? (
              <motion.div key="codes" initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={SPRING} className="flex flex-col gap-3">
                <p className="text-sm text-muted-foreground">Your old codes stopped working. Here are the new ones:</p>
                <BackupCodes codes={codes} where={where} />
                <Button type="button" variant="outline" size="sm" className="self-start rounded-xl" onClick={() => setCodes(null)}>
                  Done
                </Button>
              </motion.div>
            ) : mode === "codes" ? (
              <Confirm
                key="confirm-codes"
                action="New codes"
                withCode={false}
                onCancel={() => setMode("idle")}
                onConfirm={async (password) => {
                  const next = await run(regenerateBackupCodes(instanceKey, password));
                  setCodes(next);
                  setMode("idle");
                  onChange({ ...status, backupCodesLeft: next.length });
                }}
              />
            ) : (
              <motion.div key="button" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
                <Button type="button" variant="outline" className="group rounded-xl" onClick={() => setMode("codes")}>
                  <RefreshCwIcon className="size-4 transition-transform duration-500 group-hover:rotate-180" /> Make new backup codes
                </Button>
              </motion.div>
            )}
          </AnimatePresence>
        </Row>
        <Row id="turn-off-two-step" label="Turn off two-step sign-in" hint="Your password alone will be enough to sign in again.">
          <AnimatePresence mode="wait" initial={false}>
            {mode === "off" ? (
              <Confirm
                key="confirm-off"
                action="Turn off"
                danger
                withCode
                onCancel={() => setMode("idle")}
                onConfirm={async (password, code) => {
                  await run(disableTwoFactor(instanceKey, password, code));
                  toast("Two-step sign-in is off");
                  onChange({ enabled: false, backupCodesLeft: 0 });
                }}
              />
            ) : (
              <motion.div key="button" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
                <Button type="button" variant="outline" className="rounded-xl border-destructive/40 text-destructive hover:bg-destructive/10" onClick={() => setMode("off")}>
                  <ShieldOffIcon className="size-4" /> Turn off
                </Button>
              </motion.div>
            )}
          </AnimatePresence>
        </Row>
      </div>
    </div>
  );
}

/** Asks for the password (and a code) before something that weakens the account. */
function Confirm({
  action,
  danger = false,
  withCode,
  onCancel,
  onConfirm,
}: {
  action: string;
  danger?: boolean;
  withCode: boolean;
  onCancel: () => void;
  onConfirm: (password: string, code: string) => Promise<void>;
}) {
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [show, setShow] = useState(false);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [shake, doShake] = useShake();
  const ready = password.length > 0 && (!withCode || code.trim().length >= 6);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!ready) return doShake();
    setPending(true);
    setError(null);
    try {
      await onConfirm(password, code.trim());
    } catch (err) {
      setError((err as FuwaError).message);
      doShake();
    } finally {
      setPending(false);
    }
  }

  return (
    <motion.form
      onSubmit={submit}
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, y: -8 }}
      transition={SPRING}
      className="flex max-w-md flex-col gap-3 rounded-2xl border bg-card p-4"
    >
      <motion.div animate={shake} className="flex flex-col gap-3">
        <label className="flex flex-col gap-1.5">
          <span className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground">
            <KeyRoundIcon className="size-3.5" /> Your password
          </span>
          <PasswordInput id={`confirm-${action}`} autoComplete="current-password" value={password} onChange={setPassword} show={show} onShow={setShow} />
        </label>
        {withCode && (
          <label className="flex flex-col gap-1.5">
            <span className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground">
              <SmartphoneIcon className="size-3.5" /> A code from your app, or a backup code
            </span>
            <Input
              value={code}
              onChange={(e) => setCode(e.target.value)}
              autoComplete="one-time-code"
              spellCheck={false}
              maxLength={16}
              placeholder="123456"
              className="h-11 rounded-xl font-mono tracking-wider"
            />
          </label>
        )}
      </motion.div>
      <AnimatePresence initial={false}>
        {error && (
          <motion.p initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} className="overflow-hidden text-sm font-bold text-destructive first-letter:uppercase">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <div className="flex gap-2">
        <Button type="button" variant="ghost" size="sm" className="rounded-xl" onClick={onCancel} disabled={pending}>
          Cancel
        </Button>
        <Button type="submit" size="sm" variant={danger ? "destructive" : "default"} className={cn("rounded-xl font-bold", !danger && "btn")} disabled={pending}>
          {pending ? "Checking…" : action}
        </Button>
      </div>
    </motion.form>
  );
}
