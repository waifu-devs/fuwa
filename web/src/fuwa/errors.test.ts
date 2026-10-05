import assert from "node:assert/strict";
import { test } from "node:test";
import { Code, ConnectError } from "@connectrpc/connect";
import { toFuwaError } from "./errors.ts";

// An instance's account is forgotten, and its encrypted messages wiped, only when the instance says the session is gone.
test("only the instance turning the session down counts as signed out, never a network blip", () => {
  const blips = [
    new TypeError("Failed to fetch"),
    new TypeError("NetworkError when attempting to fetch resource."),
    new TypeError("Load failed"),
    new ConnectError("upstream connect error", Code.Unavailable),
    new ConnectError("timed out", Code.DeadlineExceeded),
    new ConnectError("aborted", Code.Canceled),
    new ConnectError("busy", Code.ResourceExhausted),
    new ConnectError("oops", Code.Internal),
    new Error("something else"),
  ];
  for (const blip of blips) assert.equal(toFuwaError(blip).signedOut, false, String(blip));
  assert.equal(toFuwaError(new ConnectError("session expired", Code.Unauthenticated)).signedOut, true);
});
