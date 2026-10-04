// An agent you talk with in a voice channel: when someone pauses, what they
// said goes to a speech-to-text service, a language model answers, and the
// answer is spoken back sentence by sentence as it's written. Talk over it
// and it stops to listen.
//
//   FUWA_URL=https://fuwa.chat FUWA_TOKEN=... SPEECH_API_KEY=... node examples/voice-chat.ts
//
// Then type "/talk Lounge" in a text channel of a server the agent is in (it
// needs CONNECT and SPEAK there), and "/bye" to send it away.
//
// The speech services are any with OpenAI's API shape (SPEECH_API_URL,
// default https://api.openai.com/v1; the models are settings below). The SDK
// itself sends nothing anywhere but the instance: this example is what sends
// people's voices and display names to the service, so it says so in the
// channel when it joins. It logs fixed text only, never what was said.
// The answer's sound must come as Ogg Opus in 20 ms frames; for a service
// that only makes raw PCM, use voice.speakPcm with an Opus encoder instead.
import { Agent, ChannelType, type Utterance, type VoiceConnection } from "@waifu-devs/fuwa";

const token = process.env.FUWA_TOKEN;
const key = process.env.SPEECH_API_KEY;
if (!token || !key) {
  console.error("Set FUWA_TOKEN to the agent's token and SPEECH_API_KEY to the speech service's key.");
  process.exit(1);
}
const api = (process.env.SPEECH_API_URL ?? "https://api.openai.com/v1").replace(/\/$/, "");
if (new URL(api).username || new URL(api).password) {
  console.error("Put the speech service's key in SPEECH_API_KEY, not in SPEECH_API_URL.");
  process.exit(1);
}
const models = {
  listen: process.env.SPEECH_LISTEN_MODEL ?? "gpt-4o-mini-transcribe",
  think: process.env.SPEECH_THINK_MODEL ?? "gpt-4o-mini",
  speak: process.env.SPEECH_SPEAK_MODEL ?? "gpt-4o-mini-tts",
  voice: process.env.SPEECH_VOICE ?? "alloy",
};

const agent = new Agent({ url: process.env.FUWA_URL ?? "http://localhost:8080", token });
const calls = new Map<string, VoiceConnection>(); // by server

agent.command("talk", async (ctx) => {
  const { channels } = await agent.api.channels.listChannels({ serverId: ctx.serverId });
  const rooms = channels.filter((c) => c.type === ChannelType.VOICE);
  const room = rooms.find((c) => c.name.toLowerCase() === ctx.rest.toLowerCase()) ?? rooms[0];
  if (!room) return ctx.reply("There's no voice channel here.");
  await calls.get(ctx.serverId)?.leave();
  const voice = await agent.joinVoice(ctx.serverId, room.id, { utteranceGapMs: 700 });
  calls.set(ctx.serverId, voice);
  converse(voice);
  voice.closed.finally(() => calls.delete(ctx.serverId)).catch(() => {});
  return ctx.reply(
    `Listening in ${room.name}. What you say there, with your display name, goes to ${new URL(api).host} to be understood and answered.`,
  );
}, { description: "Joins a voice channel to talk" });

agent.command("bye", async (ctx) => {
  await calls.get(ctx.serverId)?.leave();
  return ctx.reply("Bye!");
}, { description: "Leaves the voice channel" });

