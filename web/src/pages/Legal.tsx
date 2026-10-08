import { Link, Navigate } from "@tanstack/react-router";
import { ArrowLeftIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { useEffect } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { FuwaMark } from "@/components/Icons";
import { useI18n } from "@/i18n/react";
import { hostedByUs, LEGAL_CONTACT, type LegalPage } from "@/lib/hosted";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";
import privacy from "@/legal/privacy.md?raw";
import terms from "@/legal/terms.md?raw";

/*
 * Waifu Devs' terms of service and privacy policy (src/legal), at /terms and
 * /privacy on its own instances. Any other instance serves the same app but
 * isn't ours, so there these addresses just go home. The documents are
 * English only; the page around them follows the app's language.
 */

const DOCUMENTS: Record<LegalPage, string> = { terms, privacy };
const contact = `[${LEGAL_CONTACT}](mailto:${LEGAL_CONTACT})`;

export function Legal({ page }: { page: LegalPage }) {
  const { t } = useI18n();
  const title = page === "terms" ? t("connect.legal.termsTitle") : t("connect.legal.privacyTitle");
  useEffect(() => {
    const before = document.title;
    document.title = `${title} · fuwa`;
    return () => {
      document.title = before;
    };
  }, [title]);
  if (!hostedByUs(window.location.origin) && !import.meta.env.DEV) return <Navigate to="/" replace />;

  return (
    <div className="h-full overflow-y-auto">
      <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-10">
        <header className="flex flex-wrap items-center justify-between gap-4">
          <Link to="/" className="group flex items-center gap-2 text-sm font-bold text-muted-foreground transition hover:text-foreground">
            <ArrowLeftIcon className="size-4 transition group-hover:-translate-x-0.5" />
            <FuwaMark className="size-6" />
            {t("connect.legal.home")}
          </Link>
          <nav className="flex gap-1 rounded-full border bg-card/70 p-1 text-sm font-bold">
            {(["terms", "privacy"] as const).map((p) => (
              <Link
                key={p}
                to={p === "terms" ? "/terms" : "/privacy"}
                className={cn("relative rounded-full px-3 py-1 transition active:scale-95", p === page ? "text-primary" : "text-muted-foreground hover:text-foreground")}
              >
                {p === page && <motion.span layoutId="legal-tab" transition={SPRING} className="absolute inset-0 rounded-full bg-primary/15" />}
                <span className="relative">{p === "terms" ? t("connect.legal.terms") : t("connect.legal.privacy")}</span>
              </Link>
            ))}
          </nav>
        </header>
        <motion.article
          key={page}
          initial={{ opacity: 0, y: 12 }}
          animate={{ opacity: 1, y: 0 }}
          transition={SPRING}
          className="legal rounded-3xl border bg-card/85 p-6 leading-relaxed sm:p-10"
        >
          <Markdown remarkPlugins={[remarkGfm]}>{DOCUMENTS[page].replaceAll("{contact}", contact)}</Markdown>
        </motion.article>
      </div>
    </div>
  );
}
