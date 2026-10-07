import { DatabaseIcon, LayersIcon, ServerCogIcon } from "lucide-react";
import { m as motion, useReducedMotion } from "motion/react";
import { useMemo } from "react";
import { BubbleBackground } from "@/components/animate-ui/components/backgrounds/bubble";
import { RotatingText, RotatingTextContainer } from "@/components/animate-ui/primitives/texts/rotating";
import { Connect, useHomeInstance } from "@/components/Connect";
import { DesktopDownload } from "@/components/DesktopDownload";
import { FuwaMark } from "@/components/Icons";
import { Petals } from "@/components/Petals";
import { useI18n } from "@/i18n/react";
import type { Key } from "@/i18n/i18n";
import { activeTheme, usePrefs } from "@/lib/prefs";
import { mix } from "@/lib/themes";

const PHRASES: Key[] = ["connect.welcome.phrase.ourServers", "connect.welcome.phrase.ownBox", "connect.welcome.phrase.friends", "connect.welcome.phrase.instances"];
const EASE = [0.22, 1, 0.36, 1] as const;
const rgb = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)).join(",");

const FEATURES: { icon: typeof LayersIcon; text: Key }[] = [
  { icon: LayersIcon, text: "connect.welcome.feature.oneApp" },
  { icon: ServerCogIcon, text: "connect.welcome.feature.selfHost" },
  { icon: DatabaseIcon, text: "connect.welcome.feature.database" },
];

/** First visit: what fuwa is, and where to connect. */
export function Welcome({ initialUrl }: { initialUrl?: string }) {
  const reduce = useReducedMotion();
  const tokens = usePrefs(activeTheme).variant.tokens;
  const { t } = useI18n();
  // The same list while the language stays, so the rotation's timer isn't restarted on every render.
  const phrases = useMemo(() => PHRASES.map((key) => t(key)), [t]);
  // The fuwa server serving this page, which hands out the desktop app.
  const home = useHomeInstance();
  return (
    <div className="relative isolate min-h-full overflow-x-clip overflow-y-auto">
      <BubbleBackground
        interactive
        colors={{
          first: rgb(tokens.primary),
          second: rgb(mix(tokens.primary, "#a78bfa", 0.5)),
          third: rgb(mix(tokens.primary, "#7dd3fc", 0.45)),
          fourth: rgb(tokens.ring),
          fifth: rgb(mix(tokens.primary, "#f9a8d4", 0.5)),
          sixth: rgb(tokens.accent),
        }}
        className="fixed inset-0 -z-20 bg-none opacity-30"
      />
      <div aria-hidden className="dot-grid fixed inset-0 -z-10" />
      <Petals />

      <div className="mx-auto grid min-h-svh max-w-6xl items-center gap-10 px-4 py-12 lg:grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)]">
        <div className="flex flex-col items-start gap-6">
          <motion.div
            initial={{ opacity: 0, scale: 0.6, rotate: -12 }}
            animate={{ opacity: 1, scale: 1, rotate: 0 }}
            transition={{ type: "spring", stiffness: 260, damping: 16 }}
          >
            <FuwaMark className="float size-20 drop-shadow-[0_12px_24px_color-mix(in_srgb,var(--primary)_45%,transparent)]" />
          </motion.div>
          <h1 className="text-[clamp(2.4rem,6vw,4.4rem)] leading-[0.95] font-extrabold tracking-tight">
            <motion.span
              className="block"
              initial={{ opacity: 0, y: 24 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.7, ease: EASE, delay: 0.1 }}
            >
              {t("connect.welcome.headline")}
            </motion.span>
            <motion.span
              className="mt-1 block text-[0.72em]"
              initial={{ opacity: 0, y: 24 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.7, ease: EASE, delay: 0.25 }}
            >
              <RotatingTextContainer text={reduce ? phrases[0]! : phrases} duration={2400} delay={1200} className="min-h-[1.2em] leading-[1.2]">
                <RotatingText className="gradient-text whitespace-nowrap" />
              </RotatingTextContainer>
            </motion.span>
          </h1>
          <motion.p
            initial={{ opacity: 0, y: 16 }}
            animate={{ opacity: 1, y: 0 }}
            transition={{ duration: 0.7, ease: EASE, delay: 0.4 }}
            className="max-w-lg text-lg text-muted-foreground"
          >
            {t("connect.welcome.pitch")}
          </motion.p>
          <ul className="stagger flex flex-wrap gap-2">
            {FEATURES.map((f) => (
              <li key={f.text} className="flex items-center gap-2 rounded-full border bg-card/70 px-3 py-1.5 text-sm font-bold backdrop-blur">
                <f.icon className="size-4 text-primary" /> {t(f.text)}
              </li>
            ))}
          </ul>
          {home && <DesktopDownload node={home.node} url={home.url} align="start" />}
        </div>

        <motion.div
          initial={{ opacity: 0, y: 30, rotateX: 8 }}
          animate={{ opacity: 1, y: 0, rotateX: 0 }}
          transition={{ duration: 0.8, ease: EASE, delay: 0.3 }}
          className="w-full rounded-3xl border bg-card/85 p-6 shadow-[0_30px_80px_-40px_var(--primary)] backdrop-blur-xl sm:p-8"
        >
          <h2 className="mb-1 text-xl font-extrabold">{t("connect.welcome.connectTitle")}</h2>
          <p className="mb-5 text-sm text-muted-foreground">{t("connect.welcome.connectHint")}</p>
          <Connect initialUrl={initialUrl} />
        </motion.div>
      </div>
    </div>
  );
}
