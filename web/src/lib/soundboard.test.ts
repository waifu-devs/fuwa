import assert from "node:assert/strict";
import { test } from "node:test";
import {
  clampWindow,
  framesToMs,
  freeName,
  heardHere,
  MAX_SOUND_MS,
  playBlocked,
  playVolume,
  readEmoji,
  soundEmoji,
  soundNameFromFile,
  sortSounds,
  trimPackets,
  validName,
  windowFrames,
} from "./soundboard.ts";

const call = { kind: "voice" as const, instance: "fuwa.chat", serverId: "s1", channelId: "c1" };

test("a played sound is heard only in that voice channel's call, and not deafened", () => {
  assert.equal(heardHere(call, "fuwa.chat", "s1", "c1", false), true);
  assert.equal(heardHere(call, "fuwa.chat", "s1", "c1", true), false);
  assert.equal(heardHere(call, "fuwa.chat", "s1", "c2", false), false);
  assert.equal(heardHere(call, "other.example", "s1", "c1", false), false);
  assert.equal(heardHere({ kind: "dm" }, "fuwa.chat", "s1", "c1", false), false);
  assert.equal(heardHere(null, "fuwa.chat", "s1", "c1", false), false);
});

test("a sound plays at its volume times yours, kept between 0 and 1", () => {
  assert.equal(playVolume(0.5, 1), 0.5);
  assert.equal(playVolume(0.5, 0.5), 0.25);
  assert.equal(playVolume(2, 3), 1);
  assert.equal(playVolume(-1, 1), 0);
  assert.equal(playVolume(Number.NaN, 0.5), 0.5);
});

test("says why a sound can't play, the moderator's reasons first", () => {
  const fine = { connected: true, speak: true, selfMute: false, selfDeaf: false, serverMute: false, serverDeaf: false, suppress: false };
  assert.equal(playBlocked(fine), null);
  assert.equal(playBlocked({ ...fine, selfMute: true }), "muted");
  assert.equal(playBlocked({ ...fine, selfMute: true, selfDeaf: true }), "deafened");
  assert.equal(playBlocked({ ...fine, selfMute: true, serverMute: true }), "serverMuted");
  assert.equal(playBlocked({ ...fine, speak: false }), "noSpeak");
  assert.equal(playBlocked({ ...fine, connected: false }), "connecting");
});

test("the kept part fits in the file and is at most ten seconds", () => {
  assert.deepEqual(clampWindow(4_000, 0), { startMs: 0, endMs: 4_000 });
  assert.deepEqual(clampWindow(4_000, 2_000), { startMs: 0, endMs: 4_000 });
  assert.deepEqual(clampWindow(30_000, 5_000), { startMs: 5_000, endMs: 15_000 });
  assert.deepEqual(clampWindow(30_000, 25_000), { startMs: 20_000, endMs: 30_000 });
  assert.deepEqual(clampWindow(30_000, -50), { startMs: 0, endMs: MAX_SOUND_MS });
  assert.deepEqual(clampWindow(Number.NaN, 10), { startMs: 0, endMs: 0 });
});

test("a window in frames never passes ten seconds", () => {
  assert.deepEqual(windowFrames(1_000, 3_000), { from: 48_000, frames: 96_000 });
  assert.deepEqual(windowFrames(0, 12_000), { from: 0, frames: 480_000 });
  assert.equal(framesToMs(480_000), 10_000);
  assert.equal(framesToMs(47_999), 999);
});

test("packets are trimmed so the file says exactly how long the sound is", () => {
  const preSkip = 312;
  // 1 s of sound: 50 packets of 20 ms would play 1 s; with the pre-skip the encoder gives 51.
  const durations = Array.from({ length: 52 }, () => 960);
  const kept = trimPackets(durations, 48_000, preSkip);
  assert.equal(kept.length, 51);
  assert.equal(kept.reduce((n, d) => n + d, 0), 48_000);
  assert.ok(kept.every((d) => d >= 0 && d <= 960));
  // A sound shorter than one packet.
  const short = trimPackets([960, 960], 100, preSkip);
  assert.equal(short.length, 1);
  assert.deepEqual(short, [100]);
  assert.deepEqual(trimPackets([], 100, preSkip), []);
});

test("names come from the file's, and clash with nothing", () => {
  assert.equal(soundNameFromFile("air_horn.mp3"), "air horn");
  assert.equal(soundNameFromFile("  big   laugh.final.wav"), "big laugh.final");
  assert.equal(soundNameFromFile(".ogg"), "");
  assert.equal([...soundNameFromFile(`${"a".repeat(40)}.flac`)].length, 32);
  assert.equal(freeName("Horn", ["horn"]), "Horn 2");
  assert.equal(freeName("Horn", ["horn", "HORN 2"]), "Horn 3");
  assert.equal(freeName("Laugh", ["horn"]), "Laugh");
  assert.equal([...freeName("x".repeat(32), ["x".repeat(32)])].length, 32);
  assert.equal(validName(" "), false);
  assert.equal(validName("gg"), true);
  assert.equal(validName("y".repeat(33)), false);
});

test("emoji are kept as the instance takes them", () => {
  assert.equal(soundEmoji("<a:party:01HZY3W8Q9ZJ4K7X2M5N6P8R0T>"), "<:party:01HZY3W8Q9ZJ4K7X2M5N6P8R0T>");
  assert.equal(soundEmoji(" 📯 "), "📯");
  assert.deepEqual(readEmoji("<:party:01HZY3W8Q9ZJ4K7X2M5N6P8R0T>"), { kind: "custom", name: "party", id: "01HZY3W8Q9ZJ4K7X2M5N6P8R0T" });
  assert.deepEqual(readEmoji("🎺"), { kind: "unicode", text: "🎺" });
  assert.equal(readEmoji(""), null);
});

test("sounds list oldest first", () => {
  const at = (seconds: number) => ({ seconds: BigInt(seconds), nanos: 0 });
  const sorted = sortSounds([
    { id: "b", createdAt: at(2) },
    { id: "a", createdAt: at(2) },
    { id: "c", createdAt: at(1) },
  ]);
  assert.deepEqual(
    sorted.map((s) => s.id),
    ["c", "a", "b"],
  );
});