/** Answers each utterance in turn; someone talking over the answer stops it. */
function converse(voice: VoiceConnection) {
  const history: { role: "system" | "user" | "assistant"; content: string }[] = [
    { role: "system", content: "You're in a voice chat. Answer briefly and conversationally, in plain spoken sentences." },
  ];
  let answering: AbortController | undefined;

  // Barge-in: anyone starting to talk while it speaks stops the answer and
  // everything queued after it.
  voice.on("speaking", () => {
    if (!voice.talking) return;
    answering?.abort();
    voice.stopSpeaking();
  });

  voice.on("utterance", async (utterance: Utterance) => {
    await utterance.ended;
    if (utterance.durationMs < 300) return; // a cough
    answering?.abort();
    const turn = (answering = new AbortController());
    try {
      const text = await transcribe(await utterance.toOgg(), turn.signal);
      if (!text.trim()) return;
      const name = (await agent.user(utterance.userId))?.displayName ?? "someone";
      history.push({ role: "user", content: `${name}: ${text}` });
      let answer = "";
      // Each sentence is spoken as soon as it's written; they wait their turn.
      for await (const sentence of sentences(think(history, turn.signal))) {
        answer += sentence;
        void voice.play(say(sentence, turn.signal), { signal: turn.signal }).catch(() => {});
      }
      history.push({ role: "assistant", content: answer });
      // The last twenty turns are plenty for a chat, and keep the history bounded.
      if (history.length > 41) history.splice(1, history.length - 41);
    } catch (err) {
      // Fixed text only: an error from a service can quote what was said.
      if (!turn.signal.aborted) console.error(err instanceof SpeechError ? err.message : "couldn't reach the speech service");
    }
  });
}

async function transcribe(ogg: Uint8Array<ArrayBuffer>, signal: AbortSignal): Promise<string> {
  const form = new FormData();
  form.append("model", models.listen);
  form.append("file", new Blob([ogg], { type: "audio/ogg" }), "speech.ogg");
  const res = await call("/audio/transcriptions", { method: "POST", body: form, signal });
  const text = parse(await res.text())?.text;
  if (typeof text !== "string") throw new SpeechError("the speech service sent an unexpected transcription");
  return text;
}

/** The model's answer as it's written. */
async function* think(messages: { role: string; content: string }[], signal: AbortSignal): AsyncGenerator<string> {
  const res = await call("/chat/completions", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ model: models.think, messages, stream: true }),
    signal,
  });
  let buffered = "";
  for await (const chunk of res.body!.pipeThrough(new TextDecoderStream())) {
    buffered += chunk;
    const lines = buffered.split("\n");
    buffered = lines.pop()!;
    if (buffered.length > 64 * 1024) throw new SpeechError("the language model sent an unexpected answer");
    for (const line of lines) {
      if (!line.startsWith("data: ") || line === "data: [DONE]") continue;
      const delta = parse(line.slice(6))?.choices?.[0]?.delta?.content;
      if (typeof delta === "string") yield delta;
    }
  }
}

/** Speech for some text, as an Ogg Opus stream that plays as it arrives. */
async function* say(text: string, signal: AbortSignal): AsyncGenerator<Uint8Array> {
  const res = await call("/audio/speech", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ model: models.speak, voice: models.voice, input: text, response_format: "opus" }),
    signal,
  });
  yield* res.body!;
}

/** Text in pieces, as whole sentences (or 400 characters, when a sentence runs on). */
async function* sentences(pieces: AsyncIterable<string>): AsyncGenerator<string> {
  let text = "";
  for await (const piece of pieces) {
    text += piece;
    for (let end = text.search(/[.!?…]\s/); end >= 0; end = text.search(/[.!?…]\s/)) {
      yield text.slice(0, end + 2);
      text = text.slice(end + 2);
    }
    if (text.length > 400) {
      const cut = text.lastIndexOf(" ") > 0 ? text.lastIndexOf(" ") + 1 : text.length;
      yield text.slice(0, cut);
      text = text.slice(cut);
    }
  }
  if (text.trim()) yield text;
}

/** An error whose message is fixed text, safe to print. */
class SpeechError extends Error {}

/** JSON, or undefined: JSON.parse's own errors quote the text. */
function parse(text: string): any {
  try {
    return JSON.parse(text);
  } catch {
    return undefined;
  }
}

async function call(path: string, init: RequestInit): Promise<Response> {
  const res = await fetch(api + path, { ...init, headers: { ...init.headers, authorization: `Bearer ${key}` } });
  // The service's own words can be long; its status says enough.
  if (!res.ok) throw new SpeechError(`the speech service answered ${res.status} to ${path}`);
  return res;
}

agent.on("ready", ({ me }) => console.log(`Signed in as @${me.username}.`));
agent.on("error", (err) => console.error(err instanceof Error ? `${err.name}: ${err.message}` : err));
process.on("SIGINT", () => void agent.stop().then(() => process.exit(0)));

await agent.start();
await agent.closed;
