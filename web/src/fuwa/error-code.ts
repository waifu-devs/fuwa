import { Code, ConnectError } from "@connectrpc/connect";

/**
 * What kind of failure something thrown while talking to a server is. A
 * browser's own network failure (the request never got an answer) counts as
 * the server being unreachable, never as anything the server said.
 */
export function errorCode(cause: unknown): { code: Code; message: string; network: boolean } {
  const err = ConnectError.from(cause);
  const network = err.code === Code.Unknown && /fetch|network|load failed/i.test(err.rawMessage);
  return { code: network ? Code.Unavailable : err.code, message: err.rawMessage, network };
}
