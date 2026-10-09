/**
 * The official TypeScript SDK for fuwa (https://github.com/waifu-devs/fuwa).
 *
 * - `createFuwa` gives typed clients for every service on an instance.
 * - `Agent` runs an agent: events, messages, mentions and commands.
 * - `joinVoice` (or `agent.joinVoice`) hears and talks in voice channels,
 *   with Ogg Opus files read and written by `readOggOpus` and `OggOpusWriter`.
 * - `EventFollower`, `messages`, `listEvents` and `uploadPicture` are the
 *   pieces it's built from, for apps that want them on their own.
 *
 * Every message type and enum is exported too; the generated files are also
 * at "@waifu-devs/fuwa/gen/<file>", such as "@waifu-devs/fuwa/gen/types".
 */
export { createFuwa, instanceUrl, type Fuwa, type FuwaOptions, type ServerVersion } from "./client.js";
export {
  Agent,
  type AgentEventName,
  type AgentHandlers,
  type AgentOptions,
  type CommandContext,
  type CommandInfo,
  type InteractionContext,
  type LiveTileHandle,
  type MessageContext,
  type Outgoing,
  type ReactionEmoji,
} from "./agent.js";
export {
  AlreadyExistsError,
  CanceledError,
  Code,
  FailedPreconditionError,
  FuwaError,
  InvalidArgumentError,
  NotFoundError,
  PermissionDeniedError,
  RateLimitedError,
  ServerError,
  TimeoutError,
  UnauthenticatedError,
  UnavailableError,
  retryAfterOf,
  toFuwaError,
} from "./errors.js";
export { EventFollower, listEvents, type EventKind, type EventPayload, type FollowOptions, type FollowUpdate } from "./events.js";
export { messagePages, messages, type MessagePagesOptions, type MessageWithAuthor } from "./pages.js";
export { DEFAULT_RETRY, type RetryOptions } from "./retry.js";
export { emoji, mentions, parseCommand, roleMention, type ParsedCommand } from "./text.js";
export { uploadPicture, type UploadOptions } from "./upload.js";
export {
  Utterance,
  VoiceConnection,
  joinVoice,
  type JoinVoiceOptions,
  type OggSource,
  type SpeakOptions,
  type SpeakResult,
  type VoiceEvents,
  type VoiceFrame,
} from "./voice.js";
export {
  OggOpusReader,
  OggOpusWriter,
  oggOpusPackets,
  opusPacketDuration,
  readOggOpus,
  splitOpusPacket,
  type OpusHead,
} from "./ogg.js";
export {
  pcmFrames,
  pcmFromBytes,
  pcmToBytes,
  type OpusDecoder,
  type OpusEncoder,
  type OpusSampleRate,
  type PcmFormat,
} from "./pcm.js";
export { SDK_VERSION } from "./version.js";

export * from "./gen/fuwa/v1/account_pb.js";
export * from "./gen/fuwa/v1/admin_pb.js";
export * from "./gen/fuwa/v1/agent_pb.js";
export * from "./gen/fuwa/v1/auth_pb.js";
export * from "./gen/fuwa/v1/automod_pb.js";
export * from "./gen/fuwa/v1/call_pb.js";
export * from "./gen/fuwa/v1/channel_pb.js";
export * from "./gen/fuwa/v1/command_pb.js";
export * from "./gen/fuwa/v1/dm_pb.js";
export * from "./gen/fuwa/v1/emoji_pb.js";
export * from "./gen/fuwa/v1/event_pb.js";
export * from "./gen/fuwa/v1/friend_pb.js";
export * from "./gen/fuwa/v1/gif_pb.js";
export * from "./gen/fuwa/v1/invite_pb.js";
export * from "./gen/fuwa/v1/join_pb.js";
export * from "./gen/fuwa/v1/live_tile_pb.js";
export * from "./gen/fuwa/v1/media_pb.js";
export * from "./gen/fuwa/v1/message_pb.js";
export * from "./gen/fuwa/v1/node_pb.js";
export * from "./gen/fuwa/v1/presence_pb.js";
export * from "./gen/fuwa/v1/role_pb.js";
export * from "./gen/fuwa/v1/search_pb.js";
export * from "./gen/fuwa/v1/secure_pb.js";
export * from "./gen/fuwa/v1/server_pb.js";
export * from "./gen/fuwa/v1/sso_pb.js";
export * from "./gen/fuwa/v1/types_pb.js";
export * from "./gen/fuwa/v1/webhook_pb.js";
