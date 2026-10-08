import { m as motion } from "motion/react";
import type { ReactNode } from "react";
import { T, useI18n } from "@/i18n/react";
import { hostedByUs, legalLink, type LegalPage } from "@/lib/hosted";
import { cn } from "@/lib/utils";

/*
 * Waifu Devs' terms and privacy policy, linked only where the app reached one
 * of its own instances (lib/hosted). Other instances are run by other people,
 * so nothing here shows for them. The pages open in a new tab, so a sign-up
 * half filled in stays as it was.
 */

function LegalLink({ url, page, children, className }: { url: string; page: LegalPage; children: ReactNode; className?: string }) {
  return (
    <a href={legalLink(url, page)} target="_blank" rel="noreferrer" className={cn("transition hover:text-primary hover:underline", className)}>
      {children}
    </a>
  );
}

/** A small "Terms · Privacy" line, for the foot of a page. */
export function LegalFooter({ url, className }: { url: string | undefined; className?: string }) {
  const { t } = useI18n();
  if (!url || !hostedByUs(url)) return null;
  return (
    <motion.footer
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ duration: 0.6, delay: 0.6 }}
      className={cn("flex items-center justify-center gap-2 text-xs text-muted-foreground", className)}
    >
      <LegalLink url={url} page="terms">
        {t("connect.legal.terms")}
      </LegalLink>
      <span aria-hidden>·</span>
      <LegalLink url={url} page="privacy">
        {t("connect.legal.privacy")}
      </LegalLink>
    </motion.footer>
  );
}

/** "By continuing, you agree to...", under the ways to sign in or make an account. */
export function LegalAgreement({ url, className }: { url: string; className?: string }) {
  const { t } = useI18n();
  if (!hostedByUs(url)) return null;
  return (
    <p className={cn("text-center text-xs text-muted-foreground", className)}>
      <T
        k="connect.legal.agree"
        values={{
          terms: (
            <LegalLink url={url} page="terms" className="font-bold text-primary">
              {t("connect.legal.termsTitle")}
            </LegalLink>
          ),
          privacy: (
            <LegalLink url={url} page="privacy" className="font-bold text-primary">
              {t("connect.legal.privacyTitle")}
            </LegalLink>
          ),
        }}
      />
    </p>
  );
}
