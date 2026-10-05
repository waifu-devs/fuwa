/**
 * The translation runtime, with no dependencies so it runs anywhere (the page,
 * the server, and node's test runner). The site (waifu-devs/site) carries the same file;
 * keep the two identical, test vectors and all.
 *
 * Catalogs are static JSON in locales/<code>/<namespace>.json. A value is text
 * with {name} placeholders, or plural forms keyed by CLDR category:
 *   "members": { "one": "{count} member", "other": "{count} members" }
 * Catalogs hold no markup; values are always inserted as text.
 */

export type Plural = Partial<Record<Intl.LDMLPluralRule, string>> & { other: string };
export type Entry = string | Plural;
/** One namespace file: flat keys (dots allowed) to entries. */
export type Namespace = Record<string, Entry>;
/** Every namespace of a locale, flattened to "namespace.key". */
export type Catalog = Map<string, Entry>;

export type Meta = {
  /** The language's own name for itself ("Español"). */
  name: string;
  /** Its name in English ("Spanish"). */
  english: string;
  dir: "ltr" | "rtl";
  /** The plural rule family (desktop uses it; the web asks Intl.PluralRules). */
  plural: string;
  /** False while a translation is a draft nobody who speaks it has checked. */
  reviewed: boolean;
};

/** BCP 47 tags as we ship them: "en", "es", "pt-BR", "zh-Hant". */
const TAG = /^[a-z]{2,3}(?:-(?:[A-Z]{2}|[A-Z][a-z]{3}))?$/;

/** Whether a string is shaped like a locale tag (shipped or not). */
export const isTag = (value: string) => value.length <= 12 && TAG.test(value);

export function flatten(namespaces: Record<string, Namespace>): Catalog {
  const catalog: Catalog = new Map();
  for (const [ns, entries] of Object.entries(namespaces)) {
    for (const [key, entry] of Object.entries(entries)) catalog.set(`${ns}.${key}`, entry);
  }
  return catalog;
}

const PLACEHOLDER = /\{([a-z][a-zA-Z0-9]*)\}/g;

/** The placeholder names a template uses, sorted (for checking translations against English). */
export function placeholders(entry: Entry): string[] {
  const texts = typeof entry === "string" ? [entry] : Object.values(entry);
  const names = new Set<string>();
  for (const text of texts) for (const m of (text ?? "").matchAll(PLACEHOLDER)) names.add(m[1]);
  return [...names].sort();
}

const rules = new Map<string, Intl.PluralRules>();
function pluralRule(locale: string) {
  let rule = rules.get(locale);
  if (!rule) rules.set(locale, (rule = new Intl.PluralRules(locale)));
  return rule;
}

/** Picks the plural form for `count`; a form the catalog leaves out falls back to "other". */
export function pickPlural(locale: string, forms: Plural, count: number): string {
  return forms[pluralRule(locale).select(count)] ?? forms.other;
}

/**
 * Splits a template into text and placeholder parts, so callers can insert any
 * kind of value (strings, or React nodes) without parsing markup.
 */
export function parts(template: string): Array<string | { name: string }> {
  const out: Array<string | { name: string }> = [];
  let last = 0;
  for (const m of template.matchAll(PLACEHOLDER)) {
    if (m.index > last) out.push(template.slice(last, m.index));
    out.push({ name: m[1] });
    last = m.index + m[0].length;
  }
  if (last < template.length) out.push(template.slice(last));
  return out;
}

/** The template for a key: the locale's, else English's, else the key itself (so a gap is visible, not blank). */
export function template(locale: string, catalog: Catalog, english: Catalog, key: string, count?: number): string {
  const entry = catalog.get(key) ?? english.get(key);
  if (entry === undefined) return key;
  if (typeof entry === "string") return entry;
  // English plural forms for English text, so a half-translated plural never mixes languages.
  const from = catalog.has(key) ? locale : "en";
  return pickPlural(from, entry, count ?? 0);
}

/**
 * Fills placeholders with text, in one pass: a value that itself looks like a
 * placeholder (a server called "{count}") goes in as written. Missing values
 * stay as "{name}", which shows up in review.
 */
export function fill(text: string, values: Record<string, string> = {}): string {
  return text.replace(PLACEHOLDER, (whole, name: string) => (Object.hasOwn(values, name) ? values[name] : whole));
}

/**
 * Picks the shipped locale that best matches what someone asked for, in order:
 * an exact match, then the same language ("es-MX" gets "es"), else English.
 */
export function negotiate(requested: readonly string[], available: readonly string[], fallback = "en"): string {
  const lower = new Map(available.map((code) => [code.toLowerCase(), code]));
  for (const raw of requested) {
    const want = raw.trim().replace(/_/g, "-").toLowerCase();
    if (!want || want === "*") continue;
    const exact = lower.get(want);
    if (exact) return exact;
    const base = lower.get(want.split("-")[0]);
    if (base) return base;
  }
  return fallback;
}

/**
 * The languages in an Accept-Language header, most wanted first. Bounded, since
 * the header is the visitor's to make as long as they like.
 */
export function acceptLanguage(header: string | null | undefined): string[] {
  if (!header) return [];
  return header
    .slice(0, 256)
    .split(",")
    .slice(0, 16)
    .map((item, index) => {
      const [tag, ...params] = item.trim().split(";");
      const q = params.map((p) => p.trim()).find((p) => p.startsWith("q="));
      const weight = q ? Number(q.slice(2)) : 1;
      return { tag: tag.trim(), weight: Number.isFinite(weight) ? weight : 0, index };
    })
    .filter((item) => item.tag && item.weight > 0)
    .sort((a, b) => b.weight - a.weight || a.index - b.index)
    .map((item) => item.tag);
}
