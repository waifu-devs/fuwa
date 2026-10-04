import type { Member, Server } from "@/gen/fuwa/v1/types_pb";
import type { ModAction } from "@/lib/permissions";
import { toast } from "@/lib/ui";

/** Which dialog a menu has open (components/menus/MenuDialogs.tsx draws them). */

export type Confirm = {
  title: string;
  body: string;
  /** The button's word, like "Leave". */
  action: string;
  /** What happens; a failure shows its message and leaves the dialog open. */
  run: () => Promise<unknown>;
};

type MenuDialog =
  | { kind: "moderate"; instanceKey: string; serverId: string; member: Member; action: ModAction }
  | { kind: "confirm"; confirm: Confirm }
  | { kind: "invite"; instanceKey: string; serverId: string; channelId: string }
  | { kind: "server-settings"; instanceKey: string; server: Server; tab: string; target?: string };

/** The dialog open now, and the last of each kind, kept while it closes. */
type State = { open: MenuDialog | null; last: Partial<Record<MenuDialog["kind"], MenuDialog>> };
let state: State = { open: null, last: {} };
export const menuDialogs = () => state;
const listeners = new Set<() => void>();
const set = (next: State) => {
  state = next;
  for (const l of listeners) l();
};
export const subscribeMenuDialogs = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

export const openMenuDialog = (dialog: MenuDialog) => set({ open: dialog, last: { ...state.last, [dialog.kind]: dialog } });
export const closeMenuDialog = () => set({ ...state, open: null });

/** Asks before something that can't be taken back, like leaving a server. */
export const confirmFirst = (confirm: Confirm) => openMenuDialog({ kind: "confirm", confirm });

/** Runs a menu's action and says so if it fails, since the menu is gone by then. */
export function attempt(work: Promise<unknown>) {
  work.catch((err: Error) => toast(err.message));
}
