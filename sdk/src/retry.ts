import { CanceledError, Code, type FuwaError } from "./errors.js";

/** How calls are tried again. Pass `retry: false` to a client to turn it off. */
export interface RetryOptions {
  /** Tries after the first one. Default 4. */
  retries?: number;
  /** The first wait, doubled each time with jitter. Default 400 ms. */
  baseDelayMs?: number;
  /** No wait is longer than this. Default 20 s. */
  maxDelayMs?: number;
  /**
   * The longest wait the instance may ask for (slow mode, rate limits) that
   * is still waited out; longer ones fail at once. Default 60 s.
   */
  maxRetryAfterMs?: number;
}

export const DEFAULT_RETRY: Required<RetryOptions> = {
  retries: 4,
  baseDelayMs: 400,
  maxDelayMs: 20_000,
  maxRetryAfterMs: 60_000,
};

/** The wait before try number `attempt` (0 is the first retry): exponential, with full jitter. */
export function backoff(attempt: number, opts: Required<RetryOptions>, random = Math.random): number {
  const ceiling = Math.min(opts.maxDelayMs, opts.baseDelayMs * 2 ** attempt);
  return Math.round(ceiling / 2 + (random() * ceiling) / 2);
}

/** Whether a method only reads, so it's safe to send again after a network failure. */
export function readsOnly(method: string): boolean {
  return /^(Get|List|Discover|Preview|Check)/.test(method);
}

/**
 * Whether to try a call again after `err`, and after how long; undefined to
 * give up. Rate limits and slow mode wait as long as the instance asked.
 * Writes are tried again only when the instance answered that it didn't take
 * them (Unavailable from the instance itself), never after a network failure,
 * since they may have gone through.
 */
export function retryDelay(
  err: FuwaError,
  method: string,
  attempt: number,
  opts: Required<RetryOptions>,
): number | undefined {
  if (attempt >= opts.retries) return undefined;
  if (err.code === Code.ResourceExhausted) {
    if (err.retryAfterMs === undefined || err.retryAfterMs > opts.maxRetryAfterMs) return undefined;
    return err.retryAfterMs + 50;
  }
  if (!err.retryable) return undefined;
  if (err.network && !readsOnly(method)) return undefined;
  return backoff(attempt, opts);
}

/** Waits `ms`, or fails with CanceledError as soon as `signal` aborts. */
export function sleep(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(new CanceledError(Code.Canceled, "the call was cancelled"));
    const done = () => {
      signal?.removeEventListener("abort", stop);
      resolve();
    };
    const timer = setTimeout(done, ms);
    const stop = () => {
      clearTimeout(timer);
      reject(new CanceledError(Code.Canceled, "the call was cancelled"));
    };
    signal?.addEventListener("abort", stop, { once: true });
  });
}
