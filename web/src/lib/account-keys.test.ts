import assert from "node:assert/strict";
import { test } from "node:test";
import { accountKeys, adoptInstanceKeys, instanceKeys, type KeyStorage } from "./account-keys.ts";

function memory(initial: Record<string, string>): KeyStorage & { data: Map<string, string> } {
  const data = new Map(Object.entries(initial));
  return {
    data,
    get length() {
      return data.size;
    },
    key: (n: number) => [...data.keys()][n] ?? null,
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => void data.set(k, v),
    removeItem: (k: string) => void data.delete(k),
  };
}

const SERVER = "01J9ZK3M4N5P6Q7R8S9T0V1W2X";

test("what was kept per instance moves to its account, and an instance whose address starts the same keeps its own", () => {
  const at = memory({
    "fuwa:applied:a.chat": "[1]",
    "fuwa.gifs.recent.a.chat": "[2]",
    [`fuwa.welcomed.a.chat.${SERVER}`]: "1",
    [`fuwa.welcomed.a.chat.evil.${SERVER}`]: "1",
    "fuwa:applied:a.chat.evil": "[3]",
  });
  adoptInstanceKeys(at, "a.chat", "a.chat|u1");
  assert.deepEqual(Object.fromEntries(at.data), {
    "fuwa:applied:a.chat|u1": "[1]",
    "fuwa.gifs.recent.a.chat|u1": "[2]",
    [`fuwa.welcomed.a.chat|u1.${SERVER}`]: "1",
    [`fuwa.welcomed.a.chat.evil.${SERVER}`]: "1",
    "fuwa:applied:a.chat.evil": "[3]",
  });
});

test("an account's own keys never take another account's, even one whose id starts the same", () => {
  const at = memory({
    "fuwa:applied:a.chat|u1": "[1]",
    "fuwa:applied:a.chat|u12": "[2]",
    [`fuwa.welcomed.a.chat|u1.${SERVER}`]: "1",
    [`fuwa.welcomed.a.chat|u12.${SERVER}`]: "1",
    "fuwa:prefs:v1": "{}",
  });
  assert.deepEqual(accountKeys(at, "a.chat|u1").sort(), [`fuwa.welcomed.a.chat|u1.${SERVER}`, "fuwa:applied:a.chat|u1"]);
});

test("an instance's keys are every account's on it and its old ones, and nothing of another instance", () => {
  const at = memory({
    "fuwa:applied:a.chat|u1": "[1]",
    "fuwa.gifs.recent.a.chat|u2": "[2]",
    "fuwa:applied:a.chat": "[0]",
    [`fuwa.welcomed.a.chat.${SERVER}`]: "1",
    [`fuwa.welcomed.a.chat.evil.${SERVER}`]: "1",
    "fuwa:applied:a.chat.evil|u1": "[3]",
    "fuwa:prefs:v1": "{}",
  });
  assert.deepEqual(instanceKeys(at, "a.chat").sort(), [
    "fuwa.gifs.recent.a.chat|u2",
    `fuwa.welcomed.a.chat.${SERVER}`,
    "fuwa:applied:a.chat",
    "fuwa:applied:a.chat|u1",
  ]);
});
