// discord-migrate: a Discord bot, and a fuwa agent, that copy a Discord server
// into a fuwa server. A Discord admin runs /migrate start and gets a code; in
// the fuwa server (with the agent added as an admin) someone who manages it
// runs /import <code>. Before the code, they pick what to copy: the parts of
// the server, channels to leave out, and how much history. The bot then copies the server (migrate.ts), keeping a
// progress message up to date on both sides and posting a summary at the end.
//
//   DISCORD_TOKEN=... FUWA_TOKEN=... node src/main.ts
//
// DISCORD_TOKEN         the Discord bot's token (Developer Portal > Bot), a secret
// FUWA_URL              the fuwa instance (https://fuwa.chat)
// FUWA_TOKEN            the fuwa agent's token (Settings > Agents), a secret
// DATA_DIR              where each migration's progress is kept, to pick up after a stop (./data)
// MESSAGE_BUDGET        about how many messages /migrate copies in all unless told, shared over the channels (20000)
// MESSAGES_PER_CHANNEL  the most of each channel's latest messages /migrate copies unless told (1000)
// MAX_MESSAGES          the most /migrate may be asked for per channel (50000)
// CHANNELS_AT_ONCE      how many channels' histories are copied at once (4)
// MAX_FILE_MB           files bigger than this are named instead of copied (25)
import { Agent, CommandOptionType, type Message, Permission } from "@waifu-devs/fuwa";
import {
  ActionRowBuilder,
  ApplicationFlags,
  ButtonBuilder,
  ButtonStyle,
  ChannelSelectMenuBuilder,
  ChannelType,
  type ChatInputCommandInteraction,
  Client,
  Events,
  GatewayIntentBits,
  type Guild,
  InteractionContextType,
  MessageFlags,
  OAuth2Scopes,
  PermissionFlagsBits,
  SlashCommandBuilder,
  StringSelectMenuBuilder,
} from "discord.js";
import { type Choice, PART_NAMES, choiceText, perChannel, problems } from "./choose.ts";
import { DiscordSource } from "./discord.ts";
import { FuwaTarget } from "./fuwa.ts";
import { Links, type Pending } from "./link.ts";
import { NotReadyError, PARTS, type Part, migrate, newReport, reason } from "./migrate.ts";
import { FileStore } from "./state.ts";
import { progressText, stoppedText, summaryText } from "./summary.ts";

const env = process.env;
const discordToken = env.DISCORD_TOKEN;
const fuwaToken = env.FUWA_TOKEN;
if (!discordToken || !fuwaToken) {
  console.error("Set DISCORD_TOKEN to the Discord bot's token and FUWA_TOKEN to the fuwa agent's.");
  process.exit(1);
}
const whole = (name: string, fallback: number) => {
  const n = Number(env[name] || fallback);
  if (!Number.isSafeInteger(n) || n < 0) {
    console.error(`Set ${name} to a whole number from 0 up.`);
    process.exit(1);
  }
  return n;
};
const maxMessages = whole("MAX_MESSAGES", 50_000);
const defaultMessages = Math.min(whole("MESSAGES_PER_CHANNEL", 1000), maxMessages);
const messageBudget = whole("MESSAGE_BUDGET", 20_000);
const channelsAtOnce = Math.max(1, whole("CHANNELS_AT_ONCE", 4));
const maxFileBytes = whole("MAX_FILE_MB", 25) * 1024 * 1024;
const store = new FileStore(env.DATA_DIR || "data");
const links = new Links();
/** Progress messages are edited at most this often. */
const EDIT_EVERY_MS = 15_000;
/** How long the picker /migrate start shows keeps working (Discord's own limit is 15 minutes). */
const PICK_MS = 14 * 60_000;
const DAY_MS = 24 * 60 * 60_000;
/** Channels whose history is copied, and channels that can be left out. */
const TEXT_KINDS = [
  ChannelType.GuildText,
  ChannelType.GuildAnnouncement,
  ChannelType.GuildForum,
  ChannelType.GuildMedia,
];
const PICKABLE = [...TEXT_KINDS, ChannelType.GuildCategory, ChannelType.GuildVoice, ChannelType.GuildStageVoice];

