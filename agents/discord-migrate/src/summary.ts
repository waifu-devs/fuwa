// What the bot says about a migration, on both sides: the progress message it
// keeps editing, and the summary at the end. Pure.
import type { Phase, Report } from "./migrate.ts";

const STEPS: Phase[] = ["server", "roles", "emoji", "sounds", "channels", "invite", "history"];

const count = (n: number) => n.toLocaleString("en-US");
const plural = (n: number, one: string, many = `${one}s`) => `${count(n)} ${n === 1 ? one : many}`;

function stepText(step: Phase, r: Report): string {
  switch (step) {
    case "server":
      return "The server's name, icon and banner";
    case "roles":
      return plural(r.roles, "role");
    case "emoji":
      return `${plural(r.emojis, "emoji", "emoji")} and ${plural(r.stickers, "sticker")}`;
    case "sounds":
      return plural(r.sounds, "sound");
    case "channels":
      return plural(r.channels, "channel");
    case "invite":
      return "An invite for your members";
    default:
      return r.historyTotal
        ? `History: ${count(r.historyDone)} of ${plural(r.historyTotal, "channel")}, ${plural(r.messages, "message")}, ` +
            `${plural(r.threads, "thread")}, ${plural(r.files, "file")}`
        : "History";
  }
}

/** The progress message: each step done, going, or still to come. */
export function progressText(r: Report, history: boolean): string {
  const at = STEPS.indexOf(r.phase);
  const lines = [`Copying **${r.guildName || "the server"}** to fuwa…`];
  for (const [i, step] of STEPS.entries()) {
    if (step === "history" && !history) continue;
    const mark = r.phase === "done" || i < at ? "✅" : i === at ? "⏳" : "▫️";
    lines.push(`${mark} ${stepText(step, r)}`);
  }
  return lines.join("\n");
}

/** The summary at the end: what came over, the invite, and what didn't. */
export function summaryText(r: Report): string {
  const lines = [
    `Done! **${r.guildName}** is on fuwa: ${plural(r.roles, "role")}, ${plural(r.channels, "channel")}, ` +
      `${plural(r.emojis, "emoji", "emoji")}, ${plural(r.stickers, "sticker")}, ${plural(r.sounds, "sound")}, ` +
      `${plural(r.messages, "message")}${r.threads ? ` (with ${plural(r.threads, "thread")})` : ""}, ` +
      `${plural(r.files, "file")} and ${plural(r.pins, "pin")}.`,
  ];
  if (r.invite) lines.push("", `Invite your members with ${r.invite}`);
  if (r.skipped.length) {
    lines.push("", "Not copied:");
    for (const s of r.skipped) lines.push(`• ${s}`);
    if (r.skippedMore) lines.push(`• …and ${plural(r.skippedMore, "more thing")}`);
  }
  lines.push(
    "",
    "Never copied: members and their roles (people join with the invite), private threads, and who messages mention " +
      "(mentions of people show their names). Old messages show their author's name and picture, posted now, " +
      "with their reactions listed under them; replies in threads come from the bot, under each author's name.",
  );
  return lines.join("\n");
}

/** When a run stops early: why, and that running it again picks up where it stopped. */
export function stoppedText(r: Report, why: string): string {
  return (
    `Stopped copying **${r.guildName || "the server"}**: ${why}\n` +
    "Everything copied so far stays. Run /migrate again and use the new code in the same fuwa server to pick up where it stopped."
  );
}
