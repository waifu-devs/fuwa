import { create } from "@bufbuild/protobuf";
import { DirectMessageVoiceSchema, SealedFileSchema, type DirectMessageVoice } from "@/gen/fuwa/v1/dm_pb";
import type { Voice } from "./vault";

/** What a voice message is once opened. */
const VOICE_TYPE = "audio/ogg; codecs=opus";
/** The biggest sealed voice message a device takes from a message. */
const MAX_VOICE_BYTES = 256 * 1024 * 1024;

/** A voice message as it goes inside an encrypted message or a backup part. */
export function toVoiceMessage(voice: Voice, replyTo = 0): DirectMessageVoice {
  return create(DirectMessageVoiceSchema, {
    file: create(SealedFileSchema, {
      mediaId: voice.mediaId,
      key: voice.key,
      sha256: voice.sha256,
      size: BigInt(voice.size),
      contentType: VOICE_TYPE,
    }),
    durationMs: voice.durationMs,
    waveform: voice.waveform,
    replyToSequence: BigInt(replyTo),
  });
}

/** A voice message someone else wrote, checked, or null if it isn't one. */
export function voiceOf(body: DirectMessageVoice | undefined): Voice | null {
  const file = body?.file;
  if (!body || !file || !/^[0-9a-z]{26}$/i.test(file.mediaId) || file.key.length !== 32 || file.sha256.length !== 32) return null;
  const size = Number(file.size);
  if (!(size > 0 && size <= MAX_VOICE_BYTES)) return null;
  return {
    // Fetched from this instance by id, never from a link the message names.
    mediaId: file.mediaId.toLowerCase(),
    key: file.key,
    sha256: file.sha256,
    size,
    durationMs: Math.min(body.durationMs, 24 * 60 * 60 * 1000),
    waveform: body.waveform.slice(0, 128),
  };
}

/** "1:05": how long a voice message plays, for previews and notifications. */
export function voiceLength(ms: number): string {
  const s = Math.max(0, Math.round(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