/** Running migrations, by Discord guild and by fuwa server. */
const byGuild = new Map<string, AbortController>();
const byServer = new Set<string>();

const agent = new Agent({
  url: env.FUWA_URL || "https://fuwa.chat",
  token: fuwaToken,
  onError: (err) => console.error("fuwa:", reason(err)),
});
const discord = new Client({ intents: [GatewayIntentBits.Guilds] });

// Discord: /migrate start | cancel, for the server's administrators.
const migrateCommand = new SlashCommandBuilder()
  .setName("migrate")
  .setDescription("Copy this server to fuwa")
  .setDefaultMemberPermissions(PermissionFlagsBits.Administrator)
  .setContexts(InteractionContextType.Guild)
  .addSubcommand((s) =>
    s
      .setName("start")
      .setDescription("Get a code to link this server to a fuwa server, and copy it there")
      .addIntegerOption((o) =>
        o
          .setName("messages")
          .setDescription("Latest messages to copy per channel (0 for none; unless set, fewer the more channels)")
          .setMinValue(0)
          .setMaxValue(maxMessages),
      )
      .addIntegerOption((o) =>
        o
          .setName("days")
          .setDescription("Only copy messages from the last this many days")
          .setMinValue(1)
          .setMaxValue(36_500),
      )
      .addChannelOption((o) =>
        o
          .setName("announce")
          .setDescription("A channel to post the fuwa invite in when it's done")
          .addChannelTypes(ChannelType.GuildText, ChannelType.GuildAnnouncement),
      ),
  )
  .addSubcommand((s) =>
    s.setName("cancel").setDescription("Stop copying this server; run start again to pick up later"),
  );

/** Whether the bot may read messages' text: Discord's Message Content intent, set in the Developer Portal. */
async function readsMessages(): Promise<boolean> {
  const app = await discord.application!.fetch();
  const flags = app.flags;
  return flags.has(ApplicationFlags.GatewayMessageContent) || flags.has(ApplicationFlags.GatewayMessageContentLimited);
}

discord.once(Events.ClientReady, async (client) => {
  await client.application.commands.set([migrateCommand.toJSON()]);
  const invite = client.generateInvite({
    scopes: [OAuth2Scopes.Bot, OAuth2Scopes.ApplicationsCommands],
    permissions: [
      PermissionFlagsBits.ViewChannel,
      PermissionFlagsBits.ReadMessageHistory,
      PermissionFlagsBits.SendMessages,
      PermissionFlagsBits.EmbedLinks,
    ],
  });
  console.log(`Discord: signed in as ${client.user.tag}. Add it to a server with ${invite}`);
  if (!(await readsMessages())) {
    console.log(
      "Discord: the Message Content intent is off, so only servers' structure can be copied, not their messages.",
    );
  }
});

discord.on(Events.InteractionCreate, async (interaction) => {
  if (!interaction.isChatInputCommand() || interaction.commandName !== "migrate") return;
  try {
    if (interaction.options.getSubcommand() === "cancel") await cancel(interaction);
    else await start(interaction);
  } catch (err) {
    console.error("Discord: /migrate failed:", reason(err));
    const content = "Sorry, something went wrong. Try again in a moment.";
    await (
      interaction.replied || interaction.deferred
        ? interaction.followUp({ content, flags: MessageFlags.Ephemeral })
        : interaction.reply({ content, flags: MessageFlags.Ephemeral })
    ).catch(() => {});
  }
});

