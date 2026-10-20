import { ArrowLeftIcon, ArrowRightIcon, LoaderCircleIcon, MailCheckIcon, MailIcon, RotateCwIcon, UserPlusIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState, type FormEvent, type ReactNode } from "react";
import type { NewProviderAccount } from "@/gen/fuwa/v1/auth_pb";
import { finishEmailSignIn, startEmailSignIn } from "@/fuwa/actions";
import { useAction } from "@/fuwa/hooks";
import { CodeInput } from "@/components/CodeInput";
import { TwoFactorStep } from "@/components/Connect";
import { LegalAgreement } from "@/components/LegalLinks";
import { MotionButton } from "@/components/motion-button";
import { Private, usePrivateField } from "@/components/Private";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { T, useI18n } from "@/i18n/react";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

const EASE = [0.22, 1, 0.36, 1] as const;

type Step =
  | { kind: "address" }
  | { kind: "code"; ticket: string; email: string }
  /** Someone new: which username, with the code already checked. */
  | { kind: "new"; ticket: string; code: string; account: NewProviderAccount }
  | { kind: "twoStep"; ticket: string };

/**
 * Signing in with a code sent by email: the address, then the six digits
 * from the mail, then a username for someone new (or the two-step code for
 * an account that has it on).
 */
export function EmailSignIn({ url, onBack, onDone }: { url: string; onBack: () => void; onDone: (key: string) => void }) {
  const { t } = useI18n();
  const [step, setStep] = useState<Step>({ kind: "address" });
  const [email, setEmail] = useState("");
  const [shake, setShake] = useState(0);
  const [codeShake, setCodeShake] = useState(0);
  const [resent, setResent] = useState(false);
  const send = useAction(startEmailSignIn);
  const check = useAction(finishEmailSignIn);
  const privateField = usePrivateField();

  async function sendCode(e?: FormEvent) {
    e?.preventDefault();
    const address = email.trim();
    if (!address) return;
    check.setError(null);
    const res = await send.go(url, address);
    if (!res) return setShake((n) => n + 1);
    setResent(step.kind === "code");
    setStep({ kind: "code", ticket: res.ticket, email: address });
  }

  async function finish(ticket: string, code: string, create?: { username: string }) {
    const res = await check.go(url, ticket, code, create);
    // A wrong code clears the boxes for another try.
    if (!res) return setCodeShake((n) => n + 1);
    if ("newAccount" in res) return setStep({ kind: "new", ticket, code, account: res.newAccount! });
    if ("ticket" in res) return setStep({ kind: "twoStep", ticket: res.ticket! });
    onDone(res.key!);
  }

  if (step.kind === "twoStep") {
    return (
      <TwoFactorStep
        url={url}
        ticket={step.ticket}
        onBack={(problem) => {
          setStep({ kind: "address" });
          send.setError(problem ?? null);
        }}
        onDone={onDone}
      />
    );
  }

  if (step.kind === "new") {
    return <NewAccount url={url} step={step} onSignedIn={onDone} onTwoStep={(ticket) => setStep({ kind: "twoStep", ticket })} />;
  }

  if (step.kind === "code") {
    return (
      <motion.div
        initial={{ opacity: 0, x: 24 }}
        animate={{ opacity: 1, x: 0 }}
        transition={{ duration: 0.3, ease: EASE }}
        className="flex flex-col gap-4"
        data-testid="email-code"
      >
        <Head
          icon={<MailCheckIcon className="size-5" />}
          title={t("connect.email.checkTitle")}
          hint={<T k="connect.email.checkHint" values={{ email: <b className="text-foreground"><Private text={step.email} /></b> }} />}
          back={t("connect.email.otherAddress")}
          onBack={() => {
            check.setError(null);
            setResent(false);
            setStep({ kind: "address" });
          }}
        />
        <div className="flex justify-center">
          <CodeInput
            id="email-code"
            label={t("connect.email.code")}
            onComplete={(code) => void finish(step.ticket, code)}
            disabled={check.pending}
            shake={codeShake}
          />
        </div>
        <Problem text={check.error} />
        <AnimatePresence>
          {resent && !check.error && (
            <motion.p {...SLIDE_IN} transition={SPRING} className="text-center text-sm text-muted-foreground">
              {t("connect.email.resent")}
            </motion.p>
          )}
        </AnimatePresence>
        <motion.button
          layout="position"
          transition={SPRING}
          type="button"
          disabled={send.pending}
          onClick={() => void sendCode()}
          className="flex items-center gap-1.5 self-center text-sm font-bold text-primary hover:underline disabled:opacity-50"
        >
          {send.pending ? <LoaderCircleIcon className="size-4 animate-spin" /> : <RotateCwIcon className="size-4" />}
          {t("connect.email.resend")}
        </motion.button>
        <Problem text={send.error} />
      </motion.div>
    );
  }

  return (
    <motion.form
      initial={{ opacity: 0, x: 24 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ duration: 0.3, ease: EASE }}
      onSubmit={sendCode}
      className="flex flex-col gap-4"
      data-testid="email-address"
    >
      <Head
        icon={<MailIcon className="size-5" />}
        title={t("connect.email.title")}
        hint={t("connect.email.hint")}
        back={t("connect.email.back")}
        onBack={onBack}
      />
      <div key={shake} className={cn("flex flex-col gap-2", shake > 0 && "shake")}>
        <Label htmlFor="email-address" className="font-bold">
          {t("connect.email.address")}
        </Label>
        <Input
          id="email-address"
          type="email"
          autoFocus
          autoComplete="email"
          autoCapitalize="none"
          spellCheck={false}
          placeholder="you@example.com"
          value={email}
          onChange={(e) => setEmail(e.target.value)}
          maxLength={254}
          className={cn("h-11 rounded-xl", privateField)}
          required
        />
      </div>
      <Problem text={send.error} />
      <MotionButton layout="position" transition={SPRING} type="submit" size="lg" disabled={send.pending || !email.trim()} className="btn h-11 rounded-xl font-bold">
        {send.pending ? <LoaderCircleIcon className="animate-spin" /> : <MailIcon />}
        {t("connect.email.send")}
      </MotionButton>
      <LegalAgreement url={url} />
    </motion.form>
  );
}

