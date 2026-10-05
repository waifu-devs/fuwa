// Everything the animations need (springs, exits, layout, drag), in its own chunk so the first
// download carries only the light `m` components; main.tsx loads it alongside the language.
export { domMax as default } from "motion/react";