async function start(interaction: ChatInputCommandInteraction) {
  const private_ = (content: string) => interaction.reply({ content, flags: MessageFlags.Ephemeral });
  if (!interaction.inGuild() || !interaction.memberPermissions.has(PermissionFlagsBits.Administrator)) {
    await private_("Only the server's administrators can copy it to fuwa.");
    return;
  }
  if (byGuild.has(interaction.guildId)) {
    await private_("This server is already being copied. `/migrate cancel` stops it.");
    return;
  }
  const guild = interaction.guild ?? discord.guilds.cache.get(interaction.guildId);
  const textChannels = guild?.channels.cache.filter((c) => TEXT_KINDS.includes(c.type)).size ?? 0;
  let messages =
    interaction.options.getInteger("messages") ??
    perChannel(textChannels, messageBudget, defaultMessages);
  let note = "";
  if (messages > 0 && !(await readsMessages())) {
    messages = 0;
    note =
      "\n\nThis bot can't read messages' text (its Message Content intent is off), so no history can be copied.";
  }
  const choice: Choice = {
    parts: PARTS.filter((p) => p !== "history" || messages > 0),
    leaveOut: [],
    messagesPerChannel: messages,
    days: interaction.options.getInteger("days") ?? 0,
  };
  const offered = PARTS.filter((p) => p !== "history" || messages > 0);
  const picker = (warning = "") => ({
    content:
      "Choose what to copy to fuwa, then get your code.\n\n" +
      choiceText(choice) +
      note +
      (warning ? `\n\n**${warning}**` : ""),
    components: [
      new ActionRowBuilder<StringSelectMenuBuilder>().addComponents(
        new StringSelectMenuBuilder()
          .setCustomId("parts")
          .setPlaceholder("What to copy")
          .setMinValues(0)
          .setMaxValues(offered.length)
          .addOptions(
            offered.map((p) => ({ label: PART_NAMES[p], value: p, default: choice.parts.includes(p) })),
          ),
      ),
      new ActionRowBuilder<ChannelSelectMenuBuilder>().addComponents(
        new ChannelSelectMenuBuilder()
          .setCustomId("leave_out")
          .setPlaceholder("Channels or categories to leave out (optional)")
          .setChannelTypes(PICKABLE)
          .setMinValues(0)
          .setMaxValues(25)
          .setDefaultChannels(choice.leaveOut),
      ),
      new ActionRowBuilder<ButtonBuilder>().addComponents(
        new ButtonBuilder().setCustomId("go").setLabel("Get the code").setStyle(ButtonStyle.Primary),
        new ButtonBuilder().setCustomId("stop").setLabel("Cancel").setStyle(ButtonStyle.Secondary),
      ),
    ],
  });
  const response = await interaction.reply({ ...picker(), flags: MessageFlags.Ephemeral, withResponse: true });
  const message = response.resource?.message;
  if (!message) return;
  const collector = message.createMessageComponentCollector({ time: PICK_MS });
  collector.on("collect", async (i) => {
    try {
      if (i.isStringSelectMenu() && i.customId === "parts") {
        choice.parts = PARTS.filter((p) => i.values.includes(p));
        await i.update(picker());
      } else if (i.isChannelSelectMenu() && i.customId === "leave_out") {
        choice.leaveOut = [...i.values];
        await i.update(picker());
      } else if (i.isButton() && i.customId === "stop") {
        collector.stop("cancelled");
        await i.update({ content: "Cancelled. Nothing was copied.", components: [] });
      } else if (i.isButton() && i.customId === "go") {
        const wrong = problems(choice);
        if (wrong.length) {
          await i.update(picker(wrong.join(" ")));
          return;
        }
        if (byGuild.has(interaction.guildId)) {
          collector.stop("busy");
          const content = "This server is already being copied. `/migrate cancel` stops it.";
          await i.update({ content, components: [] });
          return;
        }
        collector.stop("done");
        await i.update({ content: codeText(interaction, choice), components: [] });
      }
    } catch (err) {
      console.error("Discord: the /migrate picker failed:", reason(err));
    }
  });
  collector.on("end", (_, why) => {
    if (why !== "time") return;
    void interaction
      .editReply({ content: "This ran out. Run `/migrate start` again when you're ready.", components: [] })
      .catch(() => {});
  });
}

/** Makes a code for what was chosen, and says what to do with it. */
function codeText(interaction: ChatInputCommandInteraction<"cached" | "raw">, choice: Choice): string {
  const history = choice.parts.includes("history") ? choice.messagesPerChannel : 0;
  const pending: Pending = {
    guildId: interaction.guildId,
    channelId: interaction.channelId,
    userId: interaction.user.id,
    messagesPerChannel: history,
    announceChannelId: interaction.options.getChannel("announce")?.id,
    parts: choice.parts,
    leaveOut: choice.leaveOut,
    since: choice.days ? Date.now() - choice.days * DAY_MS : undefined,
  };
  const code = links.create(pending);
  const name = agent.me.username;
  return (
    `Your code is **${code}**. It works once, for 30 minutes.\n\n` +
    "1. On fuwa, make a server for this one (or open one you manage).\n" +
    `2. Add **@${name}** to it (Server settings > Integrations) and give it a role with Administrator, at the top of the role list.\n` +
    `3. Run \`/import ${code}\` in any channel there.\n\n` +
    choiceText(choice) +
    "\nYou'll see its progress here and there."
  );
}

