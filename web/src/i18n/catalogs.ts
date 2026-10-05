import { type Catalog, flatten, isTag, type Meta, type Namespace } from "./core.ts";
import englishAccountsettings from "../../../locales/en/accountsettings.json";
import englishAppsettings from "../../../locales/en/appsettings.json";
import englishCommon from "../../../locales/en/common.json";
import englishConnect from "../../../locales/en/connect.json";
import englishInstancesettings from "../../../locales/en/instancesettings.json";
import englishJoin from "../../../locales/en/join.json";
import englishServersettings from "../../../locales/en/serversettings.json";
import englishSettings from "../../../locales/en/settings.json";
import englishShell from "../../../locales/en/shell.json";

/**
 * The languages this build ships: every folder under locales/ with a meta.json.
 * English is part of every bundle; the others load when someone picks them, one
 * small same-origin file per namespace.
 */
const metas = import.meta.glob<Meta>("../../../locales/*/meta.json", { eager: true, import: "default" });
const files = import.meta.glob<Namespace>(["../../../locales/*/*.json", "!../../../locales/*/meta.json", "!../../../locales/en/*.json"], {
  import: "default",
});

const codeOf = (path: string) => path.split("/").at(-2)!;

export const ENGLISH = { accountsettings: englishAccountsettings, appsettings: englishAppsettings, common: englishCommon, connect: englishConnect, instancesettings: englishInstancesettings, join: englishJoin, serversettings: englishServersettings, settings: englishSettings, shell: englishShell } as const;
export type Namespaces = typeof ENGLISH;
export const english: Catalog = flatten(ENGLISH);

export type Language = Meta & { code: string };

export const LANGUAGES: readonly Language[] = Object.entries(metas)
  .map(([path, meta]) => ({ ...meta, code: codeOf(path) }))
  .filter((language) => isTag(language.code))
  .sort((a, b) => (a.code === "en" ? -1 : b.code === "en" ? 1 : a.english.localeCompare(b.english, "en")));

export const CODES: readonly string[] = LANGUAGES.map((language) => language.code);

/** A shipped language's code, or English for anything else (cookies, headers and settings all pass through here). */
export const shipped = (code: string | null | undefined): string => (code && CODES.includes(code) ? code : "en");

export const languageOf = (code: string): Language => LANGUAGES.find((language) => language.code === code) ?? LANGUAGES[0];

const loaded = new Map<string, Promise<Catalog>>([["en", Promise.resolve(english)]]);

/** A language's catalog (without English, which every page already has). */
export function loadCatalog(code: string): Promise<Catalog> {
  code = shipped(code);
  let catalog = loaded.get(code);
  if (!catalog) {
    const mine = Object.entries(files).filter(([path]) => codeOf(path) === code);
    catalog = Promise.all(mine.map(async ([path, load]) => [path.split("/").at(-1)!.replace(/\.json$/, ""), await load()] as const)).then(
      (namespaces) => flatten(Object.fromEntries(namespaces)),
    );
    // A failed load is forgotten, so picking the language again retries it.
    catalog.catch(() => loaded.delete(code));
    loaded.set(code, catalog);
  }
  return catalog;
}

/** How much of English a catalog covers, from 0 to 1. */
export const coverage = (catalog: Catalog) => {
  let have = 0;
  for (const key of english.keys()) if (catalog.has(key)) have++;
  return english.size ? have / english.size : 1;
};