/** Someone new picks a username (their address's first part, free here, filled in). */
function NewAccount({
  url,
  step,
  onSignedIn,
  onTwoStep,
}: {
  url: string;
  step: Extract<Step, { kind: "new" }>;
  onSignedIn: (key: string) => void;
  onTwoStep: (ticket: string) => void;
}) {
  const { t } = useI18n();
  const [edited, setUsername] = useState<string | null>(null);
  const username = edited ?? step.account.suggestedUsername;
  const [shake, setShake] = useState(0);
  const create = useAction(finishEmailSignIn);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const res = await create.go(url, step.ticket, step.code, { username: username.trim().toLowerCase() });
    if (!res) return setShake((n) => n + 1);
    if ("ticket" in res) return onTwoStep(res.ticket!);
    if ("key" in res) onSignedIn(res.key!);
  }

  return (
    <motion.form
      initial={{ opacity: 0, x: 24 }}
      animate={{ opacity: 1, x: 0 }}
      transition={{ duration: 0.3, ease: EASE }}
      onSubmit={submit}
      className="flex flex-col gap-4"
      data-testid="email-new-account"
    >
      <div className="flex flex-col items-center gap-3 text-center">
        <motion.span
          initial={{ scale: 0, rotate: -20 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 420, damping: 16 }}
          className="grid size-16 place-items-center rounded-3xl bg-primary/15 text-primary"
        >
          <UserPlusIcon className="size-7" />
        </motion.span>
        <p className="text-xl font-extrabold">{t("connect.providerDone.newTitle")}</p>
        <p className="text-sm text-muted-foreground">{t("connect.email.newNote")}</p>
      </div>
      <div key={shake} className={cn("flex flex-col gap-2", shake > 0 && "shake")}>
        <Label htmlFor="email-username" className="font-bold">
          {t("connect.account.username")}
        </Label>
        <Input
          id="email-username"
          autoFocus
          autoComplete="username"
          autoCapitalize="none"
          spellCheck={false}
          value={username}
          onChange={(e) => setUsername(e.target.value)}
          className="h-11 rounded-xl"
        />
      </div>
      <Problem text={create.error} />
      <MotionButton layout="position" transition={SPRING} type="submit" size="lg" disabled={create.pending || !username.trim()} className="btn h-11 rounded-xl font-bold">
        {create.pending ? <LoaderCircleIcon className="animate-spin" /> : <ArrowRightIcon />}
        {t("connect.providerDone.create")}
      </MotionButton>
      <LegalAgreement url={url} />
    </motion.form>
  );
}

function Head({ icon, title, hint, back, onBack }: { icon: ReactNode; title: string; hint: ReactNode; back: string; onBack: () => void }) {
  return (
    <div className="flex items-center gap-3">
      <button
        type="button"
        onClick={onBack}
        className="grid size-9 shrink-0 place-items-center rounded-full text-muted-foreground transition hover:-translate-x-0.5 hover:bg-muted hover:text-foreground"
        aria-label={back}
      >
        <ArrowLeftIcon className="size-4" />
      </button>
      <motion.span
        initial={{ scale: 0, rotate: -30 }}
        animate={{ scale: 1, rotate: 0 }}
        transition={{ type: "spring", stiffness: 500, damping: 16, delay: 0.1 }}
        className="grid size-10 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary"
      >
        {icon}
      </motion.span>
      <div className="min-w-0">
        <p className="font-extrabold">{title}</p>
        <p className="text-xs text-muted-foreground">{hint}</p>
      </div>
    </div>
  );
}

function Problem({ text }: { text: string | null }) {
  return (
    <AnimatePresence mode="popLayout">
      {text && (
        <motion.p {...SLIDE_IN} transition={SPRING} className="text-center text-sm text-destructive first-letter:uppercase">
          {text}
        </motion.p>
      )}
    </AnimatePresence>
  );
}