async function cancel(interaction: ChatInputCommandInteraction) {
  const running = interaction.inGuild() ? byGuild.get(interaction.guildId) : undefined;
  if (interaction.inGuild() && !interaction.memberPermissions.has(PermissionFlagsBits.Administrator)) {
    await interaction.reply({
      content: "Only the server's administrators can do that.",
      flags: MessageFlags.Ephemeral,
    });
    return;
  }
  running?.abort(new Error("it was cancelled"));
  await interaction.reply({
    content: running ? "Stopping. Everything copied so far stays." : "Nothing is being copied from this server.",
    flags: MessageFlags.Ephemeral,
  });
}

// fuwa: /import <code>, as a slash command, or typed on instances without them.
const importCommand = {
  name: "import",
  description: "Copy a Discord server here, with the code /migrate start gave you there",
  options: [{ name: "code", description: "The code from Discord", type: CommandOptionType.STRING, required: true }],
};
const setCommands = (serverId: string) =>
  agent
    .setCommands(serverId, [importCommand])
    .catch((err) => console.error("fuwa: setting commands failed:", reason(err)));
agent.on("ready", ({ servers }) => servers.forEach((s) => void setCommands(s)));
agent.on("serverAdded", (serverId) => void setCommands(serverId));
agent.on("interaction", async (ctx) => {
  if (ctx.command !== "import") return;
  await importHere(ctx.serverId, ctx.channelId, ctx.userId, ctx.options.code ?? "", (text) => ctx.reply(text));
});
agent.command(
  "import",
  (ctx) => importHere(ctx.serverId, ctx.channelId, ctx.message.authorId, ctx.rest, (text) => ctx.reply(text)),
  {
    description: "Copy a Discord server here",
  },
);

/** Whether someone may manage the server: its owner, or a role with Administrator or Manage Server. */
async function manages(serverId: string, userId: string): Promise<boolean> {
  const [{ server }, { members }, { roles }] = await Promise.all([
    agent.api.servers.getServer({ serverId }),
    agent.api.servers.listMembers({ serverId }),
    agent.api.roles.listRoles({ serverId }),
  ]);
  if (server?.ownerId === userId) return true;
  const theirs = new Set([serverId, ...(members.find((m) => m.user?.id === userId)?.roleIds ?? [])]);
  return roles.some(
    (r) =>
      theirs.has(r.id) &&
      (r.permissions.includes(Permission.ADMINISTRATOR) || r.permissions.includes(Permission.MANAGE_SERVER)),
  );
}

async function importHere(
  serverId: string,
  channelId: string,
  userId: string,
  code: string,
  reply: (text: string) => Promise<Message>,
) {
  try {
    if (!(await manages(serverId, userId))) {
      await reply("Only people who manage this server can copy a Discord server into it.");
      return;
    }
    if (byServer.has(serverId)) {
      await reply("A Discord server is already being copied here.");
      return;
    }
    const target = new FuwaTarget(agent, serverId);
    const missing = await target.missing();
    if (missing.length) {
      await reply(missing.join("\n"));
      return;
    }
    if (!code.trim()) {
      await reply("Run `/migrate start` in the Discord server first, then `/import` here with the code it gives you.");
      return;
    }
    const redeemed = links.redeem(code, serverId);
    if (!redeemed.ok) {
      await reply(
        redeemed.why === "slow down"
          ? "That's a few wrong codes in a row. Wait ten minutes, then try again."
          : "That code didn't work. Codes work once, for 30 minutes: run `/migrate start` in Discord for a new one.",
      );
      return;
    }
    const pending = redeemed.pending;
    const guild = discord.guilds.cache.get(pending.guildId);
    if (!guild) {
      await reply("The bot isn't in that Discord server anymore. Add it again and run `/migrate start` there.");
      return;
    }
    if (byGuild.has(guild.id)) {
      await reply("That Discord server is already being copied.");
      return;
    }
    await run(pending, guild, target, channelId, reply);
  } catch (err) {
    console.error("fuwa: /import failed:", reason(err));
    await reply("Sorry, something went wrong. Try again in a moment.").catch(() => {});
  }
}

