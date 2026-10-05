import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { fill, flatten, type Namespace, template } from "../i18n/core.ts";
import type { I18n } from "../i18n/i18n.ts";
import { activeAgo, activeWhen, describeDevice, deviceName } from "./devices.ts";
import { ago, formatBytes, formatDuration, formatLeft, type Lang, roughly, shortDuration } from "./durations.ts";

/** The app's strings in a language, as the page has them (i18n/i18n.ts needs Vite, so this reads the catalogs itself). */
const common = (code: string) => JSON.parse(readFileSync(new URL(`../../../locales/${code}/common.json`, import.meta.url), "utf8")) as Namespace;
const english = flatten({ common: common("en") });
function lang(locale: string): Lang {
  const catalog = locale === "en" ? english : flatten({ common: common(locale) });
  const t: I18n["t"] = (key, values = {}) =>
    fill(
      template(locale, catalog, english, key, typeof values.count === "number" ? values.count : undefined),
      Object.fromEntries(Object.entries(values).map(([k, v]) => [k, typeof v === "number" ? new Intl.NumberFormat(locale).format(v) : v])),
    );
  return { t, locale };
}
const en = lang("en");
const es = lang("es");

const NOW = Date.UTC(2026, 9, 4, 16, 20, 30);
const before = (ms: number) => new Date(NOW - ms);
const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

test("durations read in their largest whole unit, as before in English", () => {
  assert.equal(formatDuration(en, 30), "30 seconds");
  assert.equal(formatDuration(en, 60), "1 minute");
  assert.equal(formatDuration(en, 300), "5 minutes");
  assert.equal(formatDuration(en, 3600), "1 hour");
  assert.equal(formatDuration(en, 7 * 86_400), "7 days");
  assert.equal(formatDuration(en, 90), "90 seconds");
  assert.equal(shortDuration(en, 30), "30s");
  assert.equal(shortDuration(en, 300), "5m");
  assert.equal(shortDuration(en, 3600), "1h");
  assert.equal(shortDuration(en, 7 * 86_400), "7d");
});

test("durations in Spanish", () => {
  assert.equal(formatDuration(es, 30), "30 segundos");
  assert.equal(formatDuration(es, 60), "1 minuto");
  assert.equal(formatDuration(es, 7200), "2 horas");
  assert.equal(formatDuration(es, 86_400), "1 día");
  assert.match(shortDuration(es, 300), /^5\s?min$/);
});

test("roughly and ago round down to the largest unit", () => {
  assert.equal(roughly(en, 30_000), "less than a minute");
  assert.equal(roughly(en, 5 * MINUTE), "5 minutes");
  assert.equal(roughly(en, 3 * DAY + HOUR), "3 days");
  assert.equal(roughly(en, 65 * DAY), "2 months");
  assert.equal(roughly(en, 800 * DAY), "2 years");
  assert.equal(ago(en, before(10_000), NOW), "just now");
  assert.equal(ago(en, before(MINUTE), NOW), "1 minute ago");
  assert.equal(ago(en, before(5 * MINUTE), NOW), "5 minutes ago");
  assert.equal(ago(en, before(26 * HOUR), NOW), "1 day ago");
  assert.equal(ago(en, before(400 * DAY), NOW), "1 year ago");
  assert.equal(roughly(es, 30_000), "menos de un minuto");
  assert.equal(roughly(es, 3 * DAY), "3 días");
  assert.equal(ago(es, before(10_000), NOW), "hace un momento");
  assert.equal(ago(es, before(5 * MINUTE), NOW), "hace 5 minutos");
  assert.equal(ago(es, before(65 * DAY), NOW), "hace 2 meses");
});

test("countdowns", () => {
  assert.equal(formatLeft(en, 42_000), "0:42");
  assert.equal(formatLeft(en, 12 * MINUTE + 5_000), "12:05");
  assert.equal(formatLeft(en, 3 * HOUR + 20 * MINUTE), "3h 20m");
  assert.equal(formatLeft(en, 2 * DAY + 4 * HOUR), "2d 4h");
  assert.equal(formatLeft(es, 42_000), "0:42");
  assert.match(formatLeft(es, 3 * HOUR + 20 * MINUTE), /^3\s?h 20\s?min$/);
});

test("sizes keep their units and take the language's decimal mark", () => {
  assert.equal(formatBytes(en, 512), "512 B");
  assert.equal(formatBytes(en, 1023), "1023 B");
  assert.equal(formatBytes(en, 1536), "1.5 KB");
  assert.equal(formatBytes(en, 2 * 1024 * 1024), "2.0 MB");
  assert.equal(formatBytes(en, 25 * 1024 * 1024), "25 MB");
  assert.equal(formatBytes(en, 3 * 1024 ** 4), "3.0 TB");
  assert.equal(formatBytes(es, 1536), "1,5 KB");
  assert.equal(formatBytes(es, 25 * 1024 * 1024), "25 MB");
});

const FIREFOX_MAC = "Mozilla/5.0 (Macintosh; Intel Mac OS X 14.5; rv:131.0) Gecko/20100101 Firefox/131.0";
const SAFARI_IPHONE = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

test("devices are named in the app's language", () => {
  assert.deepEqual(describeDevice(FIREFOX_MAC), { browser: "Firefox", os: "macOS", kind: "computer" });
  assert.equal(describeDevice(SAFARI_IPHONE).kind, "phone");
  assert.equal(deviceName(en.t, describeDevice(FIREFOX_MAC)), "Firefox on macOS");
  assert.equal(deviceName(en.t, describeDevice(SAFARI_IPHONE)), "Safari on iPhone");
  assert.equal(deviceName(en.t, describeDevice("")), "An unknown device");
  assert.equal(deviceName(en.t, describeDevice("curl/8.5")), "An app or script");
  assert.equal(describeDevice("curl/8.5").kind, "tool");
  assert.equal(deviceName(en.t, describeDevice("fuwa-desktop/0.3 (Windows)")), "fuwa desktop on Windows");
  assert.equal(deviceName(es.t, describeDevice(FIREFOX_MAC)), "Firefox en macOS");
  assert.equal(deviceName(es.t, describeDevice("")), "Un dispositivo desconocido");
  assert.equal(deviceName(es.t, describeDevice("python-requests/2.32")), "Una app o un script");
});

test("when a device was last active", () => {
  assert.equal(activeWhen(en, before(3 * MINUTE), NOW), null);
  assert.equal(activeAgo(en, before(3 * MINUTE), NOW), "Active now");
  assert.equal(activeAgo(en, before(20 * MINUTE), NOW), "Active 20 minutes ago");
  assert.equal(activeAgo(en, before(3 * HOUR), NOW), "Active 3 hours ago");
  assert.equal(activeAgo(en, before(DAY), NOW), "Active yesterday");
  assert.equal(activeAgo(en, before(40 * DAY), NOW), "Active last month");
  assert.equal(activeAgo(es, before(3 * MINUTE), NOW), "Activo ahora");
  assert.equal(activeAgo(es, before(20 * MINUTE), NOW), "Activo hace 20 minutos");
  assert.equal(activeAgo(es, before(DAY), NOW), "Activo ayer");
});
