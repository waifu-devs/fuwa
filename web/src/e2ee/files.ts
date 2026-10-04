import { create } from "@bufbuild/protobuf";
import { SealedFileSchema, type SealedFile } from "@/gen/fuwa/v1/dm_pb";
import { cleanName, MAX_FILE_BYTES, MAX_FILES, plausible } from "@/files/sealed";
import type { FileRef } from "./vault";

/** Files as they go inside an encrypted message or a backup part. */
export function toSealedFiles(files: FileRef[] | undefined): SealedFile[] {
  return (files ?? []).map((f) =>
    create(SealedFileSchema, {
      mediaId: f.mediaId,
      key: f.key,
      sha256: f.sha256,
      size: BigInt(f.size),
      contentType: f.type,
      name: f.name,
      width: f.width,
      height: f.height,
      chunkBytes: f.chunkBytes,
      fileSize: BigInt(f.fileSize),
    }),
  );
}

/**
 * The files someone else's message carries, checked: each one this app can
 * fetch and open, at most ten. Anything off leaves that file out, never the
 * message. What a file is shown as is decided once it's opened, from its
 * own bytes; the type and size here are the sender's word.
 */
export function filesOf(list: SealedFile[] | undefined): FileRef[] {
  const out: FileRef[] = [];
  for (const f of (list ?? []).slice(0, MAX_FILES)) {
    const size = Number(f.size);
    const fileSize = Number(f.fileSize);
    if (!/^[0-9a-z]{26}$/i.test(f.mediaId) || f.key.length !== 32 || f.sha256.length !== 32) continue;
    if (!plausible(size, f.chunkBytes)) continue;
    const mediaId = f.mediaId.toLowerCase();
    if (out.some((x) => x.mediaId === mediaId)) continue;
    out.push({
      // Fetched from this instance by id, never from a link the message names.
      mediaId,
      key: f.key,
      sha256: f.sha256,
      size,
      chunkBytes: f.chunkBytes,
      name: cleanName(f.name),
      type: f.contentType.slice(0, 100),
      width: Math.min(f.width, 16_384),
      height: Math.min(f.height, 16_384),
      fileSize: Number.isSafeInteger(fileSize) && fileSize >= 0 && fileSize <= Math.min(size, MAX_FILE_BYTES) ? fileSize : 0,
    });
  }
  return out;
}
