/**
 * Lengths of time and sizes, in the app's language: Intl does the units and
 * digits, the catalog (common.time.*) the few phrases Intl has no words for.
 * Callers pass what useI18n gives them (or i18n() outside React); lib/format
 * re-exports these, and they import nothing that needs Vite, so node's test
 * runner checks them directly.
 */
import type { I18n } from "../i18n/i18n.ts";

/** The language to write in: its strings and its code (for Intl). */
export type Lang = Pick<I18n, "t" | "locale">;

type Unit = "second" | "minute" | "hour" | "day" | "month" | "year";

// Building a formatter is the slow part, and lists reuse the same few.
const numberFormats = new Map<string, Intl.NumberFormat>();
const relativeFormats = new Map<string, Intl.RelativeTimeFormat>();

function numberFormat(locale: string, options: Intl.NumberFormatOptions) {
  const key = `${locale}|${JSON.stringify(options)}`;
  let f = numberFormats.get(key);
  if (!f) numberFormats.set(key, (f = new Intl.NumberFormat(locale, options)));
  return f;
}

/** "5 minutes", or "5m" when narrow. */
const unit = (locale: string, n: number, u: Unit, display: "long" | "narrow" = "long") =>
  numberFormat(locale, { style: "unit", unit: u, unitDisplay: display }).format(n);

/** "5 minutes ago", "yesterday" (with `auto`). */
export function relative(locale: string, n: number, u: Unit, numeric: "always" | "auto" = "always") {
  const key = `${locale}|${numeric}`;
  let f = relativeFormats.get(key);
  if (!f) relativeFormats.set(key, (f = new Intl.RelativeTimeFormat(locale, { numeric })));
  return f.format(n, u);
}

const UNITS = [
  { seconds: 86_400, unit: "day" },
  { seconds: 3_600, unit: "hour" },
  { seconds: 60, unit: "minute" },
  { seconds: 1, unit: "second" },
] as const;

function largest(seconds: number) {
  const u = UNITS.find((u) => seconds >= u.seconds && seconds % u.seconds === 0) ?? UNITS[3];
  return { n: Math.round(seconds / u.seconds), unit: u.unit };
}

/** A length of time in its largest whole unit: "30 seconds", "5 minutes", "1 hour", "7 days". */
export function formatDuration({ locale }: Lang, seconds: number) {
  const { n, unit: u } = largest(seconds);
  return unit(locale, n, u);
}

/** The same, clipped for a chip: "30s", "5m", "1h", "7d". */
export function shortDuration({ locale }: Lang, seconds: number) {
  const { n, unit: u } = largest(seconds);
  return unit(locale, n, u, "narrow");
}

/** How long something has been around, rounded down to its largest unit, and how many of it. */
function rough(ms: number): { n: number; unit: Unit } | null {
  const minutes = Math.floor(Math.max(0, ms) / 60_000);
  if (minutes < 1) return null;
  if (minutes < 60) return { n: minutes, unit: "minute" };
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return { n: hours, unit: "hour" };
  const days = Math.floor(hours / 24);
  if (days < 30) return { n: days, unit: "day" };
  if (days < 365) return { n: Math.floor(days / 30), unit: "month" };
  return { n: Math.floor(days / 365), unit: "year" };
}

/** How long something has been around: "less than a minute", "5 minutes", "3 days", "2 months". */
export function roughly({ t, locale }: Lang, ms: number) {
  const r = rough(ms);
  return r ? unit(locale, r.n, r.unit) : t("common.time.lessThanMinute");
}

/** "just now", "5 minutes ago", "3 days ago". */
export function ago({ t, locale }: Lang, date: Date, now = Date.now()) {
  const r = now - date.getTime() < 60_000 ? null : rough(now - date.getTime());
  return r ? relative(locale, -r.n, r.unit) : t("common.time.justNow");
}

/** Time left, as a countdown: "0:42", "12:05", "3h 20m", "2d 4h". */
export function formatLeft({ locale }: Lang, ms: number) {
  const s = Math.max(0, Math.ceil(ms / 1000));
  if (s < 3600) return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
  if (s < 86_400) return `${unit(locale, Math.floor(s / 3600), "hour", "narrow")} ${unit(locale, Math.floor((s % 3600) / 60), "minute", "narrow")}`;
  return `${unit(locale, Math.floor(s / 86_400), "day", "narrow")} ${unit(locale, Math.floor((s % 86_400) / 3600), "hour", "narrow")}`;
}

const BYTE_UNITS = ["KB", "MB", "GB", "TB"];

/** "512 B", "1.5 MB", "12 GB", with the language's decimal mark. */
export function formatBytes({ locale }: Lang, bytes: number) {
  const digits = (n: number, fraction: number) =>
    numberFormat(locale, { minimumFractionDigits: fraction, maximumFractionDigits: fraction, useGrouping: false }).format(n);
  if (bytes < 1024) return `${digits(bytes, 0)} B`;
  let value = bytes / 1024;
  let u = 0;
  while (value >= 1024 && u < BYTE_UNITS.length - 1) {
    value /= 1024;
    u++;
  }
  return `${digits(value, value >= 10 ? 0 : 1)} ${BYTE_UNITS[u]}`;
}
