import { create, fromJson, toJson, type JsonValue, type MessageInitShape } from "@bufbuild/protobuf";
import { Code, FuwaError } from "./errors.js";
import {
  AgentDeliveryAnswerSchema,
  AgentDeliverySchema,
  InteractionReplySchema,
  type AgentDelivery,
  type InteractionReply,
} from "./gen/fuwa/v1/agent_pb.js";
import type { Event, Interaction, InteractionKind } from "./gen/fuwa/v1/types_pb.js";

/** Why a delivery was refused (`InvalidDeliveryError.reason`). */
export type DeliveryProblem = "headers" | "signature" | "timestamp" | "body";

/**
 * A request to an agent's endpoint that isn't a delivery from its instance:
 * headers missing, a signature that doesn't match the secret, a timestamp
 * too far off, or a body that isn't an AgentDelivery. The message is in
 * fixed words; it never holds the secret or the body.
 */
export class InvalidDeliveryError extends FuwaError {
  override name = "InvalidDeliveryError";
  readonly reason: DeliveryProblem;

  constructor(reason: DeliveryProblem, message: string, options: { cause?: unknown } = {}) {
    super(reason === "body" ? Code.InvalidArgument : Code.Unauthenticated, message, options);
    this.reason = reason;
  }
}

/** A delivery's headers, as a `Headers` or a plain object (Node's `req.headers`). */
export type DeliveryHeaders = Headers | Record<string, string | string[] | undefined>;

