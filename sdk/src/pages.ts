import type { Fuwa } from "./client.js";
import type { ListMessagesResponse } from "./gen/fuwa/v1/message_pb.js";
import type { Message, User } from "./gen/fuwa/v1/types_pb.js";

export interface MessagePagesOptions {
  serverId: string;
  channelId: string;
  /**
   * "older" (default) walks back from the latest message, or from `from`;
   * "newer" walks forward from `from`.
   */
  direction?: "older" | "newer";
  /** The message to start next to (not included). Needed for "newer". */
  from?: string;
  /** Messages per call, 1 to 100. Default 50. */
  pageSize?: number;
  signal?: AbortSignal;
}

/** A channel's messages, a page (one ListMessages call) at a time, oldest first within each page. */
export async function* messagePages(fuwa: Fuwa, options: MessagePagesOptions): AsyncGenerator<ListMessagesResponse> {
  const newer = options.direction === "newer";
  if (newer && !options.from) throw new TypeError('paging "newer" needs a message to start from');
  let cursor = options.from ?? "";
  for (;;) {
    const page = await fuwa.messages.listMessages(
      {
        serverId: options.serverId,
        channelId: options.channelId,
        limit: options.pageSize ?? 50,
        beforeId: newer ? "" : cursor,
        afterId: newer ? cursor : "",
      },
      { signal: options.signal },
    );
    if (page.messages.length === 0) return;
    yield page;
    if (!page.hasMore) return;
    cursor = newer ? page.messages.at(-1)!.id : page.messages[0]!.id;
  }
}

/** One message and who wrote it, when the instance sent their profile. */
export interface MessageWithAuthor {
  message: Message;
  author: User | undefined;
}

/**
 * A channel's messages one at a time, in the order walked: newest first for
 * "older", oldest first for "newer". Stop whenever; nothing more is fetched.
 *
 *     for await (const { message } of messages(fuwa, { serverId, channelId })) {
 *       if (message.content.includes("hello")) break;
 *     }
 */
export async function* messages(fuwa: Fuwa, options: MessagePagesOptions): AsyncGenerator<MessageWithAuthor> {
  for await (const page of messagePages(fuwa, options)) {
    const authors = new Map(page.authors.map((u) => [u.id, u]));
    const list = options.direction === "newer" ? page.messages : [...page.messages].reverse();
    for (const message of list) yield { message, author: authors.get(message.authorId) };
  }
}
