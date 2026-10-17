import { DicesIcon, DownloadIcon, SparklesIcon } from "lucide-react";
import { useMemo, useState } from "react";
import type { Message, ProfileItem, User } from "@/gen/fuwa/v1/types_pb";
import { useFuwa } from "@/fuwa/store";
import { imageName, savePullImage } from "@/components/economy/gachaImage";
import { hasArt, ItemArt } from "@/components/economy/shared";
import { useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";
import { bestCard, cardsOfShare, chipKey, type ShareCard, type ShareCards } from "@/lib/gacha-share";
import { cssColor } from "@/lib/permissions";
import { ITEM_DECORATION } from "@/lib/profile-items";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const CHIPS = { refunded: "chattools.economy.gacha.refunded", duplicate: "chattools.economy.gacha.duplicate", guaranteed: "chattools.economy.gacha.guaranteed" } as const;

/** The words on a pull's saved picture, in the reader's language. */
export function useImageLabels() {
  const { t } = useI18n();
  return (name: string, count: number) => ({
    subtitle: t("chattools.economy.gacha.imageSubtitle", { name, count }),
    chip: (key: keyof typeof CHIPS) => t(CHIPS[key]),
  });
}

/** Saves the picture of a pull's cards, saying so if it can't. */
export function useSaveImage() {
  const { t } = useI18n();
  const labels = useImageLabels();
  return (pulled: ShareCards, name: string) => savePullImage(pulled, labels(name, pulled.cards.length), imageName(pulled.bannerName)).catch(() => toast(t("chattools.economy.gacha.imageFailed")));
}

/**
 * A gacha pull someone shared (Message.gacha): its cards as the server
 * kept them, the rarest one glowing, and a button to save it as a picture.
 * A profile item the server still has shows itself, worn by whoever pulled
 * it (an effect plays while its card is pointed at).
 */
export function GachaShareCard({ message, instanceKey }: { message: Message; instanceKey: string }) {
  const { t } = useI18n();
  const author = useFuwa((s) => s.instances[instanceKey]?.users[message.authorId]);
  const items = useFuwa((s) => s.instances[instanceKey]?.serverProfileItems[message.serverId]);
  const save = useSaveImage();
  const pulled = useMemo(() => (message.gacha ? cardsOfShare(message.gacha) : null), [message.gacha]);
  if (!pulled) return null;
  const best = bestCard(pulled.cards);
  const glow = cssColor(best?.color ?? 0xf59e0b);
  return (
    <div className="mt-1 w-fit max-w-[30rem] overflow-hidden rounded-2xl p-3 text-white shadow-sm" style={{ background: `radial-gradient(120% 90% at 50% 0%, ${glow}55, transparent 70%), #0b0a14` }}>
      <div className="mb-2.5 flex items-center gap-2">
        <DicesIcon className="size-4 shrink-0" style={{ color: glow }} />
        <p className="min-w-0 flex-1 truncate text-sm font-extrabold">{t("chattools.economy.gacha.sharedHeader", { count: pulled.cards.length, banner: pulled.bannerName })}</p>
        <button
          type="button"
          onClick={() => void save(pulled, displayName(author))}
          aria-label={t("chattools.economy.gacha.saveImage")}
          title={t("chattools.economy.gacha.saveImage")}
          className="grid size-7 shrink-0 place-items-center rounded-full bg-white/10 text-white/80 transition hover:bg-white/20 hover:text-white"
        >
          <DownloadIcon className="size-3.5" />
        </button>
      </div>
      <ul className="flex flex-wrap gap-1.5">
        {pulled.cards.map((card, n) => {
          const itemId = message.gacha?.cards[n]?.itemId;
          return <SharedCard key={n} card={card} item={itemId ? items?.find((i) => i.id === itemId) : undefined} author={author} />;
        })}
      </ul>
    </div>
  );
}

/** One of a shared pull's cards: its stars, what it was (the item itself, when it can be drawn) and its tier. */
function SharedCard({ card, item, author }: { card: ShareCard; item: ProfileItem | undefined; author: User | undefined }) {
  const { t } = useI18n();
  const [hover, setHover] = useState(false);
  const color = cssColor(card.color);
  const chip = chipKey(card);
  return (
    <li
      className={cn("flex h-28 w-[5.25rem] flex-col items-center rounded-xl border-2 px-1 pt-1.5 pb-1 text-center", card.rarity >= 3 && "shadow-[0_0_14px_var(--glow)]")}
      style={{ borderColor: color, background: `linear-gradient(180deg, ${color}40, transparent 60%), #15131f`, ["--glow" as string]: `${color}aa` }}
      title={`${card.prizeName} · ${card.tierName}`}
      onPointerEnter={() => setHover(true)}
      onPointerLeave={() => setHover(false)}
    >
      <span className="text-[0.6rem] leading-none tracking-tighter" style={{ color }}>
        {"★".repeat(card.stars)}
      </span>
      {item && author && hasArt(item) ? (
        <span className={cn("grid place-items-center", item.kind === ITEM_DECORATION ? "mt-2 h-7" : "mt-1 h-9")}>
          <ItemArt item={item} me={author} play={hover} />
        </span>
      ) : (
        <span className="mt-1.5 grid size-7 place-items-center rounded-full" style={{ background: `${color}38`, color }}>
          <SparklesIcon className={card.rarity >= 2 ? "size-4" : "size-3.5"} />
        </span>
      )}
      <span className="mt-auto line-clamp-2 w-full text-[0.68rem] leading-tight font-bold break-words">{card.prizeName}</span>
      <span className="w-full truncate text-[0.55rem] font-extrabold tracking-wide uppercase" style={{ color }}>
        {card.tierName}
      </span>
      {chip && <span className="w-full truncate text-[0.55rem] font-bold text-white/60">{t(CHIPS[chip])}</span>}
    </li>
  );
}
