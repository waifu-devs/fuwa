import { m as motion } from "motion/react";
import { cn } from "@/lib/utils";

/**
 * A padlock that clicks shut as it appears, with a glint and a few keys'
 * worth of sparkle around it: the mark for "only the two of you can read
 * this".
 */
export function Padlock({ className, delay = 0 }: { className?: string; delay?: number }) {
  return (
    <span className={cn("relative inline-grid size-16 place-items-center", className)}>
      <motion.span
        aria-hidden
        className="absolute inset-0 rounded-full bg-emerald-500/15"
        initial={{ scale: 0.4, opacity: 0 }}
        animate={{ scale: [0.4, 1.15, 1], opacity: 1 }}
        transition={{ duration: 0.6, delay, ease: [0.22, 1, 0.36, 1] }}
      />
      <motion.span
        aria-hidden
        className="absolute inset-0 rounded-full border border-emerald-500/40"
        initial={{ scale: 1, opacity: 0 }}
        animate={{ scale: [1, 1.6], opacity: [0.8, 0] }}
        transition={{ duration: 1.1, delay: delay + 0.55, ease: "easeOut" }}
      />
      <svg viewBox="0 0 32 32" className="relative size-8 text-emerald-600 dark:text-emerald-400" aria-hidden>
        <motion.path
          d="M10.5 14V10.5a5.5 5.5 0 0 1 11 0V14"
          fill="none"
          stroke="currentColor"
          strokeWidth="2.6"
          strokeLinecap="round"
          initial={{ y: -4 }}
          animate={{ y: [-4, -4, 0.6, 0] }}
          transition={{ duration: 0.55, delay: delay + 0.15, times: [0, 0.4, 0.8, 1], ease: "easeOut" }}
        />
        <motion.rect
          x="6.5"
          y="13.5"
          width="19"
          height="14"
          rx="4"
          fill="currentColor"
          initial={{ scale: 0.6, opacity: 0 }}
          animate={{ scale: 1, opacity: 1 }}
          style={{ transformOrigin: "16px 20px" }}
          transition={{ type: "spring", stiffness: 500, damping: 16, delay }}
        />
        <motion.circle
          cx="16"
          cy="20.5"
          r="2.1"
          className="fill-card"
          initial={{ scale: 0 }}
          animate={{ scale: 1 }}
          style={{ transformOrigin: "16px 20.5px" }}
          transition={{ type: "spring", stiffness: 600, damping: 12, delay: delay + 0.6 }}
        />
      </svg>
      {[0, 1, 2].map((n) => (
        <motion.span
          key={n}
          aria-hidden
          className="absolute text-[0.65rem] text-emerald-500"
          style={{ left: `${[8, 82, 70][n]}%`, top: `${[18, 26, 84][n]}%` }}
          initial={{ scale: 0, opacity: 0 }}
          animate={{ scale: [0, 1.2, 0.9], opacity: [0, 1, 0], rotate: [0, 90] }}
          transition={{ duration: 1.2, delay: delay + 0.7 + n * 0.12, ease: "easeOut" }}
        >
          ✦
        </motion.span>
      ))}
    </span>
  );
}
