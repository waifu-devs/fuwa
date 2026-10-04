/** A command someone wrote, such as "/dice 2d6" or "@helper dice 2d6". */
export interface ParsedCommand {
  /** Lowercased, without the prefix: "dice". */
  name: string;
  /** The words after it, split on whitespace: ["2d6"]. */
  args: string[];
  /** Everything after the name, as written (trimmed). */
  rest: string;
}

function escapeRegExp(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * Reads a command from a message's text: `<prefix><name> args...`, or the
 * agent's @username followed by `<name> args...` (with or without the
 * prefix). Returns undefined when the message isn't one.
 */
export function parseCommand(content: string, options: { prefix: string; username?: string }): ParsedCommand | undefined {
  let text = content.trim();
  if (options.username) {
    const lead = new RegExp(`^@${escapeRegExp(options.username)}(?![\\w.])[\\s,:]*`, "i").exec(text);
    if (lead) {
      text = text.slice(lead[0].length);
      if (options.prefix && text.startsWith(options.prefix)) text = text.slice(options.prefix.length);
      return split(text);
    }
  }
  if (!options.prefix || !text.startsWith(options.prefix)) return undefined;
  return split(text.slice(options.prefix.length));
}

function split(text: string): ParsedCommand | undefined {
  const m = /^([\p{L}\p{N}_-]+)(?:\s+([\s\S]*))?$/u.exec(text);
  if (!m) return undefined;
  const rest = (m[2] ?? "").trim();
  return { name: m[1]!.toLowerCase(), args: rest ? rest.split(/\s+/) : [], rest };
}

/**
 * Whether text mentions @username, the way the apps decide it (a whole
 * word, any case, not part of an email address).
 */
export function mentions(content: string, username: string): boolean {
  return new RegExp(`(^|[^\\w@])@${escapeRegExp(username)}(?![\\w.]*\\w)`, "i").test(content);
}

/** Writes a role mention the instance recognises: <@&role id>. */
export function roleMention(roleId: string): string {
  return `<@&${roleId}>`;
}

/** Writes a custom emoji the apps draw: <:name:id>, or <a:name:id> for a moving one. */
export function emoji(name: string, id: string, animated = false): string {
  return `<${animated ? "a" : ""}:${name}:${id}>`;
}
