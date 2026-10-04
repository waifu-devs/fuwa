import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clearRecentSearches,
  hasValue,
  markRanges,
  MARK_CLOSE,
  MARK_OPEN,
  parseDay,
  parseQuery,
  recentSearches,
  rememberSearch,
  replaceToken,
  timeRange,
  tokenAt,
} from "./search-query.ts";

test("words and filters come apart", () => {
  const parsed = parseQuery("from:mika cake In:general has:link  pie");
  assert.equal(parsed.text, "cake pie");
  assert.deepEqual(
    parsed.filters.map((f) => [f.key, f.value]),
    [
      ["from", "mika"],
      ["in", "general"],
      ["has", "link"],
    ],
  );
  assert.equal(parseQuery("from: cake").text, "from: cake", "a key with nothing after it is a word for now");
  assert.equal(parseQuery("http://example.com").text, "http://example.com", "not every colon is a filter");
  assert.equal(parseQuery("cake ").text, "cake ", "a space after keeps the last word finished");
});

test("the word at the caret, and replacing it", () => {
  assert.deepEqual(tokenAt("from:mi cake", 4), { start: 0, end: 7, text: "from:mi" });
  assert.deepEqual(tokenAt("cake ", 5), { start: 5, end: 5, text: "" });
  assert.deepEqual(replaceToken("cake from:mi", 12, "from:mika"), { input: "cake from:mika ", caret: 15 });
  assert.deepEqual(replaceToken("in:gen cake", 3, "in:general"), { input: "in:general cake", caret: 11 });
});

test("has: takes a few spellings", () => {
  assert.equal(hasValue("Image"), "picture");
  assert.equal(hasValue("@here"), "everyone");
  assert.equal(hasValue("poll"), null);
});

test("dates and the time they cover", () => {
  const now = new Date(2026, 9, 4, 15, 30);
  assert.deepEqual(parseDay("today", now), new Date(2026, 9, 4));
  assert.deepEqual(parseDay("yesterday", now), new Date(2026, 9, 3));
  assert.deepEqual(parseDay("2026-02-28", now), new Date(2026, 1, 28));
  assert.equal(parseDay("2026-02-30", now), null);
  assert.equal(parseDay("soon", now), null);

  const range = (q: string) => timeRange(parseQuery(q).filters, now);
  assert.deepEqual(range("during:2026-10-01"), { after: new Date(2026, 9, 1), before: new Date(2026, 9, 2) });
  assert.deepEqual(range("after:2026-10-01"), { after: new Date(2026, 9, 2), before: undefined });
  assert.deepEqual(range("before:today before:2026-10-01"), { after: undefined, before: new Date(2026, 9, 1) });
  assert.equal(range("before:someday").bad?.value, "someday");
});

test("recent searches stay on the device, newest first", () => {
  const kept = new Map<string, string>();
  const store = { getItem: (k: string) => kept.get(k) ?? null, setItem: (k: string, v: string) => void kept.set(k, v) };
  for (const q of ["a", "b", "a", "c", "d", "e", "f", "g"]) rememberSearch("here", q, false, store);
  assert.deepEqual(recentSearches("here", store), ["g", "f", "e", "d", "c", "a"]);
  assert.deepEqual(recentSearches("elsewhere", store), []);
  rememberSearch("here", "e", true, store);
  assert.deepEqual(recentSearches("here", store), ["g", "f", "d", "c", "a"]);
  clearRecentSearches("here", store);
  assert.deepEqual(recentSearches("here", store), []);
  kept.set("fuwa:search:recent:v1", "not json");
  assert.deepEqual(recentSearches("here", store), []);
});

test("highlights are marked by UTF-16 ranges", () => {
  assert.equal(markRanges("🌸 Café world", [{ start: 3, end: 7 }]), `🌸 ${MARK_OPEN}Café${MARK_CLOSE} world`);
  assert.equal(markRanges("abc", [{ start: 2, end: 9 }]), "abc", "ranges past the end are left out");
});
