import { CheckIcon, EyeIcon, EyeOffIcon, KeyRoundIcon, LogOutIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useState, type FormEvent, type ReactNode } from "react";
import { AccountKind, type User } from "@/gen/fuwa/v1/types_pb";
import { changePassword, forget, signOut, updateProfile } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { hue, UserAvatar } from "@/components/Icons";
import { SPRING, SwapText } from "@/components/motion";
import { Private } from "@/components/Private";
import { SaveBar, WithPreview } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { cn } from "@/lib/utils";

/** Accounts made on the instance sign in with a password; linked ones sign in through waifu.dev. */
export const hasPassword = (user: User | undefined) => !!user && user.kind !== AccountKind.LINKED;

export function Profile({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const me = inst?.me;
  const [name, setName] = useState(me?.displayName ?? "");
  const [avatar, setAvatar] = useState(me?.avatarUrl ?? "");
  const save = useAction(updateProfile);
  if (!me) return null;
  const preview = { ...me, displayName: name, avatarUrl: avatar };
  const changes = [name !== me.displayName, avatar !== me.avatarUrl].filter(Boolean).length;

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    await save.go(instanceKey, name.trim(), avatar.trim());
  }

  return (
    <form onSubmit={submit}>
      <WithPreview preview={<ProfileCard user={preview} />}>
        <div className="flex flex-col">
          <Row id="display-name" label="Display name" htmlFor="profile-name" hint="What people see next to your messages.">
            <Input id="profile-name" maxLength={64} value={name} placeholder={me.username} onChange={(e) => setName(e.target.value)} className="h-11 rounded-xl" />
          </Row>
          <Row id="avatar" label="Avatar" htmlFor="profile-avatar" hint="A link to a picture. Without one you get your initial on your own color.">
            <Input id="profile-avatar" type="url" placeholder="https://…" value={avatar} onChange={(e) => setAvatar(e.target.value)} className="h-11 rounded-xl" />
          </Row>
          <Row id="username" label="Username" hint="Set when the account was made.">
            <p className="text-sm font-bold">
              @<Private text={me.username} kind="name" />
            </p>
          </Row>
        </div>
        <SaveBar
          count={changes}
          saving={save.pending}
          error={save.error}
          onSave={() => void submit()}
          onDiscard={() => {
            setName(me.displayName);
            setAvatar(me.avatarUrl);
            save.setError(null);
          }}
        />
      </WithPreview>
    </form>
  );
}

/** One field of a form, as a flat row under a rule. */
function Row({ id, label, htmlFor, hint, children }: { id?: string; label: string; htmlFor?: string; hint?: ReactNode; children: ReactNode }) {
  return (
    <div data-setting={id} className="flex flex-col gap-2 border-b border-border/70 py-5 first:pt-0 last:border-b-0">
      <Label htmlFor={htmlFor} className="font-extrabold">
        {label}
      </Label>
      {children}
      {hint && <p className="text-sm text-muted-foreground">{hint}</p>}
    </div>
  );
}

/** How others see you: a card with your color, avatar and name, and one of your messages. */
function ProfileCard({ user }: { user: User }) {
  const shown = user.displayName || user.username;
  return (
    <div className="overflow-hidden rounded-3xl border bg-card shadow-lg">
      <motion.div key={user.avatarUrl} style={hue(user.id)} className="server-gradient h-24" initial={{ opacity: 0.6 }} animate={{ opacity: 1 }} />
      <div className="relative -mt-11 px-4 pb-4">
        <span className="avatar-ring inline-block rounded-full p-[3px]">
          <UserAvatar user={user} className="size-20 text-3xl ring-4 ring-card" />
        </span>
        <p className="mt-2 truncate text-xl font-extrabold">
          <SwapText className="truncate align-bottom">{shown}</SwapText>
        </p>
        <p className="truncate text-sm text-muted-foreground">
          @<Private text={user.username} kind="name" />
        </p>
        <div className="mt-4 flex gap-2.5 rounded-2xl bg-muted/60 p-3">
          <UserAvatar user={user} className="size-8 text-xs" />
          <div className="min-w-0">
            <p className="truncate text-sm font-extrabold">
              <SwapText className="truncate align-bottom">{shown}</SwapText>{" "}
              <span className="text-xs font-normal text-muted-foreground">Today</span>
            </p>
            <p className="text-sm">This is how my messages look ✨</p>
          </div>
        </div>
      </div>
    </div>
  );
}

// ───────────────────────── Password ─────────────────────────

const MIN = 8;
const MAX = 256;

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

function Warn({ children }: { children: ReactNode }) {
  return <span className="font-bold text-destructive">{children}</span>;
}

function PasswordInput({
  id,
  value,
  onChange,
  show,
  onShow,
  autoComplete,
}: {
  id: string;
  value: string;
  onChange: (value: string) => void;
  show: boolean;
  onShow: (show: boolean) => void;
  autoComplete: string;
}) {
  return (
    <div className="relative">
      <Input
        id={id}
        type={show ? "text" : "password"}
        autoComplete={autoComplete}
        maxLength={MAX}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className="h-11 rounded-xl pr-11"
        spellCheck={false}
      />
      <button
        type="button"
        onClick={() => onShow(!show)}
        aria-label={show ? "Hide passwords" : "Show passwords"}
        aria-pressed={show}
        className="absolute top-1/2 right-1.5 grid size-8 -translate-y-1/2 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90"
      >
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span key={String(show)} initial={{ opacity: 0, rotate: -40, scale: 0.6 }} animate={{ opacity: 1, rotate: 0, scale: 1 }} exit={{ opacity: 0, rotate: 40, scale: 0.6 }} transition={SPRING}>
            {show ? <EyeOffIcon className="size-4" /> : <EyeIcon className="size-4" />}
          </motion.span>
        </AnimatePresence>
      </button>
    </div>
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
