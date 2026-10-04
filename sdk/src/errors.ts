import { Code, ConnectError } from "@connectrpc/connect";

export { Code };

/**
 * Anything that went wrong talking to a fuwa instance. Every call made
 * through the SDK fails with one of the subclasses below, so apps can branch
 * with `instanceof` (or on `code`) instead of reading messages. It's a
 * ConnectError too, with the response's `metadata`.
 *
 * Messages come from the instance and are meant for people. They never hold
 * the token; a failure to reach the instance at all says so in fixed words,
 * with the underlying error kept in `cause` for the app to inspect.
 */
export class FuwaError extends ConnectError {
  override name = "FuwaError";
  /** The method that failed, such as "fuwa.v1.MessageService/SendMessage", when known. */
  readonly method: string | undefined;
  /**
   * How long the instance asked to wait before trying again, in
   * milliseconds, when it said (slow mode, or a `retry-after` header).
   */
  readonly retryAfterMs: number | undefined;
  /** The request never got an answer: the network failed or the instance was unreachable. */
  readonly network: boolean;

  constructor(
    code: Code,
    message: string,
    options: { cause?: unknown; method?: string; retryAfterMs?: number; network?: boolean; metadata?: Headers } = {},
  ) {
    super(message, code, options.metadata, undefined, options.cause);
    // The instance's words as they are, without ConnectError's "[code]" in front.
    this.message = message;
    this.method = options.method;
    this.retryAfterMs = options.retryAfterMs;
    this.network = options.network ?? false;
  }

  // ConnectError's own check only knows ConnectError itself by name; these
  // classes are told apart the ordinary way, by their prototypes.
  static override [Symbol.hasInstance](value: unknown): boolean {
    return Function.prototype[Symbol.hasInstance].call(this, value);
  }

  /** The token is missing, wrong, reset or revoked: the app has to get a new one. */
  get signedOut(): boolean {
    return this.code === Code.Unauthenticated;
  }

  /**
   * Worth trying the same call again later: the instance was unreachable,
   * restarting or busy, or asked to wait. Retrying a call that writes after a
   * network failure may do it twice, since it may have gone through.
   */
  get retryable(): boolean {
    switch (this.code) {
      case Code.Unavailable:
      case Code.DeadlineExceeded:
      case Code.Aborted:
        return true;
      case Code.ResourceExhausted:
        return this.retryAfterMs !== undefined;
      default:
        return false;
    }
  }
}

/** The token is missing, wrong or no longer works (Unauthenticated). */
export class UnauthenticatedError extends FuwaError {
  override name = "UnauthenticatedError";
}
/** Signed in, but not allowed to do this here (PermissionDenied). */
export class PermissionDeniedError extends FuwaError {
  override name = "PermissionDeniedError";
}
/** The server, channel, message or account isn't there, or can't be seen (NotFound). */
export class NotFoundError extends FuwaError {
  override name = "NotFoundError";
}
/** The request itself is wrong: a bad field, too long, empty (InvalidArgument). */
export class InvalidArgumentError extends FuwaError {
  override name = "InvalidArgumentError";
}
/** Something with that name or id already exists (AlreadyExists). */
export class AlreadyExistsError extends FuwaError {
  override name = "AlreadyExistsError";
}
/** Not possible in the current state, such as a feature the instance turned off (FailedPrecondition). */
export class FailedPreconditionError extends FuwaError {
  override name = "FailedPreconditionError";
}
/**
 * A limit was hit (ResourceExhausted): slow mode, a rate limit, or a cap
 * such as a full server or storage. `retryAfterMs` is set when waiting helps.
 */
export class RateLimitedError extends FuwaError {
  override name = "RateLimitedError";
}
/** The instance is unreachable, restarting or moving the server (Unavailable). */
export class UnavailableError extends FuwaError {
  override name = "UnavailableError";
}
/** The call took longer than its deadline (DeadlineExceeded). */
export class TimeoutError extends FuwaError {
  override name = "TimeoutError";
}
/** The call was cancelled by the app, such as with an AbortSignal (Canceled). */
export class CanceledError extends FuwaError {
  override name = "CanceledError";
}
/** The instance failed on its side (Internal, Unknown, Unimplemented and the rest). */
export class ServerError extends FuwaError {
  override name = "ServerError";
}

const classes: Partial<Record<Code, typeof FuwaError>> = {
  [Code.Unauthenticated]: UnauthenticatedError,
  [Code.PermissionDenied]: PermissionDeniedError,
  [Code.NotFound]: NotFoundError,
  [Code.InvalidArgument]: InvalidArgumentError,
  [Code.OutOfRange]: InvalidArgumentError,
  [Code.AlreadyExists]: AlreadyExistsError,
  [Code.FailedPrecondition]: FailedPreconditionError,
  [Code.ResourceExhausted]: RateLimitedError,
  [Code.Unavailable]: UnavailableError,
  [Code.Aborted]: UnavailableError,
  [Code.DeadlineExceeded]: TimeoutError,
  [Code.Canceled]: CanceledError,
};

const UNITS: Record<string, number> = { second: 1_000, minute: 60_000, hour: 3_600_000, day: 86_400_000 };

/**
 * How long to wait, from the instance's answer: its `fuwa-retry-after-ms`
 * header, a `retry-after` header in seconds, or (from older instances) slow
 * mode's "you can send again in 12 seconds".
 */
export function retryAfterOf(message: string, metadata?: Headers): number | undefined {
  const exact = metadata?.get("fuwa-retry-after-ms");
  if (exact && /^\d+$/.test(exact.trim())) return Number(exact);
  const header = metadata?.get("retry-after");
  if (header && /^\d+(\.\d+)?$/.test(header.trim())) return Math.ceil(Number(header) * 1000);
  const m = /again in (\d+) (second|minute|hour|day)s?\b/.exec(message);
  if (m) return Number(m[1]) * UNITS[m[2]!]!;
  return undefined;
}

function looksLikeNetwork(err: ConnectError): boolean {
  if (err.cause instanceof TypeError) return true;
  return /fetch failed|failed to fetch|networkerror|network error|load failed|econnrefused|econnreset|socket/i.test(
    err.rawMessage,
  );
}

/** Turns anything thrown by a call into the matching FuwaError subclass. */
export function toFuwaError(cause: unknown, method?: string): FuwaError {
  // connect wraps what an interceptor throws; the typed error is inside.
  for (let c = cause, depth = 0; c instanceof Error && depth < 4; c = c.cause, depth++) {
    if (c instanceof FuwaError) return c;
  }
  const err = ConnectError.from(cause);
  if (err.code === Code.Canceled || (cause instanceof Error && cause.name === "AbortError")) {
    return new CanceledError(Code.Canceled, "the call was cancelled", { cause, method });
  }
  if (err.code !== Code.Unauthenticated && looksLikeNetwork(err)) {
    // Fixed words: the underlying error may name the address it tried.
    return new UnavailableError(Code.Unavailable, "can't reach the instance right now", {
      cause,
      method,
      network: true,
    });
  }
  const message = err.rawMessage || "something went wrong";
  const Class = classes[err.code] ?? ServerError;
  const retryAfterMs = err.code === Code.ResourceExhausted ? retryAfterOf(message, err.metadata) : undefined;
  return new Class(err.code, message, { cause, method, retryAfterMs, metadata: err.metadata });
}
