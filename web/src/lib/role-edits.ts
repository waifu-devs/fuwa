/**
 * Permission switches flipped on a role and not saved yet, as bits
 * (`lib/permissions.ts`). Someone else can save the same role meanwhile, so
 * what's kept is which switches were flipped here, never the whole set.
 */

/** The switches turned on and off here. */
export type Switches = { on: number; off: number };

export const NO_SWITCHES: Switches = { on: 0, off: 0 };

/** The role's permissions as they are `now`, with the switches flipped here on top. */
export const switched = (s: Switches, now: number): number => (now | s.on) & ~s.off;

/**
 * The switches after the permissions shown go from `from` to `to`. A switch
 * put back as the role has it `now` isn't being changed any more.
 */
export const flip = (s: Switches, from: number, to: number, now: number): Switches => {
  const on = to & ~from;
  const off = from & ~to;
  return { on: (s.on | on) & ~off & ~now, off: (s.off | off) & ~on & now };
};

/** What saving `mine` turns on and off in the role as it is `now`. */
export const changes = (mine: number, now: number) => ({ grant: mine & ~now, revoke: now & ~mine });

/**
 * The permissions part of a save. An instance with "role-permission-changes"
 * (`kept`) takes just the switches flipped; an older one ignores those and
 * only takes every permission at once.
 */
export const permissionPatch = (mine: number, now: number, kept: boolean): { grant?: number; revoke?: number; permissions?: number } => {
  const { grant, revoke } = changes(mine, now);
  if ((grant | revoke) === 0) return {};
  return kept ? { grant, revoke } : { permissions: mine };
};
