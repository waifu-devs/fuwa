import assert from "node:assert/strict";
import { test } from "node:test";
import { codeLeft, findShareCode, foreignServer, listNames, shareCodeInstance, sharedLabel } from "./shared.ts";

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
  assert.equal(listNames([]), "");
  assert.equal(listNames(["Cats"]), "Cats");
  assert.equal(listNames(["Cats", "Dogs"]), "Cats and Dogs");
  assert.equal(listNames(["Cats", "Dogs", "Birds"]), "Cats, Dogs and Birds");
});

const server = (id: string, name: string) => ({ id, name, iconUrl: "" }) as never;

test("the label says which way a channel is shared", () => {
  assert.equal(sharedLabel({ shared: undefined }), null);
  assert.deepEqual(sharedLabel({ shared: { home: true, guests: [server("g", "Guests")], homeChannelName: "", homeServer: undefined } as never }), {
    home: true,
    names: "Guests",
    text: "Shared with Guests",
  });
  assert.equal(sharedLabel({ shared: { home: false, guests: [], homeServer: server("h", "Home"), homeChannelName: "general" } as never })?.text, "Shared from Home");
});

test("only authors from another server get a tag", () => {
  assert.equal(foreignServer({ shared: undefined }, "here"), null);
  assert.equal(foreignServer({ shared: { server: server("here", "Us") } as never }, "here"), null);
  assert.equal(foreignServer({ shared: { server: server("there", "Them") } as never }, "here")?.name, "Them");
});

test("time left on a code reads plainly", () => {
  assert.equal(codeLeft(7 * 24 * 3_600_000), "7 days");
  assert.equal(codeLeft(7 * 24 * 3_600_000 - 60_000), "7 days");
  assert.equal(codeLeft(5 * 3_600_000), "5 hours");
  assert.equal(codeLeft(30 * 60_000), "30 minutes");
  assert.equal(codeLeft(60_000), "a few minutes");
  assert.equal(codeLeft(0), "expired");
});
