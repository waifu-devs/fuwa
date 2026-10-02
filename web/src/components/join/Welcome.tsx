import { useNavigate } from "@tanstack/react-router";
import { ArrowRightIcon, HashIcon, MegaphoneIcon } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useState } from "react";
import { ChannelType, Permission, type Channel, type Emoji, type Server, type WelcomeScreen } from "@/gen/fuwa/v1/types_pb";
import { getWelcomeScreen, run } from "@/fuwa/actions";
import { useAccess, useInstance, useMyMember } from "@/fuwa/hooks";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { ServerIcon } from "@/components/Icons";
import { InlineMarkdown } from "@/components/Markdown";
import { Dialog, DialogContent } from "@/components/ui/dialog";
import { toDate } from "@/lib/format";
import { has } from "@/lib/permissions";
import { cn } from "@/lib/utils";

/**
 * A server's welcome screen: its icon and name, a few words, and the
 * channels it suggests, each a card that pops in after the one before.
 */
export function WelcomeCard({
  server,
  screen,
  channels,
  emojis,
  onPick,
  className,
}: {
  server: Pick<Server, "id" | "name" | "iconUrl">;
  screen: WelcomeScreen;
  channels: Channel[];
  emojis: Emoji[] | undefined;
  onPick?: (channelId: string) => void;
  className?: string;
}) {
  const suggested = screen.channels.flatMap((w) => {
    const channel = channels.find((c) => c.id === w.channelId);
    return channel ? [{ ...w, channel }] : [];
  });
  return (
    <div className={cn("flex flex-col items-center gap-3 text-center", className)}>
      <motion.span
        initial={{ scale: 0.4, rotate: -18, y: 10 }}
        animate={{ scale: 1, rotate: 0, y: 0 }}
        transition={{ type: "spring", stiffness: 380, damping: 12 }}
        className="relative"
      >
        <ServerIcon server={server} active className="size-16 text-xl shadow-lg" />
        <motion.span
          aria-hidden
          initial={{ scale: 0, rotate: -60, opacity: 0 }}
          animate={{ scale: 1, rotate: 0, opacity: 1 }}
          transition={{ type: "spring", stiffness: 500, damping: 12, delay: 0.3 }}
          className="absolute -top-2 -right-3 text-xl"
        >
          👋
        </motion.span>
      </motion.span>
      <div className="min-w-0">
        <p className="text-xs font-bold tracking-wide text-muted-foreground uppercase">Welcome to</p>
        <h2 className="text-xl font-extrabold break-words">{server.name}</h2>
      </div>
      {screen.description.trim() && (
        <motion.p initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} transition={{ delay: 0.12 }} className="text-sm break-words text-muted-foreground">
          <InlineMarkdown>{screen.description}</InlineMarkdown>
        </motion.p>
      )}
      {suggested.length > 0 && (
        <div className="mt-1 flex w-full flex-col gap-2 text-left">
          <p className="text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">Start here</p>
          {suggested.map((w, n) => (
            <motion.button
              key={w.channelId}
              type="button"
              disabled={!onPick}
              onClick={() => onPick?.(w.channelId)}
              initial={{ opacity: 0, y: 14, scale: 0.96 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              transition={{ type: "spring", stiffness: 420, damping: 24, delay: 0.18 + n * 0.07 }}
              whileHover={onPick ? { x: 4 } : undefined}
              whileTap={onPick ? { scale: 0.98 } : undefined}
              className="group flex items-center gap-3 rounded-2xl border bg-background/60 p-3 text-left transition-colors enabled:hover:border-primary/40 enabled:hover:bg-primary/5"
            >
              <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-primary/10 text-xl transition-transform group-hover:scale-110 group-hover:-rotate-6">
                {w.emoji ? (
                  <EmojiGlyph value={w.emoji} emojis={emojis} className="size-6" />
                ) : w.channel.type === ChannelType.ANNOUNCEMENT ? (
                  <MegaphoneIcon className="size-5 text-primary" />
                ) : (
                  <HashIcon className="size-5 text-primary" />
                )}
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate text-sm font-bold">#{w.channel.name}</span>
                {w.description && <span className="block truncate text-xs text-muted-foreground">{w.description}</span>}
              </span>
              {onPick && <ArrowRightIcon className="size-4 shrink-0 text-muted-foreground transition group-hover:translate-x-1 group-hover:text-primary" />}
            </motion.button>
          ))}
        </div>
      )}
    </div>
  );
}

const SEEN = (instanceKey: string, serverId: string) => `fuwa.welcomed.${instanceKey}.${serverId}`;
/** New members are people who joined in the last week. */
const NEW_FOR = 7 * 86_400_000;

function seen(instanceKey: string, serverId: string) {
  try {
    return localStorage.getItem(SEEN(instanceKey, serverId)) !== null;
  } catch {
    return true;
  }
}

function markSeen(instanceKey: string, serverId: string) {
  try {
    localStorage.setItem(SEEN(instanceKey, serverId), "1");
  } catch {
    // Without storage it shows again next time, which is harmless.
  }
}

/**
 * Greets new members with the server's welcome screen, once, after they've
 * agreed to any rules. People who can change it never get it unasked, and
 * anyone can open it again from the server menu.
 */
export function WelcomeGate({
  instanceKey,
  server,
  open: asked,
  onOpenChange,
}: {
  instanceKey: string;
  server: Server;
  /** Opened from the server menu. */
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const inst = useInstance(instanceKey);
  const me = useMyMember(instanceKey, server.id);
  const access = useAccess(instanceKey, server.id);
  const navigate = useNavigate();
  const [screen, setScreen] = useState<WelcomeScreen | null>(null);
  const [greeting, setGreeting] = useState(false);
  const channels = inst?.channels[server.id] ?? [];

  const newcomer =
    !!me && !me.pending && !has(access, Permission.MANAGE_SERVER) && Date.now() - toDate(me.joinedAt).getTime() < NEW_FOR && !seen(instanceKey, server.id);
  const wanted = asked || (server.hasWelcomeScreen && newcomer);

  useEffect(() => {
    if (!wanted) return;
    let cancelled = false;
    run(getWelcomeScreen(instanceKey, server.id)).then(
      (s) => {
        if (cancelled) return;
        setScreen(s);
        if (!asked) {
          markSeen(instanceKey, server.id);
          if (s.enabled) setGreeting(true);
        }
      },
      () => {},
    );
    return () => {
      cancelled = true;
    };
  }, [wanted, asked, instanceKey, server.id]);

  const open = (asked || greeting) && !!screen?.enabled;
  function close() {
    setGreeting(false);
    onOpenChange(false);
  }
  return (
    <Dialog open={open} onOpenChange={(o) => !o && close()}>
      <DialogContent className="overflow-hidden">
        <div className="pointer-events-none absolute inset-x-0 top-0 h-32 bg-gradient-to-b from-primary/20 to-transparent" />
        {screen && (
          <WelcomeCard
            className="relative"
            server={server}
            screen={screen}
            channels={channels}
            emojis={inst?.emojis[server.id]}
            onPick={(channel) => {
              close();
              void navigate({ to: "/$instance/$server/$channel", params: { instance: instanceKey, server: server.id, channel } });
            }}
          />
        )}
        <button type="button" onClick={close} className="relative mx-auto block w-fit justify-self-center text-sm font-bold text-muted-foreground transition hover:text-foreground">
          I'll look around myself
        </button>
      </DialogContent>
    </Dialog>
  );
}
