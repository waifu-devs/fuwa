import { DatabaseIcon, LayersIcon, ServerCogIcon } from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import { BubbleBackground } from "@/components/animate-ui/components/backgrounds/bubble";
import { RotatingText, RotatingTextContainer } from "@/components/animate-ui/primitives/texts/rotating";
import { Connect } from "@/components/Connect";
import { FuwaMark } from "@/components/Icons";
import { Petals } from "@/components/Petals";
import { mix, savedTheme } from "@/lib/themes";

const PHRASES = ["on our servers.", "on your own box.", "with your friends.", "across instances."];
const EASE = [0.22, 1, 0.36, 1] as const;
const rgb = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16)).join(",");

const FEATURES = [
  { icon: LayersIcon, text: "One app, every server" },
  { icon: ServerCogIcon, text: "Self-host with one command" },
  { icon: DatabaseIcon, text: "A database per community" },
];

/** First visit: what fuwa is, and where to connect. */
export function Welcome({ initialUrl }: { initialUrl?: string }) {
  const reduce = useReducedMotion();
  const t = savedTheme().variant.tokens;
  return (
    <div className="relative isolate min-h-full overflow-x-clip overflow-y-auto">
      <BubbleBackground
        interactive
        colors={{
          first: rgb(t.primary),
          second: rgb(mix(t.primary, "#a78bfa", 0.5)),
          third: rgb(mix(t.primary, "#7dd3fc", 0.45)),
          fourth: rgb(t.ring),
          fifth: rgb(mix(t.primary, "#f9a8d4", 0.5)),
          sixth: rgb(t.accent),
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
              Chat anywhere,
            </motion.span>
            <motion.span
              className="mt-1 block text-[0.72em]"
              initial={{ opacity: 0, y: 24 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ duration: 0.7, ease: EASE, delay: 0.25 }}
            >
              <RotatingTextContainer text={reduce ? PHRASES[0]! : PHRASES} duration={2400} delay={1200} className="min-h-[1.2em] leading-[1.2]">
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
            fuwa is a chat app for communities, made by Waifu Devs. Join servers hosted by us or by anyone else, all from
            one place. Or run your own.
          </motion.p>
          <ul className="stagger flex flex-wrap gap-2">
            {FEATURES.map((f) => (
              <li key={f.text} className="flex items-center gap-2 rounded-full border bg-card/70 px-3 py-1.5 text-sm font-bold backdrop-blur">
                <f.icon className="size-4 text-primary" /> {f.text}
              </li>
            ))}
          </ul>
        </div>

        <motion.div
          initial={{ opacity: 0, y: 30, rotateX: 8 }}
          animate={{ opacity: 1, y: 0, rotateX: 0 }}
          transition={{ duration: 0.8, ease: EASE, delay: 0.3 }}
          className="w-full rounded-3xl border bg-card/85 p-6 shadow-[0_30px_80px_-40px_var(--primary)] backdrop-blur-xl sm:p-8"
        >
          <h2 className="mb-1 text-xl font-extrabold">Connect to a fuwa server</h2>
          <p className="mb-5 text-sm text-muted-foreground">Enter its address to sign in or make an account.</p>
          <Connect initialUrl={initialUrl} />
        </motion.div>
      </div>
    </div>
  );
}
