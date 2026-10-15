import { ChevronDownIcon, DicesIcon, LoaderCircleIcon, PercentIcon, RotateCcwIcon, ShieldCheckIcon, SparklesIcon, TicketIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useMemo, useState } from "react";
import type { Balance, Currency, GachaBanner, GachaProgress, GachaPull, GachaResult, ProfileItem, Role, ShopListing } from "@/gen/fuwa/v1/types_pb";
import { economyMessage, pull } from "@/fuwa/economy";
import { useAccess, useInstance, useMyMember } from "@/fuwa/hooks";
import { Amount, CurrencyIcon, listingName } from "@/components/economy/shared";
import { Count } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { T, useI18n } from "@/i18n/react";
import { num } from "@/lib/economy";
import { openness, OUTCOME_WON, outcomeLabel, percent, pityIn, pullCost, pullsLeft, ratesTable, shownBanners, tierColor } from "@/lib/gacha";
import { formatLeft, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { cssColor } from "@/lib/permissions";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

/**
 * The shop's gacha banners (docs/economy.md), for any member: each open (or
 * coming) banner with its price, its drop rates in full, your pity count,
 * and Pull buttons whose results flip in one by one.
 */
export function Banners({
  instanceKey,
  serverId,
  currencies,
  banners,
  progress,
  shop,
  balanceOf,
}: {
  instanceKey: string;
  serverId: string;
  currencies: Currency[];
  banners: GachaBanner[] | null;
  progress: Record<string, GachaProgress>;
  shop: ShopListing[] | null;
  balanceOf: (currencyId: string) => Balance | undefined;
}) {
  const { t } = useI18n();
  const now = useNow(30_000);
  if (!banners)
    return (
      <div className="flex flex-col gap-2">
        {[0, 1].map((n) => (
          <div key={n} className="shimmer h-40 rounded-3xl" />
        ))}
      </div>
    );
  const shown = shownBanners(
    banners,
    currencies.map((c) => c.id),
    now,
  );
  if (!shown.length)
    return (
      <div className="flex flex-col items-center gap-1 py-10 text-center">
        <motion.span animate={{ rotate: [0, 20, -20, 0], y: [0, -6, 0] }} transition={{ duration: 2.4, repeat: Infinity, repeatDelay: 1 }} className="text-4xl">
          🎰
        </motion.span>
        <p className="font-bold">{t("chattools.economy.gacha.empty")}</p>
        <p className="text-sm text-muted-foreground">{t("chattools.economy.gacha.emptyHint")}</p>
      </div>
    );
  return (
    <ul className="flex flex-col gap-3">
      {shown.map((b, n) => (
        <BannerCard
          key={b.id}
          instanceKey={instanceKey}
          serverId={serverId}
          banner={b}
          index={n}
          currency={currencies.find((c) => c.id === b.currencyId)}
          currencies={currencies}
          balance={num(balanceOf(b.currencyId)?.amount)}
          progress={progress[b.id]}
          shop={shop ?? []}
        />
      ))}
    </ul>
  );
}

/** What a prize is called: its listing's name, or the currency it pays. */
function usePrizeName(instanceKey: string, serverId: string, shop: readonly ShopListing[]) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const roles = useMemo(() => inst?.roles[serverId] ?? [], [inst?.roles, serverId]);
  const items = inst?.serverProfileItems[serverId];
  return {
    roles,
    items,
    name: (listingId: string) => listingName(shop.find((l) => l.id === listingId), roles, items, t("chattools.economy.somethingFromShop")),
  };
}

