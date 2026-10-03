/**
 * Puts the frame encryption (frames.worker.ts) on a call's senders and
 * receivers, with whichever API this browser has: RTCRtpScriptTransform, or
 * Chrome's encoded streams.
 */

type Options = { direction: "send" | "recv"; sender: string };

declare const RTCRtpScriptTransform: { new (worker: Worker, options: Options): unknown } | undefined;

type WithStreams = { createEncodedStreams?: () => { readable: ReadableStream; writable: WritableStream } };

const scriptTransform = () => typeof RTCRtpScriptTransform !== "undefined";
const encodedStreams = () => typeof RTCRtpSender !== "undefined" && "createEncodedStreams" in RTCRtpSender.prototype;

/** Whether this browser can encrypt a call's frames. */
export const canEncryptCalls = () => scriptTransform() || encodedStreams();

/** What a call's RTCPeerConnection needs to allow encrypting its frames. */
export const encryptedConfig = (): RTCConfiguration =>
  scriptTransform() ? {} : ({ encodedInsertableStreams: true } as RTCConfiguration);

export class FrameCrypto {
  private readonly worker = new Worker(new URL("./frames.worker.ts", import.meta.url), { type: "module" });

  /** `need` hears about an epoch a frame arrived in that this browser doesn't have the secret for yet. */
  constructor(need: (epoch: number) => void) {
    this.worker.onmessage = (e: MessageEvent<{ type: "need"; epoch: number }>) => {
      if (e.data?.type === "need") need(e.data.epoch);
    };
  }

  setKey(epoch: number, secret: Uint8Array) {
    const copy = new Uint8Array(secret);
    this.worker.postMessage({ type: "key", epoch, secret: copy }, [copy.buffer]);
  }

  /** Senders and receivers already set up: Chrome throws on a second createEncodedStreams. */
  private readonly attached = new WeakSet<object>();

  private attach(target: (RTCRtpSender | RTCRtpReceiver) & WithStreams & { transform?: unknown }, options: Options) {
    if (this.attached.has(target)) return;
    this.attached.add(target);
    if (scriptTransform()) {
      target.transform = new RTCRtpScriptTransform!(this.worker, options) as RTCRtpSender["transform"];
      return;
    }
    const { readable, writable } = target.createEncodedStreams!();
    this.worker.postMessage({ type: "stream", readable, writable, options }, [readable as never, writable as never]);
  }

  /** Seals what you send, as `me`. */
  send(sender: RTCRtpSender, me: string) {
    this.attach(sender, { direction: "send", sender: me });
  }

  /** Opens what someone sends. */
  receive(receiver: RTCRtpReceiver, from: string) {
    this.attach(receiver, { direction: "recv", sender: from });
  }

  close() {
    this.worker.terminate();
  }
}
