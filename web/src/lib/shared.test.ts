import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fill, flatten, type Namespace, template } from "../i18n/core.ts";
import type { I18n } from "../i18n/i18n.ts";
import type { Lang } from "./durations.ts";
import { codeLeft, findShareCode, foreignServer, listNames, shareCodeInstance, sharedLabel } from "./shared.ts";

/** The server settings strings in a language, as the page has them (i18n/i18n.ts needs Vite, so this reads the catalogs itself). */
const serversettings = (code: string) =>
  JSON.parse(readFileSync(new URL(`../../../locales/${code}/serversettings.json`, import.meta.url), "utf8")) as Namespace;
const chat = (code: string) => {
  try {
    return JSON.parse(readFileSync(new URL(`../../../locales/${code}/chat.json`, import.meta.url), "utf8")) as Namespace;
  } catch {
    return {};
  }
};
const english = flatten({ serversettings: serversettings("en"), chat: chat("en") });
function lang(locale: string): Lang {
  const catalog = locale === "en" ? english : flatten({ serversettings: serversettings(locale), chat: chat(locale) });
  const t: I18n["t"] = (key, values = {}) =>
    fill(
      template(locale, catalog, english, key, typeof values.count === "number" ? values.count : undefined),
      Object.fromEntries(Object.entries(values).map(([k, v]) => [k, String(v)])),
    );
  return { t, locale };
}
const en = lang("en");
const es = lang("es");

// Run with `pnpm test` (node's own test runner; no extra dependencies).

const CODE = "01JABCDEFGHJKMNPQRSTVWXYZ0-AbCdEfGh23456789";

test("a share code is found in whatever was pasted around it", () => {
  assert.equal(findShareCode(CODE), CODE);
  assert.equal(findShareCode(`  here you go: ${CODE}\n`), CODE);
  assert.equal(findShareCode("not a code"), "");
  // Too short after the dash.
  assert.equal(findShareCode("01JABCDEFGHJKMNPQRSTVWXYZ0-short"), "");
});

test("a code for other instances keeps the instance it names", () => {
  const remote = `${CODE}@chat.example.com`;
  assert.equal(findShareCode(`join us: ${remote}.`), remote);
  assert.equal(findShareCode(`${CODE}@chat.example.com:8443 thanks`), `${CODE}@chat.example.com:8443`);
  assert.equal(findShareCode(`${CODE}@http://127.0.0.1:4000`), `${CODE}@http://127.0.0.1:4000`);
  assert.equal(shareCodeInstance(remote), "chat.example.com");
  assert.equal(shareCodeInstance(`${CODE}@http://127.0.0.1:4000`), "http://127.0.0.1:4000");
  assert.equal(shareCodeInstance(CODE), "");
});

test("names read like a sentence", () => {
  assert.equal(listNames(en, []), "");
  assert.equal(listNames(en, ["Cats"]), "Cats");
  assert.equal(listNames(en, ["Cats", "Dogs"]), "Cats and Dogs");
  assert.equal(listNames(en, ["Cats", "Dogs", "Birds"]), "Cats, Dogs and Birds");
});

const server = (id: string, name: string) => ({ id, name, iconUrl: "" }) as never;

test("the label says which way a channel is shared", () => {
  assert.equal(sharedLabel(en, { shared: undefined }), null);
  assert.deepEqual(sharedLabel(en, { shared: { home: true, guests: [server("g", "Guests")], homeChannelName: "", homeServer: undefined } as never }), {
    home: true,
    names: "Guests",
    text: "Shared with Guests",
  });
  assert.equal(sharedLabel(en, { shared: { home: false, guests: [], homeServer: server("h", "Home"), homeChannelName: "general" } as never })?.text, "Shared from Home");
});

test("only authors from another server get a tag", () => {
  assert.equal(foreignServer({ shared: undefined }, "here"), null);
  assert.equal(foreignServer({ shared: { server: server("here", "Us") } as never }, "here"), null);
  assert.equal(foreignServer({ shared: { server: server("there", "Them") } as never }, "here")?.name, "Them");
});

test("time left on a code reads plainly", () => {
  assert.equal(codeLeft(en, 7 * 24 * 3_600_000), "7 days");
  assert.equal(codeLeft(en, 7 * 24 * 3_600_000 - 60_000), "7 days");
  assert.equal(codeLeft(en, 30 * 3_600_000), "30 hours");
  assert.equal(codeLeft(en, 5 * 3_600_000), "5 hours");
  assert.equal(codeLeft(en, 30 * 60_000), "30 minutes");
  assert.equal(codeLeft(en, 60_000), "a few minutes");
  assert.equal(codeLeft(en, 0), "expired");
});

test("time left on a code reads in the app's language", () => {
  assert.equal(codeLeft(es, 7 * 24 * 3_600_000), "7 días");
  assert.equal(codeLeft(es, 5 * 3_600_000), "5 horas");
  assert.equal(codeLeft(es, 30 * 60_000), "30 minutos");
});