export interface VerifyOptions {
  /** How far the `webhook-timestamp` may be from now, in seconds. Default 300; 0 doesn't check. */
  toleranceSeconds?: number;
  /** Now, in unix milliseconds, for tests. */
  now?: number;
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const keys = new Map<string, Promise<CryptoKey>>();

function header(headers: DeliveryHeaders, name: string): string | undefined {
  if (typeof (headers as Headers).get === "function") return (headers as Headers).get(name) ?? undefined;
  const value = (headers as Record<string, string | string[] | undefined>)[name];
  return Array.isArray(value) ? value[0] : value;
}

function base64(text: string): Uint8Array<ArrayBuffer> | undefined {
  try {
    return Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
  } catch {
    return undefined;
  }
}

/** The HMAC key of a secret ("whsec_" and base64, as the instance gives it). */
function keyOf(secret: string): Promise<CryptoKey> {
  let key = keys.get(secret);
  if (!key) {
    const raw = base64(secret.startsWith("whsec_") ? secret.slice(6) : secret);
    if (!raw || raw.length === 0) throw new TypeError("an endpoint's secret is \"whsec_\" and base64, as the instance gives it");
    key = globalThis.crypto.subtle.importKey("raw", raw, { name: "HMAC", hash: "SHA-256" }, false, ["verify"]);
    if (keys.size >= 16) keys.delete(keys.keys().next().value!);
    keys.set(secret, key);
  }
  return key;
}

/**
 * Checks that a request to an agent's endpoint came from its instance, and
 * reads the delivery in it (docs/agent-endpoints.md). `body` must be exactly
 * as it came: the signature covers its bytes. `secret` is the endpoint's
 * (`AgentService.GetAgentEndpoint`), or several while one replaces another.
 *
 * The signature is checked the Standard Webhooks way, with WebCrypto (Node
 * 20+, Deno, Bun, Workers), and the timestamp must be within five minutes.
 * Throws InvalidDeliveryError otherwise.
 */
export async function verifyDelivery(
  secret: string | readonly string[],
  body: string | Uint8Array | ArrayBuffer,
  headers: DeliveryHeaders,
  options: VerifyOptions = {},
): Promise<AgentDelivery> {
  const id = header(headers, "webhook-id");
  const timestamp = header(headers, "webhook-timestamp");
  const signatures = header(headers, "webhook-signature");
  if (!id || !timestamp || !signatures) {
    throw new InvalidDeliveryError("headers", "the request has no webhook-id, webhook-timestamp or webhook-signature");
  }
  const seconds = /^\d{1,12}$/.test(timestamp) ? Number(timestamp) : NaN;
  if (!Number.isFinite(seconds)) throw new InvalidDeliveryError("headers", "the webhook-timestamp isn't a time");
  const tolerance = options.toleranceSeconds ?? 300;
  if (tolerance > 0 && Math.abs((options.now ?? Date.now()) / 1000 - seconds) > tolerance) {
    throw new InvalidDeliveryError("timestamp", "the delivery's timestamp is too far from now");
  }

  const bytes = typeof body === "string" ? encoder.encode(body) : new Uint8Array(body);
  const prefix = encoder.encode(`${id}.${timestamp}.`);
  const signed = new Uint8Array(prefix.length + bytes.length);
  signed.set(prefix);
  signed.set(bytes, prefix.length);
  const given = signatures
    .split(" ")
    .filter((s) => s.startsWith("v1,"))
    .map((s) => base64(s.slice(3)))
    .filter((s): s is Uint8Array<ArrayBuffer> => s !== undefined && s.length === 32);
  let ok = false;
  // crypto.subtle.verify compares in constant time.
  for (const s of typeof secret === "string" ? [secret] : secret) {
    const key = await keyOf(s);
    for (const sig of given) if (await globalThis.crypto.subtle.verify("HMAC", key, sig, signed)) ok = true;
  }
  if (!ok) throw new InvalidDeliveryError("signature", "the delivery's signature doesn't match the secret");

  try {
    return fromJson(AgentDeliverySchema, JSON.parse(decoder.decode(bytes)) as JsonValue, { ignoreUnknownFields: true });
  } catch (cause) {
    throw new InvalidDeliveryError("body", "the delivery isn't an AgentDelivery", { cause });
  }
}

/** What to answer an interaction with: text, or text with embeds and buttons. */
export type EndpointReply = string | Omit<MessageInitShape<typeof InteractionReplySchema>, "$typeName" | "interactionId">;

/**
 * Someone ran one of the agent's slash commands or pressed one of its
 * buttons, as a delivery brought it. Like the Agent's InteractionContext,
 * but `reply` goes back in the answer to the delivery: no token needed.
 */
export interface EndpointInteractionContext {
  interaction: Interaction;
  event: Event;
  serverId: string;
  channelId: string;
  userId: string;
  kind: InteractionKind;
  /** A command: its name. */
  command: string;
  /** A command: what was filled in, by option name, as text. */
  options: Record<string, string>;
  /** A button: its custom_id, and the message it's on. */
  customId: string;
  messageId: string;
  /**
   * Answers it with a message in its channel (up to five times). Sent back
   * with the delivery's answer, once every event in it is handled; returning
   * a reply from the handler does the same.
   */
  reply(content: EndpointReply): void;
}

export interface EndpointOptions {
  /**
   * The endpoint's signing secret, from AgentService.GetAgentEndpoint (or
   * two while one replaces another). A secret: keep it out of code and logs.
   */
  secret: string | readonly string[];
  /** Every event, in order, each awaited before the next. */
  onEvent?: (event: Event, delivery: AgentDelivery) => unknown;
  /** Each of the agent's interactions in a delivery, after `onEvent`. Return a reply to answer it. */
  onInteraction?: (ctx: EndpointInteractionContext) => EndpointReply | void | Promise<EndpointReply | void>;
  /**
   * Where each server's events have been handled up to: events at or before
   * it (a delivery tried again) are skipped. Pass a Map you keep, or one
   * read back from storage, to skip them across restarts; default a new one.
   */
  cursors?: Map<string, bigint>;
  /** See verifyDelivery. Default 300. */
  toleranceSeconds?: number;
  /**
   * Where a handler's error goes; the delivery still counts, as with an
   * Agent's handlers. Default console.error with its name and message.
   */
  onError?: (error: unknown) => void;
}

function replyOf(interactionId: string, content: EndpointReply): InteractionReply {
  return create(InteractionReplySchema, typeof content === "string" ? { interactionId, content } : { ...content, interactionId });
}

function json(body: JsonValue, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
}

/**
 * A fetch-style handler for an agent's endpoint: `(request) => response`, as
 * Cloudflare Workers, Deno, Bun and most frameworks take it. It checks each
 * delivery's signature (401 if it's wrong), answers the instance's check,
 * hands your handlers the events in order and sends back the replies to
 * interactions.
 *
 *     export default {
 *       fetch: createEndpoint({
 *         secret: env.FUWA_ENDPOINT_SECRET,
 *         onInteraction: (ctx) => `rolled ${1 + Math.floor(Math.random() * 6)}`,
 *       }),
 *     };
 */
export function createEndpoint(options: EndpointOptions): (request: Request) => Promise<Response> {
  for (const s of typeof options.secret === "string" ? [options.secret] : options.secret) keyOf(s).catch(() => {});
  const cursors = options.cursors ?? new Map<string, bigint>();
  const report = (err: unknown) => {
    if (options.onError) return options.onError(err);
    // Name and message only: a cause may hold addresses.
    const e = err instanceof Error ? err : new Error(String(err));
    console.error(`[fuwa endpoint] ${e.name}: ${e.message}`);
  };

  return async (request) => {
    if (request.method !== "POST") return new Response("only POST", { status: 405, headers: { allow: "POST" } });
    let delivery: AgentDelivery;
    try {
      delivery = await verifyDelivery(options.secret, await request.arrayBuffer(), request.headers, {
        toleranceSeconds: options.toleranceSeconds,
      });
    } catch (err) {
      if (!(err instanceof InvalidDeliveryError)) throw err;
      return new Response(err.message, { status: err.reason === "body" ? 400 : 401 });
    }
    if (delivery.challenge) return json(toJson(AgentDeliveryAnswerSchema, create(AgentDeliveryAnswerSchema, { challenge: delivery.challenge })));

    const replies: InteractionReply[] = [];
    // A delivery tried again repeats events: skip those already handled, with
    // the channel changes (sequence 0) that follow them.
    let skipping = false;
    for (const event of delivery.events) {
      if (event.sequence > 0n) {
        skipping = event.sequence <= (cursors.get(event.serverId) ?? 0n);
      }
      if (skipping) continue;
      try {
        await options.onEvent?.(event, delivery);
      } catch (err) {
        report(err);
      }
      const interaction = event.payload.case === "interactionCreated" ? event.payload.value.interaction : undefined;
      if (interaction && options.onInteraction && interaction.agentId === delivery.agentId) {
        const reply = (content: EndpointReply) => void replies.push(replyOf(interaction.id, content));
        try {
          const out = await options.onInteraction({
            interaction,
            event,
            serverId: interaction.serverId,
            channelId: interaction.channelId,
            userId: interaction.userId,
            kind: interaction.kind,
            command: interaction.command,
            options: Object.fromEntries(interaction.arguments.map((a) => [a.name, a.value])),
            customId: interaction.customId,
            messageId: interaction.messageId,
            reply,
          });
          if (out !== undefined && out !== null) reply(out);
        } catch (err) {
          report(err);
        }
      }
      if (event.sequence > 0n) cursors.set(event.serverId, event.sequence);
    }
    return json(toJson(AgentDeliveryAnswerSchema, create(AgentDeliveryAnswerSchema, { replies })));
  };
}
