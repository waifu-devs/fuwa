import assert from "node:assert/strict";
import { test } from "node:test";
import { cleanDomain, hostOf, isMasked, remarkMaskedLinks, trusted } from "./links.ts";

test("links whose words are their address aren't masked", () => {
  assert.equal(isMasked("https://example.com", "https://example.com"), false);
  assert.equal(isMasked("example.com", "https://example.com/"), false);
  assert.equal(isMasked("www.example.com/a", "http://www.example.com/a"), false);
  assert.equal(isMasked("https://example.com/", "https://example.com"), false);
});

test("words that aren't the address are", () => {
  assert.equal(isMasked("free nitro", "https://phish.example"), true);
  assert.equal(isMasked("https://bank.example", "https://phish.example"), true);
  assert.equal(isMasked("example.com", "https://example.com.phish.example"), true);
});

test("trusting a site trusts what's under it", () => {
  assert.equal(hostOf("https://WWW.Example.com/x"), "example.com");
  assert.equal(cleanDomain(" https://docs.example.com/page "), "docs.example.com");
  assert.equal(cleanDomain("not a site at all"), "");
  assert.equal(trusted(["example.com"], "docs.example.com"), true);
  assert.equal(trusted(["example.com"], "example.com"), true);
  assert.equal(trusted(["example.com"], "badexample.com"), false);
  assert.equal(trusted([], ""), false);
});

test("the plugin marks only masked links", () => {
  const link = (url: string, words: string) => ({ type: "link", url, children: [{ type: "text", value: words }] }) as Record<string, unknown>;
  const masked = link("https://phish.example", "click");
  const bare = link("https://example.com", "https://example.com");
  remarkMaskedLinks()({ type: "root", children: [{ type: "paragraph", children: [masked, bare] }] } as never);
  assert.deepEqual(masked.data, { hProperties: { dataMasked: "true" } });
  assert.equal(bare.data, undefined);
});
