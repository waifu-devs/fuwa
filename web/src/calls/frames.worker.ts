/// <reference lib="webworker" />

/**
 * End-to-end encryption for direct message calls, one frame at a time.
 *
 * Every frame of sound is sealed with AES-GCM before it leaves the browser
 * and opened only by the other people in the conversation, so the media
 * server forwards what it can't read. The key comes from the
 * conversation's MLS group (an exported secret, new every epoch), and each
 * sender gets their own key from it (HKDF with their user id), so two
 * people never seal with the same key.
 *
 * A sealed frame is the ciphertext with its tag, then a trailer the
 * receiver reads first: the 12-byte nonce, the epoch (4 bytes, big endian)
 * and a version byte. The trailer is authenticated along with the frame.
 *
 * Camera frames (VP8) keep their first few bytes in the clear, as other
 * end-to-end encrypted calls do: 10 for a keyframe, 3 otherwise, which say
 * only whether it's a keyframe and its size, so the media server can start
 * each viewer on a keyframe. They're authenticated too: changing them
 * breaks the frame.
 */

type Direction = "send" | "recv";
type Kind = "audio" | "video";
type Options = { direction: Direction; sender: string; kind?: Kind };
type Frame = { data: ArrayBuffer };

const VERSION = 1;
const TRAILER = 12 + 4 + 1;
/** Epochs kept for frames still in flight after a change. */
const KEEP_EPOCHS = 4;

const secrets = new Map<number, Uint8Array<ArrayBuffer>>();
const keys = new Map<string, Promise<CryptoKey>>();
let current = -1;
/** Epochs already asked for, so a run of frames asks once. */
const asked = new Set<number>();

const encoder = new TextEncoder();

function keyFor(epoch: number, sender: string): Promise<CryptoKey> | null {
  const secret = secrets.get(epoch);
  if (!secret) return null;
  const id = `${epoch}/${sender}`;
  let key = keys.get(id);
  if (!key) {
    key = crypto.subtle
      .importKey("raw", secret, "HKDF", false, ["deriveKey"])
      .then((base) =>
        crypto.subtle.deriveKey(
          { name: "HKDF", hash: "SHA-256", salt: new Uint8Array(), info: encoder.encode(`fuwa call frame ${sender}`) },
          base,
          { name: "AES-GCM", length: 256 },
          false,
          ["encrypt", "decrypt"],
        ),
      );
    keys.set(id, key);
  }
  return key;
}

/** How many bytes at the start of a frame stay in the clear: a VP8 frame's header. */
function clearBytes(bytes: Uint8Array, kind: Kind | undefined): number {
  if (kind !== "video" || !bytes.length) return 0;
  // VP8's first bit is 0 on a keyframe.
  return Math.min(bytes.length, (bytes[0]! & 1) === 0 ? 10 : 3);
}

/** What the tag covers besides the ciphertext: the clear header, then the epoch and version. */
function associated(header: Uint8Array, trailer: Uint8Array): Uint8Array<ArrayBuffer> {
  const out = new Uint8Array(header.length + 5);
  out.set(header, 0);
  out.set(trailer.subarray(12), header.length);
  return out;
}

async function seal(frame: Frame, options: Options): Promise<boolean> {
  if (current < 0) return false;
  const key = keyFor(current, options.sender);
  if (!key) return false;
  const bytes = new Uint8Array(frame.data);
  const header = bytes.subarray(0, clearBytes(bytes, options.kind));
  const trailer = new Uint8Array(TRAILER);
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  trailer.set(nonce, 0);
  new DataView(trailer.buffer).setUint32(12, current);
  trailer[16] = VERSION;
  const additionalData = header.length ? associated(header, trailer) : trailer.subarray(12);
  const sealed = new Uint8Array(await crypto.subtle.encrypt({ name: "AES-GCM", iv: nonce, additionalData }, await key, bytes.subarray(header.length)));
  const out = new Uint8Array(header.length + sealed.length + TRAILER);
  out.set(header, 0);
  out.set(sealed, header.length);
  out.set(trailer, header.length + sealed.length);
  frame.data = out.buffer;
  return true;
}

async function open(frame: Frame, options: Options): Promise<boolean> {
  const sender = options.sender;
  const bytes = new Uint8Array(frame.data);
  if (bytes.length <= TRAILER + 16 || bytes[bytes.length - 1] !== VERSION) return false;
  const header = bytes.subarray(0, clearBytes(bytes, options.kind));
  if (bytes.length <= header.length + TRAILER + 16) return false;
  const trailer = bytes.subarray(bytes.length - TRAILER);
  const epoch = new DataView(trailer.buffer, trailer.byteOffset).getUint32(12);
  const key = keyFor(epoch, sender);
  if (!key) {
    if (epoch > current && !asked.has(epoch)) {
      asked.add(epoch);
      self.postMessage({ type: "need", epoch });
    }
    return false;
  }
  try {
    const additionalData = header.length ? associated(header, trailer) : trailer.slice(12);
    const plain = new Uint8Array(
      await crypto.subtle.decrypt({ name: "AES-GCM", iv: trailer.slice(0, 12), additionalData }, await key, bytes.subarray(header.length, bytes.length - TRAILER)),
    );
    if (!header.length) {
      frame.data = plain.buffer;
      return true;
    }
    const out = new Uint8Array(header.length + plain.length);
    out.set(header, 0);
    out.set(plain, header.length);
    frame.data = out.buffer;
    return true;
  } catch {
    return false;
  }
}

function pipe(readable: ReadableStream<Frame>, writable: WritableStream<Frame>, options: Options) {
  const work = options.direction === "send" ? seal : open;
  void readable
    .pipeThrough(
      new TransformStream<Frame, Frame>({
        async transform(frame, controller) {
          // A frame that can't be sealed or opened is dropped: never sent in the clear, never played as noise.
          if (await work(frame, options)) controller.enqueue(frame);
        },
      }),
    )
    .pipeTo(writable)
    .catch(() => {});
}

self.onmessage = (e: MessageEvent) => {
  const m = e.data as
    | { type: "key"; epoch: number; secret: Uint8Array<ArrayBuffer> }
    | { type: "stream"; readable: ReadableStream<Frame>; writable: WritableStream<Frame>; options: Options };
  if (m.type === "key") {
    secrets.set(m.epoch, m.secret);
    if (m.epoch > current) current = m.epoch;
    for (const epoch of [...secrets.keys()].sort((a, b) => b - a).slice(KEEP_EPOCHS)) {
      secrets.delete(epoch);
      for (const id of [...keys.keys()]) if (id.startsWith(`${epoch}/`)) keys.delete(id);
    }
  } else if (m.type === "stream") {
    pipe(m.readable, m.writable, m.options);
  }
};

// RTCRtpScriptTransform (Safari, Firefox, newer Chrome) hands the streams over here.
(self as unknown as { onrtctransform: ((e: Event & { transformer: { readable: ReadableStream<Frame>; writable: WritableStream<Frame>; options: Options } }) => void) | null }).onrtctransform =
  (e) => pipe(e.transformer.readable, e.transformer.writable, e.transformer.options);
