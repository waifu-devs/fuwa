// An agent in voice channels: /join and /leave, a parrot that says back what
// each person said once they pause, and /play for an Ogg Opus file.
//
//   FUWA_URL=https://fuwa.chat FUWA_TOKEN=... FUWA_SOUND=hello.ogg node examples/voice-agent.ts
//
// Then type "/join Lounge" in a text channel of a server the agent is in. It
// needs CONNECT and SPEAK in the voice channel. Sound comes and goes as Opus
// frames, so no codec is needed for this; make a file it can play with
//   ffmpeg -i in.wav -c:a libopus -frame_duration 20 hello.ogg
import { readFileSync } from "node:fs";
import { Agent, ChannelType, type VoiceConnection } from "@waifu-devs/fuwa";

const token = process.env.FUWA_TOKEN;
if (!token) {
  console.error("Set FUWA_TOKEN to the agent's token.");
  process.exit(1);
}
const agent = new Agent({ url: process.env.FUWA_URL ?? "http://localhost:8080", token });
const voices = new Map<string, VoiceConnection>(); // by server

agent.command("join", async (ctx) => {
  const { channels } = await agent.api.channels.listChannels({ serverId: ctx.serverId });
  const rooms = channels.filter((c) => c.type === ChannelType.VOICE);
  const room = rooms.find((c) => c.name.toLowerCase() === ctx.rest.toLowerCase()) ?? rooms[0];
  if (!room) return ctx.reply("There's no voice channel here.");
  await voices.get(ctx.serverId)?.leave();
  const voice = await agent.joinVoice(ctx.serverId, room.id);
  voices.set(ctx.serverId, voice);
  parrot(voice);
  voice.closed.then(
    () => voices.delete(ctx.serverId),
    (err) => {
      voices.delete(ctx.serverId);
      void ctx.send(`I left ${room.name}: ${err.message}`);
    },
  );
  return ctx.reply(`In ${room.name}. Say something and pause, and I'll say it back.`);
}, { description: "Joins a voice channel by name" });

agent.command("leave", async (ctx) => {
  await voices.get(ctx.serverId)?.leave();
  return ctx.reply("Left.");
}, { description: "Leaves the voice channel" });

agent.command("play", async (ctx) => {
  const voice = voices.get(ctx.serverId);
  if (!voice) return ctx.reply("I'm not in a voice channel here; `/join` first.");
  if (!process.env.FUWA_SOUND) return ctx.reply("Start me with FUWA_SOUND set to an .ogg file.");
  await voice.play(readFileSync(process.env.FUWA_SOUND));
}, { description: "Plays the sound file" });

/** Keeps each person's frames while they talk, and says them back when they stop. */
function parrot(voice: VoiceConnection) {
  const said = new Map<string, Uint8Array[]>();
  voice.on("frame", (frame) => {
    const frames = said.get(frame.userId) ?? [];
    if (frames.length < 500) frames.push(frame.opus); // ten seconds at most
    said.set(frame.userId, frames);
  });
  voice.on("silent", async (userId) => {
    const frames = said.get(userId);
    said.delete(userId);
    if (frames && frames.length > 10) await voice.speak(frames);
  });
}

agent.on("ready", ({ me }) => console.log(`Signed in as @${me.username}.`));
agent.on("error", (err) => console.error(err instanceof Error ? `${err.name}: ${err.message}` : err));
process.on("SIGINT", () => void agent.stop().then(() => process.exit(0)));

await agent.start();
await agent.closed;
