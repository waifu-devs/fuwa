import assert from "node:assert/strict";
import { test } from "node:test";
import { draftKey, forgetDrafts, getDraft, setDraft } from "./drafts.ts";

test("drafts are kept per account: another account on the same instance and channel sees none", () => {
  setDraft(draftKey("fuwa.chat|u1", "c1"), "hello from one");
  assert.equal(getDraft(draftKey("fuwa.chat|u2", "c1")), "");
  assert.equal(getDraft(draftKey("fuwa.chat|u1", "c1")), "hello from one");
});

test("forgetting an account's drafts leaves other accounts' alone, even one whose id starts the same", () => {
  setDraft(draftKey("fuwa.chat|u1", "c1"), "one");
  setDraft(draftKey("fuwa.chat|u12", "c1"), "twelve");
  setDraft(draftKey("other.chat|u1", "c1"), "elsewhere");
  forgetDrafts("fuwa.chat|u1");
  assert.equal(getDraft(draftKey("fuwa.chat|u1", "c1")), "");
  assert.equal(getDraft(draftKey("fuwa.chat|u12", "c1")), "twelve");
  assert.equal(getDraft(draftKey("other.chat|u1", "c1")), "elsewhere");
  forgetDrafts("fuwa.chat|");
  assert.equal(getDraft(draftKey("fuwa.chat|u12", "c1")), "", "forgetting the instance takes every account's");
  assert.equal(getDraft(draftKey("other.chat|u1", "c1")), "elsewhere");
});
