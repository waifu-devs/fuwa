import { Fragment, type ReactNode, useSyncExternalStore } from "react";
import { english } from "./catalogs.ts";
import { parts, template } from "./core.ts";
import { type I18n, i18n, type Key, subscribeI18n } from "./i18n.ts";

export type { I18n, Key };

/** The app's language, its strings and its number and date formats; re-renders when the language changes. */
export function useI18n(): I18n {
  return useSyncExternalStore(subscribeI18n, i18n);
}

/**
 * A string with elements in its placeholders, for the few that need bold text
 * or a link inside: <T k="..." values={{ count: <b>{n}</b> }} />. The catalog
 * stays plain text; the elements come from the app.
 */
export function T({ k, values = {} }: { k: Key; values?: Record<string, ReactNode> }) {
  const { locale, catalog, number } = useI18n();
  const count = typeof values.count === "number" ? values.count : undefined;
  return (
    <>
      {parts(template(locale, catalog, english, k, count)).map((part, n) => {
        if (typeof part === "string") return <Fragment key={n}>{part}</Fragment>;
        const value = values[part.name];
        return <Fragment key={n}>{Object.hasOwn(values, part.name) ? (typeof value === "number" ? number(value) : value) : `{${part.name}}`}</Fragment>;
      })}
    </>
  );
}
