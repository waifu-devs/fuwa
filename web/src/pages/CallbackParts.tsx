import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, CloudOffIcon } from "lucide-react";
import { m as motion } from "motion/react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import type { Key } from "@/i18n/i18n";
import { useI18n } from "@/i18n/react";

/** What the sign-in callback pages (waifu.dev's and an identity provider's) show alike. */

export function Title({ children }: { children: ReactNode }) {
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

/** A check that pops in with a ring rippling out behind it. */
export function Check() {
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

/** The person said the sign-in wasn't theirs. */
export function Cancelled() {
  const { t } = useI18n();
  return (
    <>
      <Badge tone="muted">
        <CheckIcon className="size-7" />
      </Badge>
      <Title>{t("connect.callback.cancelledTitle")}</Title>
      <p className="text-sm text-muted-foreground">{t("connect.callback.closeTab")}</p>
    </>
  );
}

/** The sign-in didn't finish: `message` is what the server or provider said, if anything; `note` is ours, shown when it said nothing. */
export function Failed({ title, message, note }: { title: Key; message?: string; note?: Key }) {
  const { t } = useI18n();
  const navigate = useNavigate();
  return (
    <>
      <Badge tone="error">
        <CloudOffIcon className="size-7" />
      </Badge>
      <Title>{t(title)}</Title>
      <p className="text-sm text-muted-foreground first-letter:uppercase">{message || (note && t(note))}</p>
      <Button size="lg" className="btn h-11 w-full rounded-xl font-bold" onClick={() => navigate({ to: "/", replace: true })}>
        {t("connect.callback.backToFuwa")}
      </Button>
    </>
  );
}
