import type { CSSProperties } from "react";
import type { Connection } from "@/fuwa/store";
import { i18n, type Key } from "@/i18n/i18n";
import { hueOf } from "@/lib/format";

/** Sets `--h` to the hue that belongs to an id, for `.server-gradient`. */
export const hue = (id: string) => ({ "--h": hueOf(id) }) as CSSProperties;

export const CONNECTION_LABEL: Record<Connection, Key> = {
  connecting: "workspace.connection.connecting",
  live: "workspace.connection.live",
  reconnecting: "workspace.connection.reconnecting",
  offline: "workspace.connection.offline",
  "signed-out": "workspace.connection.signedOut",
};

/** In the app's language now; components re-render with useI18n when it changes. */
export const connectionLabel = (state: Connection) => i18n().t(CONNECTION_LABEL[state]);