/** A progress message that's edited at most every EDIT_EVERY_MS, and always with the last text. */
class LiveMessage {
  #text = "";
  #shown = "";
  #timer: NodeJS.Timeout | undefined;
  #last = 0;
  readonly #edit: (text: string) => Promise<unknown>;

  constructor(edit: (text: string) => Promise<unknown>) {
    this.#edit = edit;
  }

  set(text: string) {
    this.#text = text;
    if (this.#timer) return;
    const wait = Math.max(0, this.#last + EDIT_EVERY_MS - Date.now());
    this.#timer = setTimeout(() => void this.flush(), wait);
  }

  async flush() {
    clearTimeout(this.#timer);
    this.#timer = undefined;
    if (this.#text === this.#shown) return;
    this.#last = Date.now();
    this.#shown = this.#text;
    await this.#edit(this.#text).catch((err) => console.error("Editing progress failed:", reason(err)));
  }
}

async function run(
  pending: Pending,
  guild: Guild,
  target: FuwaTarget,
  fuwaChannelId: string,
  reply: (text: string) => Promise<Message>,
) {
  const controller = new AbortController();
  byGuild.set(guild.id, controller);
  byServer.add(target.serverId);
  const parts: Part[] = pending.parts ?? [...PARTS];
  const history = pending.messagesPerChannel > 0 && parts.includes("history");
  const report = newReport(guild.name);
  report.parts = parts.filter((p) => p !== "history" || history);
  try {
    const fuwaPost = await reply(progressText(report, history));
    const discordChannel = await discord.channels.fetch(pending.channelId).catch(() => null);
    const sendable = discordChannel?.isSendable() ? discordChannel : undefined;
    const discordPost = await sendable?.send(progressText(report, history)).catch(() => undefined);
    const live = [
      new LiveMessage((text) => agent.edit(fuwaPost, text)),
      ...(discordPost ? [new LiveMessage((text) => discordPost.edit(text))] : []),
    ];
    const tell = async (text: string) => {
      await Promise.all(live.map((l) => l.flush()));
      await agent
        .send(target.serverId, fuwaChannelId, text)
        .catch((err) => console.error("fuwa: posting failed:", reason(err)));
      await sendable?.send(text).catch(() => {});
    };
    try {
      await migrate(
        new DiscordSource(guild),
        target,
        store,
        {
          messagesPerChannel: pending.messagesPerChannel,
          since: pending.since,
          parts,
          leaveOut: pending.leaveOut,
          channelsAtOnce,
          maxFileBytes,
          signal: controller.signal,
          onProgress: (r) => live.forEach((l) => l.set(progressText(r, history))),
        },
        report,
      );
      await tell(summaryText(report));
      if (pending.announceChannelId && report.invite) {
        const announce = await discord.channels.fetch(pending.announceChannelId).catch(() => null);
        if (announce?.isSendable()) {
          await announce
            .send(`**${guild.name}** is moving to fuwa! Join us there: ${report.invite}`)
            .catch((err) => console.error("Discord: announcing failed:", reason(err)));
        }
      }
    } catch (err) {
      const why =
        err instanceof NotReadyError
          ? err.message
          : controller.signal.aborted
            ? reason(controller.signal.reason)
            : reason(err);
      if (!controller.signal.aborted && !(err instanceof NotReadyError))
        console.error("Migration failed:", reason(err));
      await tell(stoppedText(report, why));
    }
  } finally {
    byGuild.delete(guild.id);
    byServer.delete(target.serverId);
  }
}

const shutdown = async () => {
  for (const c of byGuild.values()) c.abort(new Error("the bot is restarting"));
  await agent.stop().catch(() => {});
  await discord.destroy().catch(() => {});
  process.exit(0);
};
process.once("SIGTERM", () => void shutdown());
process.once("SIGINT", () => void shutdown());

await agent.start();
console.log(`fuwa: signed in as @${agent.me.username}.`);
await discord.login(discordToken);
