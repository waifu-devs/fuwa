import { motion } from "motion/react";
import type { CSSProperties } from "react";
import type { Server, User } from "@/gen/fuwa/v1/types_pb";
import type { Connection } from "@/fuwa/store";
import { displayName, hueOf, initials } from "@/lib/format";
import { cn } from "@/lib/utils";

const hue = (id: string) => ({ "--h": hueOf(id) }) as CSSProperties;

/** A server's picture, or its initials on a gradient of its own hue. */
export function ServerIcon({
  server,
  active = false,
  className,
}: {
  server: Pick<Server, "id" | "name" | "iconUrl">;
  active?: boolean;
  className?: string;
}) {
  return (
    <span
      data-active={active}
      style={hue(server.id)}
      className={cn(
        "server-icon server-gradient grid size-12 shrink-0 select-none place-items-center overflow-hidden text-base font-extrabold",
        className,
      )}
    >
      {server.iconUrl ? (
        <img src={server.iconUrl} alt="" className="size-full object-cover" draggable={false} />
      ) : (
        initials(server.name)
      )}
    </span>
  );
}

/** Someone's avatar, or their first letter on their hue. */
export function UserAvatar({ user, className }: { user: User | undefined; className?: string }) {
  const name = displayName(user);
  return (
    <span
      style={hue(user?.id ?? name)}
      className={cn(
        "server-gradient relative grid size-10 shrink-0 select-none place-items-center overflow-hidden rounded-full text-sm font-extrabold",
        className,
      )}
    >
      {user?.avatarUrl ? (
        <img src={user.avatarUrl} alt="" className="size-full object-cover" draggable={false} />
      ) : (
        initials(name).slice(0, 1)
      )}
    </span>
  );
}

const CONNECTION_LABEL: Record<Connection, string> = {
  connecting: "Connecting",
  live: "Connected",
  reconnecting: "Reconnecting",
  offline: "Offline",
  "signed-out": "Signed out",
};

/** Pops each time the connection changes, so a drop or a reconnect catches the eye. */
export function ConnDot({ state, className }: { state: Connection; className?: string }) {
  return (
    <motion.span
      key={state}
      initial={{ scale: 0.2 }}
      animate={{ scale: 1 }}
      transition={{ type: "spring", stiffness: 600, damping: 14 }}
      role="img"
      aria-label={CONNECTION_LABEL[state]}
      title={CONNECTION_LABEL[state]}
      data-state={state}
      className={cn("conn-dot", className)}
    />
  );
}

export const connectionLabel = (state: Connection) => CONNECTION_LABEL[state];

/** The fuwa mark: a soft cloud with a speech tail. */
export function FuwaMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 64 64" className={className} aria-hidden>
      <defs>
        <linearGradient id="fuwa-g" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="var(--primary)" />
          <stop offset="1" stopColor="color-mix(in srgb, var(--primary) 50%, #7dd3fc)" />
        </linearGradient>
      </defs>
      <path
        fill="url(#fuwa-g)"
        d="M20 50c-8.3 0-15-6.3-15-14.2 0-6.6 4.7-12.1 11-13.7C18.1 14.1 25.2 8 33.7 8c8.7 0 16 6.4 17.4 14.7C56.8 24 61 29 61 35c0 7.7-6.5 14-14.5 14H30l-9 8v-7z"
      />
      <circle cx="24" cy="32" r="3.2" fill="var(--primary-foreground)" />
      <circle cx="40" cy="32" r="3.2" fill="var(--primary-foreground)" />
      <path d="M28 39q4 3.5 8 0" stroke="var(--primary-foreground)" strokeWidth="2.6" fill="none" strokeLinecap="round" />
    </svg>
  );
}
