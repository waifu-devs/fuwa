import { ConnectError } from "@connectrpc/connect";
import type { Fuwa } from "./client.js";
import { Code, FailedPreconditionError, InvalidArgumentError, toFuwaError } from "./errors.js";
import type { Media, MediaPurpose } from "./gen/fuwa/v1/media_pb.js";

export interface UploadOptions {
  /** What it's for: MediaPurpose.AVATAR, EMOJI, SERVER_ICON and so on. */
  purpose: MediaPurpose;
  /** The picture's bytes. */
  data: Blob | ArrayBuffer | Uint8Array;
  /** image/png, image/jpeg, image/gif, image/webp or image/avif. Read from a Blob's type when left out. */
  contentType?: string;
  /** For a server's icon or emoji: the server it's for. */
  serverId?: string;
  signal?: AbortSignal;
}

/**
 * Uploads a picture to the instance the way the apps do
 * (MediaService.CreateUpload, then a PUT of the bytes) and returns where
 * it's served. Use `media.url` as an avatar (UpdateProfile, or UpdateAgent by
 * its owner), an emoji's picture (CreateEmoji) or a server icon. The bytes go
 * only to the instance: the upload address must be on the instance's own
 * public address.
 */
export async function uploadPicture(fuwa: Fuwa, options: UploadOptions): Promise<Media> {
  const body =
    options.data instanceof Blob
      ? options.data
      : new Blob([options.data instanceof Uint8Array ? options.data.slice() : new Uint8Array(options.data)]);
  const contentType = options.contentType ?? (options.data instanceof Blob ? options.data.type : "");
  if (!contentType) throw new InvalidArgumentError(Code.InvalidArgument, "say what kind of picture it is (contentType)");
  const reserved = await fuwa.media.createUpload(
    { purpose: options.purpose, contentType, size: BigInt(body.size), serverId: options.serverId ?? "" },
    { signal: options.signal },
  );
  if (!reserved.media) throw new FailedPreconditionError(Code.FailedPrecondition, "the instance didn't reserve the upload");
  await checkOrigin(fuwa, reserved.uploadUrl);
  let res: Response;
  try {
    res = await fuwa.fetch(reserved.uploadUrl, {
      method: "PUT",
      body,
      headers: { "content-type": contentType },
      signal: options.signal,
    });
  } catch (cause) {
    throw toFuwaError(cause, "PUT /media/upload");
  }
  if (!res.ok) {
    const text = (await res.text().catch(() => "")).slice(0, 300).trim();
    const code =
      res.status === 413 || res.status === 429 || res.status === 507
        ? Code.ResourceExhausted
        : res.status >= 500
          ? Code.Unavailable
          : Code.InvalidArgument;
    throw toFuwaError(
      new ConnectError(text || `the upload was refused (${res.status})`, code),
      "PUT /media/upload",
    );
  }
  return reserved.media;
}

/** The upload must go to the instance itself: the address it was reached on, or the one it says is its own. */
async function checkOrigin(fuwa: Fuwa, uploadUrl: string): Promise<void> {
  let target: URL;
  try {
    target = new URL(uploadUrl);
  } catch {
    throw new FailedPreconditionError(Code.FailedPrecondition, "the instance gave an upload address that isn't a URL");
  }
  if (target.origin === new URL(fuwa.url).origin) return;
  const { node } = await fuwa.node.getNode({});
  if (node?.publicUrl && target.origin === new URL(node.publicUrl).origin && target.protocol === "https:") return;
  throw new FailedPreconditionError(
    Code.FailedPrecondition,
    "the upload address isn't on this instance, so the picture wasn't sent",
  );
}
