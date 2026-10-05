import type { Key } from "../../i18n/i18n.ts";

const EFFECT_TEXT: Record<string, { name: Key; about: Key }> = {
  sakura: { name: "accountsettings.effects.name.sakura", about: "accountsettings.effects.about.sakura" },
  starfall: { name: "accountsettings.effects.name.starfall", about: "accountsettings.effects.about.starfall" },
  sparkles: { name: "accountsettings.effects.name.sparkles", about: "accountsettings.effects.about.sparkles" },
  hearts: { name: "accountsettings.effects.name.hearts", about: "accountsettings.effects.about.hearts" },
  snow: { name: "accountsettings.effects.name.snow", about: "accountsettings.effects.about.snow" },
  bubbles: { name: "accountsettings.effects.name.bubbles", about: "accountsettings.effects.about.bubbles" },
  fireflies: { name: "accountsettings.effects.name.fireflies", about: "accountsettings.effects.about.fireflies" },
  confetti: { name: "accountsettings.effects.name.confetti", about: "accountsettings.effects.about.confetti" },
};

/** The catalog keys for a built-in effect's name and line, or undefined for any other id (inherited names like "constructor" included). */
export function effectKeys(id: string): { name: Key; about: Key } | undefined {
  return Object.hasOwn(EFFECT_TEXT, id) ? EFFECT_TEXT[id] : undefined;
}
