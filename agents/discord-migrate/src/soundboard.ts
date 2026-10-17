// fuwa's soundboard (docs/soundboard.md): adding a server's sounds through
// SoundboardService.CreateSound, and turning Discord's sound files into the
// Ogg Opus fuwa keeps. The SDK on npm doesn't have the service yet, so its
// one call is described here, over the agent's own connection (its token,
// retries and typed errors). Once the SDK ships it, `agent.api.soundboard`
// does the same.
import { execFile } from "node:child_process";
import { create, createFileRegistry } from "@bufbuild/protobuf";
import {
  FieldDescriptorProto_Label,
  FieldDescriptorProto_Type,
  FileDescriptorProtoSchema,
} from "@bufbuild/protobuf/wkt";
import { Code, ConnectError, createClient, type Transport } from "@connectrpc/connect";

const { STRING, FLOAT, INT32, MESSAGE } = FieldDescriptorProto_Type;

const field = (name: string, number: number, type: FieldDescriptorProto_Type, extra: object = {}) => ({
  name,
  number,
  type,
  label: FieldDescriptorProto_Label.OPTIONAL,
  jsonName: name.replace(/_(\w)/g, (_, c: string) => c.toUpperCase()),
  ...extra,
});

// The part of proto/fuwa/v1/soundboard.proto (and Sound, in types.proto) this uses.
const file = create(FileDescriptorProtoSchema, {
  name: "discord-migrate/soundboard.proto",
  package: "fuwa.v1",
  syntax: "proto3",
  messageType: [
    {
      name: "Sound",
      field: [
        field("id", 1, STRING),
        field("name", 3, STRING),
        field("emoji", 4, STRING),
        field("volume", 6, FLOAT),
        field("duration_ms", 7, INT32),
      ],
    },
    {
      name: "CreateSoundRequest",
      field: [
        field("server_id", 1, STRING),
        field("name", 2, STRING),
        field("url", 3, STRING),
        field("emoji", 4, STRING),
        field("volume", 5, FLOAT, { proto3Optional: true, oneofIndex: 0 }),
        field("duration_ms", 6, INT32),
      ],
      oneofDecl: [{ name: "_volume" }],
    },
    { name: "CreateSoundResponse", field: [field("sound", 1, MESSAGE, { typeName: ".fuwa.v1.Sound" })] },
    { name: "ListSoundsRequest", field: [field("server_id", 1, STRING)] },
    {
      name: "ListSoundsResponse",
      field: [field("sounds", 1, MESSAGE, { typeName: ".fuwa.v1.Sound", label: FieldDescriptorProto_Label.REPEATED })],
    },
  ],
  service: [
    {
      name: "SoundboardService",
      method: [
        { name: "ListSounds", inputType: ".fuwa.v1.ListSoundsRequest", outputType: ".fuwa.v1.ListSoundsResponse" },
        { name: "CreateSound", inputType: ".fuwa.v1.CreateSoundRequest", outputType: ".fuwa.v1.CreateSoundResponse" },
      ],
    },
  ],
});

const SoundboardService = createFileRegistry(file, () => undefined).getService("fuwa.v1.SoundboardService")!;

export interface CreateSoundRequest {
  serverId: string;
  name: string;
  url: string;
  emoji: string;
  volume?: number;
  durationMs: number;
}

export interface Sound {
  id: string;
  name: string;
  emoji: string;
  volume: number;
  durationMs: number;
}

export interface SoundboardClient {
  listSounds(req: { serverId: string }): Promise<{ sounds: Sound[] }>;
  createSound(req: CreateSoundRequest): Promise<{ sound?: Sound }>;
}

export function soundboard(transport: Transport): SoundboardClient {
  return createClient(SoundboardService, transport) as unknown as SoundboardClient;
}

