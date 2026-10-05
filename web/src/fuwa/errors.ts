import { Code } from "@connectrpc/connect";
import { Data, Effect } from "effect";
import { i18n } from "@/i18n/i18n";
import { errorCode } from "./error-code";

/** Anything that went wrong talking to a fuwa server, in words a person can read. */
export class FuwaError extends Data.TaggedError("FuwaError")<{ code: Code; message: string }> {
  /** The session token is gone or expired, so the person has to sign in again. */
  get signedOut() {
    return this.code === Code.Unauthenticated;
  }
  /** Worth trying again later: the server is unreachable, restarting or busy. */
  get retryable() {
    return (
      this.code === Code.Unavailable ||
      this.code === Code.Aborted ||
      this.code === Code.DeadlineExceeded ||
      this.code === Code.Unknown ||
      this.code === Code.Internal ||
      this.code === Code.Canceled
    );
  }
}

export function toFuwaError(cause: unknown): FuwaError {
  if (cause instanceof FuwaError) return cause;
  const { code, message, network } = errorCode(cause);
  if (network) return new FuwaError({ code, message: i18n().t("system.connection.unreachable") });
  return new FuwaError({ code, message: message || i18n().t("system.error.unknown") });
}

/** Runs one RPC as an Effect. The call is cancelled if the Effect is interrupted. */
export const call = <A>(rpc: (signal: AbortSignal) => Promise<A>): Effect.Effect<A, FuwaError> =>
  Effect.tryPromise({ try: rpc, catch: toFuwaError });
