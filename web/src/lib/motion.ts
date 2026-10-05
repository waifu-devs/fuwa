/** The app's springs and easing, so things move alike everywhere. */
export const SPRING = { type: "spring", stiffness: 520, damping: 34 } as const;
export const SOFT_SPRING = { type: "spring", stiffness: 380, damping: 32 } as const;
export const EASE_OUT = [0.22, 1, 0.36, 1] as const;
