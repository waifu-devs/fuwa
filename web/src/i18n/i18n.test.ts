// The translation runtime (core.ts) and every catalog under locales/ at the repo root.
// The site (waifu-devs/site) runs the same runtime tests; keep the two in step.
import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { acceptLanguage, fill, flatten, isTag, negotiate, type Namespace, parts, pickPlural, placeholders, template } from "./core.ts";

test("fills placeholders and leaves unknown ones visible", () => {
  assert.equal(fill("Joined {server}", { server: "Cozy" }), "Joined Cozy");
  assert.equal(fill("Hi {name}, {name}!", { name: "Ai" }), "Hi Ai, Ai!");
  assert.equal(fill("Hi {name}"), "Hi {name}");
  // Only the values given: nothing inherited from Object.prototype.
  assert.equal(fill("{constructor} {toString}", {}), "{constructor} {toString}");
  assert.equal(fill("{Not} a {place-holder}"), "{Not} a {place-holder}");
  // One pass: values are never read as placeholders themselves.
  assert.equal(fill("Joined {server} with {count}", { server: "{count}", count: "3" }), "Joined {count} with 3");
});

test("splits templates into text and placeholders", () => {
  assert.deepEqual(parts("Made with {heart} by us"), ["Made with ", { name: "heart" }, " by us"]);
  assert.deepEqual(parts("{a}{b}"), [{ name: "a" }, { name: "b" }]);
  assert.deepEqual(parts("plain"), ["plain"]);
});

test("picks plural forms by the language's rules, falling back to other", () => {
  const forms = { one: "{count} member", other: "{count} members" };
  assert.equal(pickPlural("en", forms, 1), "{count} member");
  assert.equal(pickPlural("en", forms, 0), "{count} members");
  assert.equal(pickPlural("en", forms, 2), "{count} members");
  // Spanish says "many" for exact millions; a catalog without it gets "other".
  assert.equal(pickPlural("es", forms, 1_000_000), "{count} members");
  assert.equal(pickPlural("es", { ...forms, many: "{count} de miembros" }, 1_000_000), "{count} de miembros");
  // Polish has few and many.
  const pl = { one: "1 plik", few: "{count} pliki", many: "{count} plików", other: "{count} pliku" };
  assert.equal(pickPlural("pl", pl, 3), "{count} pliki");
  assert.equal(pickPlural("pl", pl, 5), "{count} plików");
});

test("looks keys up in the language, then English, then shows the key", () => {
  const en = flatten({ common: { save: "Save", files: { one: "{count} file", other: "{count} files" } } });
  const es = flatten({ common: { save: "Guardar" } });
  assert.equal(template("es", es, en, "common.save"), "Guardar");
  assert.equal(template("es", es, en, "common.files", 1), "{count} file");
  assert.equal(template("es", es, en, "common.nope"), "common.nope");
});

test("negotiates the closest shipped language", () => {
  const shipped = ["en", "es", "pt-BR"];
  assert.equal(negotiate(["es-MX", "en"], shipped), "es");
  assert.equal(negotiate(["pt-br"], shipped), "pt-BR");
  assert.equal(negotiate(["fr", "de"], shipped), "en");
  assert.equal(negotiate(["*"], shipped), "en");
  assert.equal(negotiate(["es_ES"], shipped), "es");
});

test("reads Accept-Language by weight, bounded", () => {
  assert.deepEqual(acceptLanguage("fr;q=0.5, es-MX, en;q=0.8"), ["es-MX", "en", "fr"]);
  assert.deepEqual(acceptLanguage("de;q=0, it"), ["it"]);
  assert.deepEqual(acceptLanguage(null), []);
  assert.ok(acceptLanguage("en,".repeat(10_000)).length <= 16);
});

test("only takes tags shaped like ours", () => {
  for (const ok of ["en", "es", "pt-BR", "zh-Hant", "fil"]) assert.ok(isTag(ok), ok);
  for (const bad of ["", "EN", "en-", "../en", "en/../../x", "en-us", "a".repeat(20), "es\n"]) assert.ok(!isTag(bad), bad);
});

// ---------------------------------------------------------------------------
// The catalogs themselves

const root = join(import.meta.dirname, "..", "..", "..", "locales");
const RULES = new Set(["other", "one-other", "one-many-other-romance", "one-few-many-other-slavic", "one-few-other-romanian", "zero-one-other-latvian", "one-two-few-many-other-arabic", "one-two-other", "one-few-many-other-polish"]);
const read = (path: string) => JSON.parse(readFileSync(path, "utf8"));
const codes = readdirSync(root, { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => d.name);
const english = Object.fromEntries(
  readdirSync(join(root, "en"))
    .filter((f) => f !== "meta.json" && f.endsWith(".json"))
    .map((f) => [f.replace(/\.json$/, ""), read(join(root, "en", f)) as Namespace]),
);

test("English is there and every language folder is a tag with a meta.json", () => {
  assert.ok(codes.includes("en"));
  for (const code of codes) {
    assert.ok(isTag(code), `locales/${code} isn't a language tag`);
    const metaPath = join(root, code, "meta.json");
    assert.ok(existsSync(metaPath), `locales/${code}/meta.json is missing`);
    const meta = read(metaPath);
    assert.equal(typeof meta.name, "string");
    assert.equal(typeof meta.english, "string");
    assert.ok(meta.dir === "ltr" || meta.dir === "rtl", `${code}: dir`);
    assert.ok(RULES.has(meta.plural), `${code}: unknown plural rule ${meta.plural}`);
    assert.equal(typeof meta.reviewed, "boolean");
  }
});

test("every translation matches English's keys, placeholders and plural shape", () => {
  for (const code of codes) {
    for (const file of readdirSync(join(root, code))) {
      if (file === "meta.json") continue;
      const ns = file.replace(/\.json$/, "");
      const source = english[ns];
      assert.ok(source, `locales/${code}/${file}: English has no ${file}`);
      const entries = read(join(root, code, file)) as Namespace;
      for (const [key, entry] of Object.entries(entries)) {
        const where = `locales/${code}/${file} "${key}"`;
        assert.ok(key in source, `${where}: English has no such key`);
        const want = source[key];
        assert.equal(typeof entry, typeof want, `${where}: plural in one and not the other`);
        if (typeof entry === "object") {
          assert.equal(typeof entry.other, "string", `${where}: plural needs "other"`);
          const rule = new Intl.PluralRules(code).resolvedOptions().pluralCategories;
          for (const category of Object.keys(entry)) assert.ok(rule.includes(category as Intl.LDMLPluralRule), `${where}: ${code} has no "${category}"`);
        }
        assert.deepEqual(placeholders(entry), placeholders(want), `${where}: placeholders differ from English`);
        for (const text of typeof entry === "string" ? [entry] : Object.values(entry)) {
          assert.ok(!/<[a-z/]/i.test(text ?? ""), `${where}: no markup in catalogs`);
          // Some lines go through Markdown, so no links or images either.
          assert.ok(!/\]\(|!\[/.test(text ?? ""), `${where}: no Markdown links or images in catalogs`);
        }
      }
    }
  }
});
