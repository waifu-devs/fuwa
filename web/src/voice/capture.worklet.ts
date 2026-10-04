/**
 * Hands the microphone's samples (the first channel) to the recorder, a
 * block at a time. Runs on the audio thread.
 */

declare class AudioWorkletProcessor {
  readonly port: MessagePort;
}
declare function registerProcessor(name: string, ctor: unknown): void;

class Capture extends AudioWorkletProcessor {
  private buffer = new Float32Array(2048);
  private filled = 0;

  process(inputs: Float32Array[][]) {
    const channel = inputs[0]?.[0];
    if (channel) {
      for (let i = 0; i < channel.length; i++) {
        this.buffer[this.filled++] = channel[i]!;
        if (this.filled === this.buffer.length) {
          this.port.postMessage(this.buffer, [this.buffer.buffer]);
          this.buffer = new Float32Array(2048);
          this.filled = 0;
        }
      }
    }
    return true;
  }
}

registerProcessor("fuwa-voice-capture", Capture);
