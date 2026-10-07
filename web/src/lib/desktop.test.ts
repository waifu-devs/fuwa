import assert from "node:assert/strict";
import { test } from "node:test";
import { downloadsFor, downloadUrl, PACKAGE, SYSTEM, systemOf } from "./desktop.ts";

test("the system comes from the browser", () => {
  assert.equal(systemOf("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 Chrome/130.0"), "windows");
  assert.equal(systemOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Safari/605.1.15"), "macos");
  assert.equal(systemOf("Mozilla/5.0 (X11; Linux x86_64; rv:131.0) Gecko/20100101 Firefox/131.0"), "linux");
  // No desktop app on phones, tablets or ChromeOS, even an iPad that says it's a Mac.
  assert.equal(systemOf("Mozilla/5.0 (Linux; Android 14; Pixel 8) Chrome/130.0 Mobile"), null);
  assert.equal(systemOf("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)"), null);
  assert.equal(systemOf("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15", 5), null);
  assert.equal(systemOf("Mozilla/5.0 (X11; CrOS x86_64 14541.0.0) Chrome/130.0"), null);
});

test("each system gets its own downloads, the installer first", () => {
  const all = [
    { system: SYSTEM.linux, package: PACKAGE.deb, path: "deb" },
    { system: SYSTEM.windows, package: PACKAGE.setup, path: "exe" },
    { system: SYSTEM.linux, package: PACKAGE.appimage, path: "appimage" },
  ];
  assert.deepEqual(downloadsFor(all, "linux").map((d) => d.path), ["appimage", "deb"]);
  assert.deepEqual(downloadsFor(all, "windows").map((d) => d.path), ["exe"]);
  assert.deepEqual(downloadsFor(all, "macos"), []);
});

test("downloads come from the instance", () => {
  assert.equal(downloadUrl("https://fuwa.chat", "/updates/files/a.dmg"), "https://fuwa.chat/updates/files/a.dmg");
  assert.equal(downloadUrl("https://example.com/fuwa", "/updates/files/a.dmg"), "https://example.com/fuwa/updates/files/a.dmg");
});
