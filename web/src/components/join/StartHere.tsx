import { ArrowRightIcon, HashIcon, MegaphoneIcon } from "lucide-react";
import { motion } from "motion/react";
import { ChannelType, type Channel, type Emoji, type WelcomeScreen } from "@/gen/fuwa/v1/types_pb";
import { EmojiGlyph } from "@/components/EmojiGlyph";
import { useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";

/** A suggested channel as a card: its emoji or kind, its name and why to go there. */
export type Suggested = { channelId: string; description: string; emoji: string; channel: Channel };

/** The welcome screen's channels that exist and the caller can see, with their channel. */
export function suggestedChannels(screen: Pick<WelcomeScreen, "channels">, channels: Channel[]): Suggested[] {
  return screen.channels.flatMap((w) => {
    const channel = channels.find((c) => c.id === w.channelId);
    return channel ? [{ channelId: w.channelId, description: w.description, emoji: w.emoji, channel }] : [];
  });
}

/** "Start here": channel cards that rise in one after another, two to a row where there's room. */
export function StartHere({
  suggested,
  emojis,
  onPick,
  wide = false,
  label,
}: {
  suggested: Suggested[];
  emojis: Emoji[] | undefined;
  onPick?: (channelId: string) => void;
  /** Two columns. */
  wide?: boolean;
  label?: string;
}) {
  const { t } = useI18n();
  if (suggested.length === 0) return null;
  return (
    <div className="flex w-full flex-col gap-2 text-left">
      <p className="text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">{label ?? t("join.startHere")}</p>
      <div className={cn("grid gap-2", wide && "sm:grid-cols-2")}>
        {suggested.map((w, n) => (
          <motion.button
            key={w.channelId}
            type="button"
            disabled={!onPick}
            onClick={() => onPick?.(w.channelId)}
            initial={{ opacity: 0, y: 16, scale: 0.96 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            transition={{ type: "spring", stiffness: 420, damping: 26, delay: 0.22 + n * 0.06 }}
            whileHover={onPick ? { y: -3 } : undefined}
            whileTap={onPick ? { scale: 0.97 } : undefined}
            className="group flex items-center gap-3 rounded-2xl border bg-background/60 p-3 text-left transition-colors enabled:hover:border-[color-mix(in_srgb,var(--accent-server)_55%,transparent)] enabled:hover:bg-[color-mix(in_srgb,var(--accent-server)_8%,transparent)]"
          >
            <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-[color-mix(in_srgb,var(--accent-server)_16%,transparent)] text-xl transition-transform duration-300 group-hover:scale-110 group-hover:-rotate-6">
              {w.emoji ? (
                <EmojiGlyph value={w.emoji} emojis={emojis} className="size-6" />
              ) : w.channel.type === ChannelType.ANNOUNCEMENT ? (
                <MegaphoneIcon className="size-5 text-[var(--accent-server)]" />
              ) : (
                <HashIcon className="size-5 text-[var(--accent-server)]" />
              )}
            </span>
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm font-bold">#{w.channel.name}</span>
              {w.description && <span className="block truncate text-xs text-muted-foreground">{w.description}</span>}
            </span>
            {onPick && <ArrowRightIcon className="size-4 shrink-0 text-muted-foreground transition group-hover:translate-x-1 group-hover:text-[var(--accent-server)]" />}
          </motion.button>
        ))}
      </div>
    </div>
  );
}
