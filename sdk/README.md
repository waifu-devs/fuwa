# @waifu-devs/fuwa

The official TypeScript SDK for [fuwa](https://github.com/waifu-devs/fuwa):
typed clients for every service, and agents (bots) that follow events, answer
commands and mentions, post messages, and hold spoken conversations in voice
channels (each person's utterances in, streamed speech out, barge-in), with reconnecting and catching up handled. Node 20+ and browsers, ES modules and CommonJS.

```ts
import { Agent } from "@waifu-devs/fuwa";

const agent = new Agent({ url: "https://fuwa.chat", token: process.env.FUWA_TOKEN! });
agent.command("ping", (ctx) => ctx.reply("pong"));
await agent.start();
```

An agent's token is a secret: keep it in the environment or a secret store,
and reset it if it leaks. The SDK sends it only to the instance you name and
never logs it.

The guide is [docs/sdk.md](https://github.com/waifu-devs/fuwa/blob/master/docs/sdk.md),
and [examples/echo-agent.ts](https://github.com/waifu-devs/fuwa/blob/master/sdk/examples/echo-agent.ts)
is a whole agent to start from.
