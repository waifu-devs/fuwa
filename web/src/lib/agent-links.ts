/**
 * Agent invite links: `<instance>/agent/<username>`, on the instance itself
 * like server invites, so they open in any browser and land on the page
 * that adds the agent to a server you manage.
 */

const USERNAME = /^[a-z0-9][a-z0-9_.]{1,31}$/;

export const agentLink = (base: string, username: string) => `${base.replace(/\/+$/, "")}/agent/${username}`;

/**
 * Reads what someone pasted as an agent link: the instance's address as
 * written (empty for a bare `@username`, which means the one they're
 * looking at) and the username. Null if it isn't one.
 */
export function parseAgentLink(text: string): { address: string; username: string } | null {
  const pasted = text.trim().replace(/[?#].*$/, "").replace(/\/+$/, "");
  if (pasted.startsWith("@")) {
    const username = pasted.slice(1).toLowerCase();
    return USERNAME.test(username) ? { address: "", username } : null;
  }
  const at = pasted.lastIndexOf("/agent/");
  if (at <= 0) return null;
  const username = pasted.slice(at + "/agent/".length).replace(/^@/, "").toLowerCase();
  if (!USERNAME.test(username)) return null;
  return { address: pasted.slice(0, at), username };
}