/** Whether the instance has a soundboard: older ones don't know the service (Unimplemented, or HTTP 404). */
export async function hasSoundboard(client: SoundboardClient, serverId: string): Promise<boolean> {
  try {
    await client.listSounds({ serverId });
    return true;
  } catch (err) {
    if (err instanceof ConnectError && (err.code === Code.Unimplemented || /\bHTTP 404\b/.test(err.rawMessage))) {
      return false;
    }
    throw err;
  }
}

/** What fuwa takes: Ogg Opus, at most 10 seconds and 1 MiB. */
export const MAX_SOUND_MS = 10_000;
export const MAX_SOUND_BYTES = 1 << 20;

/** Ogg Opus, and how long it plays. */
export interface OggOpus {
  bytes: Uint8Array;
  durationMs: number;
}

/**
 * How long an Ogg Opus file plays, as fuwa reads it: the last page's
 * granule position less the pre-skip, at 48 kHz. Undefined when it isn't
 * one stream of Opus.
 */
export function oggOpusMs(bytes: Uint8Array): number | undefined {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const isPage = (at: number) =>
    at + 27 <= bytes.length &&
    bytes[at] === 0x4f &&
    bytes[at + 1] === 0x67 &&
    bytes[at + 2] === 0x67 &&
    bytes[at + 3] === 0x53;
  if (!isPage(0)) return undefined;
  // The first page holds OpusHead: "OpusHead", version, channels, pre-skip (LE16).
  const head = 27 + bytes[26]!;
  if (head + 12 > bytes.length || new TextDecoder().decode(bytes.subarray(head, head + 8)) !== "OpusHead")
    return undefined;
  const preSkip = view.getUint16(head + 10, true);
  let granule = 0n;
  for (let at = 0; isPage(at); ) {
    const g = view.getBigInt64(at + 6, true);
    if (g >= 0n) granule = g;
    const segments = bytes[at + 26]!;
    let size = 27 + segments;
    for (let i = 0; i < segments; i++) size += bytes[at + 27 + i]!;
    at += size;
  }
  const samples = Number(granule) - preSkip;
  return samples > 0 ? Math.round((samples / 48_000) * 1000) : undefined;
}

/**
 * A sound file in any format (Discord keeps MP3 and Ogg) as Ogg Opus: as it
 * is when it already is, otherwise through ffmpeg, cut to 10 seconds.
 * Throws a sentence when it can't.
 */
export async function toOggOpus(bytes: Uint8Array, ffmpeg = "ffmpeg"): Promise<OggOpus> {
  const already = oggOpusMs(bytes);
  if (already !== undefined && already <= MAX_SOUND_MS && bytes.length <= MAX_SOUND_BYTES) {
    return { bytes, durationMs: already };
  }
  const out = await new Promise<Uint8Array>((resolve, reject) => {
    const child = execFile(
      ffmpeg,
      [
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        "pipe:0",
        "-t",
        "9.9",
        "-vn",
        "-ar",
        "48000",
        "-c:a",
        "libopus",
        "-b:a",
        "96k",
        "-f",
        "ogg",
        "pipe:1",
      ],
      { encoding: "buffer", maxBuffer: 8 << 20, timeout: 60_000 },
      (err, stdout, stderr) => {
        if (err) {
          const missing = (err as NodeJS.ErrnoException).code === "ENOENT";
          reject(
            new Error(
              missing
                ? "ffmpeg isn't installed to convert it"
                : `ffmpeg couldn't read it: ${String(stderr).trim().split("\n")[0] ?? ""}`,
            ),
          );
        } else resolve(new Uint8Array(stdout));
      },
    );
    child.stdin?.on("error", () => {});
    child.stdin?.end(bytes);
  });
  const durationMs = oggOpusMs(out);
  if (durationMs === undefined) throw new Error("it has no sound");
  if (out.length > MAX_SOUND_BYTES) throw new Error("it's too big once converted");
  return { bytes: out, durationMs: Math.min(durationMs, MAX_SOUND_MS) };
}
