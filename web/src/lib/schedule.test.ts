import assert from "node:assert/strict";
import { mock, test } from "node:test";
import { afterWhileVisible, clockNow, everyWhileVisible, runningClocks, subscribeClock } from "./schedule.ts";

// A page whose visibility the tests change.
const listeners: (() => void)[] = [];
const page = { hidden: false, addEventListener: (_: string, fn: () => void) => listeners.push(fn) };
(globalThis as { document?: unknown }).document = page;
const setHidden = (hidden: boolean) => {
  page.hidden = hidden;
  for (const l of listeners) l();
};

test("runs every period while shown, pauses while hidden, catches up when shown", () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  try {
    let runs = 0;
    const stop = everyWhileVisible(1000, () => runs++);
    mock.timers.tick(1000);
    mock.timers.tick(1000);
    assert.equal(runs, 2);
    setHidden(true);
    mock.timers.tick(5000);
    assert.equal(runs, 2);
    setHidden(false);
    assert.equal(runs, 3, "a run came due while hidden: it runs when shown");
    mock.timers.tick(999);
    assert.equal(runs, 3);
    mock.timers.tick(1);
    assert.equal(runs, 4);
    stop();
    mock.timers.tick(5000);
    assert.equal(runs, 4);
  } finally {
    mock.timers.reset();
  }
});

test("keeps running slower while hidden when asked", () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  try {
    let runs = 0;
    const stop = everyWhileVisible(1000, () => runs++, { hiddenMs: 10_000 });
    setHidden(true);
    mock.timers.tick(9_999);
    assert.equal(runs, 0);
    mock.timers.tick(1);
    assert.equal(runs, 1);
    setHidden(false);
    stop();
  } finally {
    mock.timers.reset();
  }
});

test("a delay that ends while hidden waits to be shown", () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  try {
    let done = 0;
    afterWhileVisible(1000, () => done++);
    setHidden(true);
    mock.timers.tick(2000);
    assert.equal(done, 0);
    setHidden(false);
    assert.equal(done, 1);
    setHidden(true);
    setHidden(false);
    assert.equal(done, 1);
  } finally {
    mock.timers.reset();
  }
});

test("a shared clock: one timer however many listen, gone when nobody does", () => {
  mock.timers.enable({ apis: ["setTimeout", "Date"], now: 0 });
  try {
    let a = 0;
    let b = 0;
    const offA = subscribeClock(500, () => a++);
    const offB = subscribeClock(500, () => b++);
    assert.equal(runningClocks(), 1);
    mock.timers.tick(500);
    assert.equal(a, 1);
    assert.equal(b, 1);
    assert.equal(clockNow(500), 500);
    offA();
    assert.equal(runningClocks(), 1);
    offB();
    assert.equal(runningClocks(), 0);
    // Asked twice with nobody listening: the same answer.
    mock.timers.tick(100);
    assert.equal(clockNow(500), clockNow(500));
  } finally {
    mock.timers.reset();
  }
});
