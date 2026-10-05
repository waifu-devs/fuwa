/** The app's springs and easing, so things move alike everywhere. */
export const SPRING = { type: "spring", stiffness: 520, damping: 34 } as const;
export const SOFT_SPRING = { type: "spring", stiffness: 380, damping: 32 } as const;
export const EASE_OUT = [0.22, 1, 0.36, 1] as const;

/**
 * Something that opens up in a column (an error line, a panel) fades and slides in rather than
 * growing taller. Whatever sits below it glides out of the way: give those rows `layout="position"`
 * (or share a LayoutGroup with them when they live in another component). Put it in
 * `<AnimatePresence mode="popLayout">`, so a leaving one steps out of the flow at once and the
 * rows after it glide up while it fades.
 */
export const SLIDE_IN = {
  initial: { opacity: 0, y: -6 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: -6 },
} as const;
