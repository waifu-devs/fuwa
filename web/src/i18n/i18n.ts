import { CODES, english, languageOf, loadCatalog, type Namespaces, shipped } from "./catalogs.ts";
import { type Catalog, fill, negotiate, template } from "./core.ts";
import { Problem } from "./problem.ts";

/**
 * The app's language: the Language setting (lib/prefs), or the browser's own
 * languages while it says "auto". Strings come from locales/ at the repo root,
 * shared with the desktop app.
 */

/** Every key English has, as "namespace.key"; a typo is a type error. */
export type Key = { [N in keyof Namespaces]: `${N & string}.${keyof Namespaces[N] & string}` }[keyof Namespaces];

type Value = string | number;

export type I18n = {
  /** The language the app is in. */
  locale: string;
  dir: "ltr" | "rtl";
  /** A string; `count` picks the plural form and is shown in the language's digits. */
  t: (key: Key, values?: Record<string, Value>) => string;
  number: (value: number, options?: Intl.NumberFormatOptions) => string;
  date: (value: Date | number, options?: Intl.DateTimeFormatOptions) => string;
  /** The catalog, for <T>. */
  catalog: Catalog;
};

const formats = new Map<string, Intl.NumberFormat | Intl.DateTimeFormat>();
/** Intl formatters are slow to make, so each (language, options) pair is made once. */
export function cachedFormat<F extends Intl.NumberFormat | Intl.DateTimeFormat>(kind: "n" | "d", locale: string, options: object | undefined, make: () => F): F {
  const id = `${kind}|${locale}|${JSON.stringify(options ?? {})}`;
  let format = formats.get(id) as F | undefined;
  if (!format) formats.set(id, (format = make()));
  return format;
}

/** Strings, numbers and dates in one language, falling back to English per key. */
export function makeI18n(locale: string, catalog: Catalog): I18n {
  const number = (value: number, options?: Intl.NumberFormatOptions) =>
    cachedFormat("n", locale, options, () => new Intl.NumberFormat(locale, options)).format(value);
  const date = (value: Date | number, options?: Intl.DateTimeFormatOptions) =>
    cachedFormat("d", locale, options, () => new Intl.DateTimeFormat(locale, options)).format(value);
  const strings = (values: Record<string, Value> = {}) =>
    Object.fromEntries(Object.entries(values).map(([name, v]) => [name, typeof v === "number" ? number(v) : v]));
  return {
    locale,
    dir: languageOf(locale).dir,
    number,
    date,
    catalog,
    t: (key, values) => {
      const count = typeof values?.count === "number" ? values.count : undefined;
      return fill(template(locale, catalog, english, key, count), strings(values));
    },
  };
}

let current = makeI18n("en", english);
const listeners = new Set<() => void>();

/** The app's language now. Components read it with useI18n, which re-renders them when it changes. */
export const i18n = () => current;

/** A Problem (i18n/problem.ts) as an Error in the app's language; anything else as it was. */
export const inWords = (err: unknown): unknown => (err instanceof Problem ? new Error(current.t(err.key)) : err);

export function subscribeI18n(listener: () => void) {
  listeners.add(listener);
  return () => void listeners.delete(listener);
}

/** The shipped language closest to the browser's own. */
export const browserLanguage = () => negotiate(typeof navigator === "undefined" ? [] : navigator.languages ?? [navigator.language], CODES);

/** The language a setting means: "auto" follows the browser. */
export const resolveLanguage = (setting: string) => (setting === "auto" ? browserLanguage() : shipped(setting));

let wanted = "en";

/**
 * Switches the app to a language once its strings are here. A language that
 * can't load (offline, say) leaves the app as it was; picking it again retries.
 */
export async function switchLanguage(setting: string): Promise<boolean> {
  const code = resolveLanguage(setting);
  wanted = code;
  if (code === current.locale) return true;
  let catalog: Catalog;
  try {
    catalog = await loadCatalog(code);
  } catch {
    return false;
  }
  // A later pick won while this one loaded; that one decides.
  if (wanted !== code) return true;
  current = makeI18n(code, catalog);
  if (typeof document !== "undefined") {
    document.documentElement.lang = code;
    document.documentElement.dir = current.dir;
  }
  for (const listener of listeners) listener();
  return true;
}
