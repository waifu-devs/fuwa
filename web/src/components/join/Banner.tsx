import { UsersIcon } from "lucide-react";
import { motion, useTransform, type MotionValue } from "motion/react";
import { useState, type ReactNode } from "react";
import { ServerIcon } from "@/components/Icons";
import { Count } from "@/components/motion";
import { accentVars, bannerPosition, type BannerServer } from "@/lib/banner";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

/** A slow drift across the banner, there and back, on the compositor only. */
const PAN = {
  animate: { transform: ["scale(1.06) translate3d(-1.2%, 0.6%, 0)", "scale(1.12) translate3d(1.2%, -0.6%, 0)"] },
  transition: { duration: 26, repeat: Infinity, repeatType: "mirror" as const, ease: "easeInOut" as const },
};

/**
 * A server's banner, or a gradient in its colors when it has none. The
 * picture keeps its focal point in view whatever shape it's shown in, pans
 * slowly unless motion is reduced, and with `scrollY` it moves at half the
 * speed of what scrolls over it. It fades into the card at the bottom.
 */
export function ServerBanner({
  server,
  className,
  scrollY,
  pan = true,
  fade = true,
  children,
}: {
  server: Pick<BannerServer, "id" | "bannerUrl" | "bannerFocusX" | "bannerFocusY" | "accentColor">;
  className?: string;
  /** How far the page over the banner has scrolled, for the parallax. */
  scrollY?: MotionValue<number>;
  pan?: boolean;
  fade?: boolean;
  children?: ReactNode;
}) {
  const still = usePrefs(reduceMotion);
  const [broken, setBroken] = useState<string | null>(null);
  const src = shownPicture(server.bannerUrl);
  const picture = !!src && broken !== src;
  return (
    <div style={accentVars(server)} className={cn("relative isolate overflow-hidden", className)}>
      <Parallax scrollY={still ? undefined : scrollY}>
        {picture ? (
          <motion.img
            key={src}
            src={src}
            alt=""
            draggable={false}
            onError={() => setBroken(src)}
            initial={{ opacity: 0 }}
            animate={pan && !still ? { opacity: 1, ...PAN.animate } : { opacity: 1, transform: "scale(1.04)" }}
            transition={{ opacity: { duration: 0.6 }, transform: pan && !still ? PAN.transition : { duration: 0 } }}
            style={{ objectPosition: bannerPosition(server) }}
            className="absolute inset-0 size-full object-cover will-change-transform"
          />
        ) : (
          <Gradient moving={pan && !still} />
        )}
      </Parallax>
      {fade && <div aria-hidden className="absolute inset-x-0 bottom-0 h-2/3 bg-gradient-to-t from-card via-card/40 to-transparent" />}
      {children}
    </div>
  );
}

/** Moves its banner at half the scroll, and holds it still without one. */
function Parallax({ scrollY, children }: { scrollY?: MotionValue<number>; children: ReactNode }) {
  if (!scrollY) return <div className="absolute inset-0">{children}</div>;
  return <Parallaxed scrollY={scrollY}>{children}</Parallaxed>;
}

function Parallaxed({ scrollY, children }: { scrollY: MotionValue<number>; children: ReactNode }) {
  const y = useTransform(scrollY, [0, 400], [0, 200], { clamp: false });
  return (
    <motion.div style={{ y }} className="absolute inset-0 will-change-transform">
      {children}
    </motion.div>
  );
}