function BannerCard({
  instanceKey,
  serverId,
  banner,
  index,
  currency,
  currencies,
  balance,
  progress,
  shop,
}: {
  instanceKey: string;
  serverId: string;
  banner: GachaBanner;
  index: number;
  currency: Currency | undefined;
  currencies: Currency[];
  balance: number;
  progress: GachaProgress | undefined;
  shop: ShopListing[];
}) {
  const lang = useI18n();
  const { t } = lang;
  const now = useNow(15_000);
  const member = useMyMember(instanceKey, serverId);
  const access = useAccess(instanceKey, serverId);
  const [rates, setRates] = useState(false);
  const [busy, setBusy] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<GachaPull | null>(null);
  const open = openness(banner, now);
  const pity = pityIn(banner, progress);
  const left = pullsLeft(banner, progress);
  const pityTier = banner.tiers.find((t) => t.id === banner.pityTierId);
  const pityIndex = banner.tiers.findIndex((t) => t.id === banner.pityTierId);
  const lead = banner.tiers.find((t) => t.rate > 0);
  const glow = cssColor(tierColor(lead ?? {}, 0));
  const held = access.pending ? t("chattools.economy.agreeFirst") : timedOutUntil(member, now) ? t("chattools.economy.timedOut") : null;
  const counts = [1, ...(banner.multiPull >= 2 ? [banner.multiPull] : [])];
  const art = shownPicture(banner.artUrl);

  async function go(count: number) {
    const price = pullCost(banner, count);
    if (price === null) return;
    setBusy(count);
    setError(null);
    try {
      const res = await pull(instanceKey, serverId, banner, count, price);
      if (res.pull) setResult(res.pull);
    } catch (err) {
      setError(economyMessage(err));
    } finally {
      setBusy(0);
    }
  }

  return (
    <motion.li
      initial={{ opacity: 0, y: 12, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { ...SPRING, delay: Math.min(index, 6) * 0.05 } }}
      className="relative overflow-hidden rounded-3xl border bg-background/50"
    >
      <div aria-hidden className="pointer-events-none absolute inset-0 opacity-25" style={{ background: `radial-gradient(120% 90% at 100% 0%, ${glow}, transparent 60%)` }} />
      <div className="relative flex flex-col gap-3 p-4">
        <div className="flex items-start gap-3">
          {art ? (
            // The box or card pack, rocking now and then as if it wants opening.
            <motion.img
              src={art}
              alt=""
              draggable={false}
              animate={{ rotate: [0, -6, 6, -3, 0], y: [0, -3, 0] }}
              transition={{ duration: 1.4, repeat: Infinity, repeatDelay: 3 }}
              className="h-24 w-20 shrink-0 rounded-2xl object-contain drop-shadow-lg"
              style={{ filter: `drop-shadow(0 6px 16px ${glow}66)` }}
            />
          ) : (
            <motion.span
              animate={{ rotate: [0, -12, 12, 0] }}
              transition={{ duration: 1.8, repeat: Infinity, repeatDelay: 2.5 }}
              className="grid size-12 shrink-0 place-items-center rounded-2xl text-white shadow-lg"
              style={{ backgroundColor: glow }}
            >
              <SparklesIcon className="size-6" />
            </motion.span>
          )}
          <div className="min-w-0 flex-1">
            <p className="truncate text-lg font-extrabold">{banner.name}</p>
            <p className="text-xs font-bold text-muted-foreground">
              {open.state === "soon"
                ? t("chattools.economy.gacha.opensIn", { time: formatLeft(lang, open.at - now) })
                : open.at
                  ? t("chattools.economy.gacha.closesIn", { time: formatLeft(lang, open.at - now) })
                  : t("chattools.economy.gacha.always")}
            </p>
            {banner.description && <p className="mt-1 text-sm whitespace-pre-line text-muted-foreground">{banner.description}</p>}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs font-bold text-muted-foreground">
          {pity !== null && pityTier && (
            <span className="flex items-center gap-1" title={t("chattools.economy.gacha.pityAbout", { tier: pityTier.name, count: banner.pityPulls })}>
              <ShieldCheckIcon className="size-3.5" style={{ color: cssColor(tierColor(pityTier, pityIndex)) }} />
              <T k="chattools.economy.gacha.pityIn" values={{ count: <Count value={pity} />, tier: <b style={{ color: cssColor(tierColor(pityTier, pityIndex)) }}>{pityTier.name}</b> }} count={pity} />
            </span>
          )}
          {left !== null && (
            <span className="flex items-center gap-1">
              <TicketIcon className="size-3.5" />
              <T k="chattools.economy.gacha.pullsLeft" values={{ count: <Count value={left} /> }} count={left} />
            </span>
          )}
          {!!progress?.pulls && (
            <span className="flex items-center gap-1">
              <DicesIcon className="size-3.5" />
              <T k="chattools.economy.gacha.pulled" values={{ count: <Count value={progress.pulls} /> }} count={progress.pulls} />
            </span>
          )}
        </div>
        {pity !== null && banner.pityPulls > 0 && (
          <div className="h-1.5 overflow-hidden rounded-full bg-muted" role="progressbar" aria-valuemin={0} aria-valuemax={banner.pityPulls} aria-valuenow={banner.pityPulls - pity}>
            <motion.div
              className="h-full rounded-full"
              style={{ backgroundColor: pityTier ? cssColor(tierColor(pityTier, pityIndex)) : undefined }}
              initial={false}
              animate={{ width: `${((banner.pityPulls - pity) / banner.pityPulls) * 100}%` }}
              transition={SPRING}
            />
          </div>
        )}

        <div className="flex flex-wrap items-center gap-2">
          {counts.map((count) => {
            const price = pullCost(banner, count) ?? 0;
            const why =
              open.state !== "open"
                ? t("chattools.economy.gacha.notOpen")
                : held ?? (left !== null && left < count ? t("chattools.economy.gacha.noPulls") : balance < price ? t("chattools.economy.notEnough", { currency: currency?.name ?? "" }) : null);
            return (
              <Button
                key={count}
                type="button"
                variant={count === 1 ? "outline" : "default"}
                className="btn group h-auto flex-col gap-0 rounded-2xl px-4 py-1.5 font-extrabold"
                disabled={!!why || !!busy}
                title={why ?? undefined}
                onClick={() => void go(count)}
              >
                <span className="flex items-center gap-1.5">
                  {busy === count ? <LoaderCircleIcon className="animate-spin" /> : <DicesIcon className="transition-transform group-hover:rotate-45" />}
                  {count === 1 ? t("chattools.economy.gacha.pullOnce") : t("chattools.economy.gacha.pullMany", { count })}
                </span>
                <Amount instanceKey={instanceKey} currency={currency} value={price} className="text-xs font-bold opacity-80" iconClassName="size-3" />
              </Button>
            );
          })}
          <Button type="button" variant="ghost" size="sm" className="ml-auto rounded-xl" aria-expanded={rates} onClick={() => setRates((r) => !r)}>
            <PercentIcon /> {t("chattools.economy.gacha.rates")}
            <ChevronDownIcon className={cn("transition-transform", rates && "rotate-180")} />
          </Button>
        </div>
        <AnimatePresence mode="popLayout" initial={false}>
          {error && (
            <motion.p key="error" {...SLIDE_IN} transition={SPRING} role="alert" className="text-xs text-destructive first-letter:uppercase">
              {error}
            </motion.p>
          )}
          {rates && (
            <motion.div key="rates" {...SLIDE_IN} transition={SPRING}>
              <Rates instanceKey={instanceKey} serverId={serverId} banner={banner} currencies={currencies} shop={shop} />
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      <Reveal
        instanceKey={instanceKey}
        serverId={serverId}
        banner={banner}
        result={result}
        currencies={currencies}
        shop={shop}
        onClose={() => setResult(null)}
        again={!!result && !busy ? () => void go(result.results.length) : undefined}
      />
    </motion.li>
  );
}

/** Every tier with its rate, and every prize in it with its own chance. */
function Rates({ instanceKey, serverId, banner, currencies, shop }: { instanceKey: string; serverId: string; banner: GachaBanner; currencies: Currency[]; shop: ShopListing[] }) {
  const { t } = useI18n();
  const { name } = usePrizeName(instanceKey, serverId, shop);
  return (
    <div className="flex flex-col gap-2 rounded-2xl bg-muted/40 p-3">
      {ratesTable(banner).map(({ tier, prizes }, i) => (
        <div key={tier.id} className="flex flex-col gap-1">
          <p className="flex items-center gap-2 text-sm font-extrabold">
            <span className="size-2.5 rounded-full" style={{ backgroundColor: cssColor(tierColor(tier, i)) }} />
            <span style={{ color: cssColor(tierColor(tier, i)) }}>{tier.name}</span>
            <span className="ml-auto tabular-nums">{percent(tier.rate)}</span>
          </p>
          <ul className="flex flex-col pl-4.5">
            {prizes.map((p) => {
              const cur = currencies.find((c) => c.id === p.currencyId);
              return (
                <li key={p.id} className="flex items-center gap-2 text-xs text-muted-foreground">
                  <span className="min-w-0 flex-1 truncate">
                    {p.listingId ? name(p.listingId) : <Amount instanceKey={instanceKey} currency={cur} value={num(p.amount)} className="font-normal" iconClassName="size-3" />}
                  </span>
                  <span className="tabular-nums">{percent(p.chance)}</span>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
      <p className="text-[0.7rem] text-muted-foreground">{t("chattools.economy.gacha.ratesNote")}</p>
    </div>
  );
}

/** What dropped, flipped in one by one, rarest glowing. */
function Reveal({
  instanceKey,
  serverId,
  banner,
  result,
  currencies,
  shop,
  onClose,
  again,
}: {
  instanceKey: string;
  serverId: string;
  banner: GachaBanner;
  result: GachaPull | null;
  currencies: Currency[];
  shop: ShopListing[];
  onClose: () => void;
  again?: () => void;
}) {
  const { t } = useI18n();
  const { name, roles, items } = usePrizeName(instanceKey, serverId, shop);
  const [kept, setKept] = useState<GachaPull | null>(result);
  if (result && result !== kept) setKept(result);
  const shown = result ?? kept;
  const art = shownPicture(banner.artUrl);
  return (
    <Dialog open={!!result} onOpenChange={(o) => !o && onClose()}>
      <DialogContent wide>
        {shown && (
          <div className="flex flex-col gap-4">
            <DialogHeader title={t("chattools.economy.gacha.revealTitle", { banner: banner.name })} description={t("chattools.economy.gacha.revealAbout", { count: shown.results.length })} />
            <div className="relative">
              {art && (
                // The pack shakes and bursts over the cards, which then flip in.
                <motion.img
                  key={shown.id}
                  src={art}
                  alt=""
                  aria-hidden
                  draggable={false}
                  initial={{ scale: 0.9, rotate: 0, opacity: 1 }}
                  animate={{ rotate: [0, -10, 10, -8, 8, 0], scale: [0.9, 1, 1, 1.15, 1.4, 1.8], opacity: [1, 1, 1, 1, 0.7, 0] }}
                  transition={{ duration: 0.9, ease: "easeIn" }}
                  className="pointer-events-none absolute inset-x-0 top-1/2 z-10 mx-auto h-36 max-h-full -translate-y-1/2 object-contain"
                />
              )}
              <ul className={cn("grid gap-2", shown.results.length > 1 ? "grid-cols-2 sm:grid-cols-5" : "mx-auto w-48 grid-cols-1")}>
                {shown.results.map((r, n) => (
                  <Drop key={`${shown.id}-${n}`} instanceKey={instanceKey} result={r} index={n} after={art ? 0.75 : 0} banner={banner} currencies={currencies} name={r.listingId ? r.prizeName || name(r.listingId) : r.prizeName} roles={roles} items={items} />
                ))}
              </ul>
            </div>
            <div className="flex justify-end gap-2">
              {again && (
                <Button type="button" variant="outline" className="rounded-xl" onClick={again}>
                  <RotateCcwIcon /> {t("chattools.economy.gacha.again")}
                </Button>
              )}
              <Button type="button" className="rounded-xl font-bold" onClick={onClose}>
                {t("chattools.economy.gacha.done")}
              </Button>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}

function Drop({
  instanceKey,
  result,
  index,
  after,
  banner,
  currencies,
  name,
}: {
  instanceKey: string;
  result: GachaResult;
  index: number;
  /** Seconds to wait first, while the pack opens. */
  after: number;
  banner: GachaBanner;
  currencies: Currency[];
  name: string;
  roles: Role[];
  items: ProfileItem[] | undefined;
}) {
  const { t } = useI18n();
  const tierIndex = banner.tiers.findIndex((tier) => tier.id === result.tierId);
  const tier = banner.tiers[tierIndex];
  const color = cssColor(tierColor(tier ?? {}, tierIndex === -1 ? banner.tiers.length : tierIndex));
  // The rarest tier (or the pity tier and rarer) gets the big entrance.
  const rare = tierIndex === 0 || (banner.pityTierId && tierIndex <= banner.tiers.findIndex((x) => x.id === banner.pityTierId));
  const currency = currencies.find((c) => c.id === result.currencyId);
  return (
    <motion.li
      initial={{ opacity: 0, rotateY: 90, scale: 0.8 }}
      animate={{ opacity: 1, rotateY: 0, scale: rare ? [0.8, 1.12, 1] : 1 }}
      transition={{ ...SPRING, delay: after + 0.15 + index * 0.12 }}
      className="relative flex flex-col items-center gap-1 overflow-hidden rounded-2xl border-2 p-3 text-center"
      style={{ borderColor: color, boxShadow: rare ? `0 0 24px ${color}66` : undefined }}
    >
      <div aria-hidden className="pointer-events-none absolute inset-0 opacity-15" style={{ background: `linear-gradient(180deg, ${color}, transparent)` }} />
      <span className="relative text-[0.65rem] font-extrabold tracking-wide uppercase" style={{ color }}>
        {result.tierName}
      </span>
      <span className="relative line-clamp-2 text-sm font-extrabold break-words">{name}</span>
      {result.outcome !== OUTCOME_WON ? (
        <span className="relative text-[0.7rem] font-bold text-muted-foreground">
          {t(outcomeLabel(result.outcome))}
          {result.amount > 0n && (
            <>
              {" "}
              <Amount instanceKey={instanceKey} currency={currency} value={num(result.amount)} className="font-bold" iconClassName="size-3" />
            </>
          )}
        </span>
      ) : (
        result.guaranteed && <span className="relative text-[0.7rem] font-bold text-muted-foreground">{t("chattools.economy.gacha.guaranteed")}</span>
      )}
      {!result.listingId && currency && <CurrencyIcon instanceKey={instanceKey} currency={currency} className="relative size-6 text-xl" />}
    </motion.li>
  );
}
