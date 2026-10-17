import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { test } from "node:test";
import { oggOpusMs, toOggOpus } from "../src/soundboard.ts";

/** One Ogg page holding one packet. */
function page(granule: bigint, packet: Uint8Array): Uint8Array {
  const out = new Uint8Array(28 + packet.length);
  out.set([0x4f, 0x67, 0x67, 0x53]);
  new DataView(out.buffer).setBigInt64(6, granule, true);
  out[26] = 1;
  out[27] = packet.length;
  out.set(packet, 28);
  return out;
}

function opusHead(preSkip: number): Uint8Array {
  const head = new Uint8Array(19);
  head.set(new TextEncoder().encode("OpusHead"));
  head[8] = 1;
  head[9] = 1;
  new DataView(head.buffer).setUint16(10, preSkip, true);
  return head;
}

const ogg = (...pages: Uint8Array[]) => Uint8Array.from(pages.flatMap((p) => [...p]));

test("how long Ogg Opus plays: the last granule less the pre-skip", () => {
  const file = ogg(page(0n, opusHead(312)), page(0n, new Uint8Array(8)), page(48_000n + 312n, new Uint8Array(3)));
  assert.equal(oggOpusMs(file), 1000);
  assert.equal(oggOpusMs(new TextEncoder().encode("ID3 an mp3")), undefined);
  assert.equal(oggOpusMs(ogg(page(0n, new TextEncoder().encode("OggVorbis..........")))), undefined);
});

test("Ogg Opus is kept as it is", async () => {
  const file = ogg(page(0n, opusHead(312)), page(96_312n, new Uint8Array(3)));
  const { bytes, durationMs } = await toOggOpus(file, "no-ffmpeg-needed");
  assert.equal(bytes, file);
  assert.equal(durationMs, 2000);
});

test("without ffmpeg, other files are turned down in words", async () => {
  await assert.rejects(toOggOpus(new TextEncoder().encode("ID3"), "/nonexistent/ffmpeg"), /ffmpeg isn't installed/);
});

let ffmpeg = false;
try {
  execFileSync("ffmpeg", ["-version"], { stdio: "ignore" });
  ffmpeg = true;
} catch {
  // not installed here
}

test("an MP3 becomes Ogg Opus, cut to 10 seconds", { skip: ffmpeg ? false : "ffmpeg isn't installed" }, async () => {
  const mp3 = execFileSync("ffmpeg", [
    ...["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=12"],
    ...["-c:a", "libmp3lame", "-f", "mp3", "pipe:1"],
  ]);
  const { bytes, durationMs } = await toOggOpus(new Uint8Array(mp3));
  assert.equal(oggOpusMs(bytes), durationMs);
  assert.ok(durationMs > 9_000 && durationMs <= 10_000, `${durationMs} ms`);
});