/** The banner of a server without one: its accent and hue, two soft lights drifting over them. */
function Gradient({ moving }: { moving: boolean }) {
  const drift = (to: string) =>
    moving ? { animate: { transform: ["translate3d(0,0,0) scale(1)", to] }, transition: { duration: 18, repeat: Infinity, repeatType: "mirror" as const, ease: "easeInOut" as const } } : {};
  return (
    <div
      aria-hidden
      className="absolute inset-0"
      style={{
        background:
          "linear-gradient(120deg, color-mix(in srgb, var(--accent-server) 85%, black) 0%, var(--accent-server) 45%, hsl(calc(var(--h) + 50) 70% 60%) 100%)",
      }}
    >
      <motion.span {...drift("translate3d(18%, 12%, 0) scale(1.2)")} className="absolute -top-1/3 -left-[10%] size-[70%] rounded-full bg-white/25 blur-3xl" />
      <motion.span {...drift("translate3d(-16%, -10%, 0) scale(1.15)")} className="absolute -right-[10%] -bottom-1/2 size-[80%] rounded-full bg-black/20 blur-3xl" />
      <span className="dot-grid absolute inset-0 opacity-25 mix-blend-overlay" />
    </div>
  );
}

/**
 * The top of the welcome, applying and onboarding screens and of invites:
 * the banner, the server's icon popping in over its bottom edge, a line
 * above the name ("Welcome to"), the name, and how many are in it. `bleed`
 * pulls it out to a dialog's edges.
 */
export function BannerHero({
  server,
  eyebrow,
  badge,
  scrollY,
  bleed = false,
  compact = false,
  children,
  className,
}: {
  server: BannerServer & { memberCount?: bigint };
  eyebrow?: ReactNode;
  /** A little round sign on the icon, such as a scroll for rules. */
  badge?: ReactNode;
  scrollY?: MotionValue<number>;
  bleed?: boolean;
  /** Shorter, for phones and previews. */
  compact?: boolean;
  children?: ReactNode;
  className?: string;
}) {
  const members = server.memberCount;
  return (
    <div style={accentVars(server)} className={cn("relative", bleed && "-mx-6 -mt-6", className)}>
      <ServerBanner server={server} scrollY={scrollY} className={cn(compact ? "h-28" : "h-32 sm:h-40", bleed && "rounded-t-3xl")} />
      <div className={cn("relative flex flex-col gap-1", compact ? "-mt-9 px-4" : "-mt-11 px-6")}>
        <motion.span
          initial={{ scale: 0.4, rotate: -16, y: 14, opacity: 0 }}
          animate={{ scale: 1, rotate: 0, y: 0, opacity: 1 }}
          transition={{ type: "spring", stiffness: 380, damping: 14, delay: 0.08 }}
          className="relative w-fit"
        >
          <ServerIcon server={server} active className={cn("shadow-xl ring-4 ring-card", compact ? "size-16 text-lg" : "size-20 text-xl")} />
          {badge && (
            <motion.span
              initial={{ scale: 0, rotate: -50 }}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: "spring", stiffness: 500, damping: 13, delay: 0.32 }}
              className="absolute -right-2 -bottom-1 grid size-7 place-items-center rounded-full bg-[var(--accent-server)] text-white shadow ring-[3px] ring-card"
            >
              {badge}
            </motion.span>
          )}
        </motion.span>
        <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ type: "spring", stiffness: 420, damping: 30, delay: 0.14 }} className="min-w-0 pr-8">
          {eyebrow && <p className="text-xs font-extrabold tracking-wide text-[color-mix(in_srgb,var(--accent-server)_75%,var(--foreground))] uppercase">{eyebrow}</p>}
          <h2 className={cn("font-extrabold tracking-tight break-words", compact ? "text-lg" : "text-2xl")}>{server.name}</h2>
          {members !== undefined && members > 0n && (
            <p className="flex items-center gap-1 text-xs text-muted-foreground">
              <span className="relative grid size-3.5 place-items-center">
                <span className="absolute size-2 animate-ping rounded-full bg-emerald-500/60 motion-reduce:hidden" />
                <span className="size-1.5 rounded-full bg-emerald-500" />
              </span>
              <UsersIcon className="ml-0.5 size-3.5" /> <Count value={Number(members)} /> {members === 1n ? "member" : "members"}
            </p>
          )}
        </motion.div>
        {children}
      </div>
    </div>
  );
}
