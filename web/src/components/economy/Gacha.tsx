import * as DialogPrimitive from "@radix-ui/react-dialog";
import {
  CheckIcon,
  ChevronDownIcon,
  ClockIcon,
  DicesIcon,
  DownloadIcon,
  HashIcon,
  LoaderCircleIcon,
  PackageIcon,
  PercentIcon,
  RotateCcwIcon,
  ShieldCheckIcon,
  Share2Icon,
  ShieldIcon,
  SparklesIcon,
  TicketIcon,
  Volume2Icon,
  VolumeXIcon,
} from "lucide-react";
import { AnimatePresence, m as motion, useReducedMotion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type CSSProperties, type ReactNode } from "react";
import type { Balance, Currency, GachaBanner, GachaProgress, GachaPull, GachaResult, Role, ShopListing } from "@/gen/fuwa/v1/types_pb";
import { economyMessage, pull, shareHere, sharePull } from "@/fuwa/economy";
import { useAccess, useInstance, useMyMember } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { useSaveImage } from "@/components/chat/GachaShareCard";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { ChannelType, Permission as P } from "@/gen/fuwa/v1/types_pb";
import { displayName } from "@/lib/format";
import { cardsOfPull } from "@/lib/gacha-share";
import { hasIn } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { Amount, CurrencyIcon, listingName } from "@/components/economy/shared";
import { Count } from "@/components/motion";
import { T, useI18n } from "@/i18n/react";
import { LISTING_PROFILE_ITEM, LISTING_ROLE, num } from "@/lib/economy";
import { bestTier, openness, OUTCOME_WON, outcomeLabel, percent, pityIn, pullCost, pullsLeft, rarity, ratesTable, revealPlan, REVEAL, shownBanners, stars, tierColor } from "@/lib/gacha";
import { formatLeft, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { cssColor } from "@/lib/permissions";
import { setPrefs, usePrefs } from "@/lib/prefs";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { shownPicture } from "@/lib/shown";
import { gachaCue, RARITY_CUES } from "@/lib/sounds";
import { cn } from "@/lib/utils";

/*
 * The shop's gacha (docs/economy.md), for any member. One banner at a time
 * on a stage: its pack floating over slow light rays, the top prizes, a pity
 * meter and two big Pull buttons, with the drop rates in full a click away.
 * A pull plays out over the whole window: the pack charges in the color of
 * the best thing inside, bursts, deals the cards face down and turns them
 * one by one, waiting a beat before a rare one, with sounds to match
 * (`gachaCue`). The desktop's `ui/shop.rs` draws the same, to the same clock
 * (`revealPlan`).
 */

/** The color a tier shows in, by its place among the banner's tiers. */
const tierCss = (banner: GachaBanner, index: number) => cssColor(tierColor(banner.tiers[index] ?? {}, index === -1 ? banner.tiers.length : index));

/** The banner's own color: its rarest tier that can drop. */
function glowOf(banner: GachaBanner) {
  const lead = Math.max(
    0,
    banner.tiers.findIndex((t) => t.rate > 0),
  );
  return tierCss(banner, lead);
}

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
  const [picked, setPicked] = useState<string | null>(null);
  const from = useRef(0);
  if (!banners)
    return (
      <div className="flex flex-col gap-3">
        <div className="shimmer h-56 rounded-3xl" />
        <div className="shimmer h-16 rounded-2xl" />
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
  const at = Math.max(
    0,
    shown.findIndex((b) => b.id === picked),
  );
  const banner = shown[at]!;
  const step = Math.sign(at - from.current);
  from.current = at;
  return (
    <div className="flex flex-col gap-3">
      {shown.length > 1 && (
        <div role="tablist" aria-label={t("chattools.economy.gacha.banners")} className="-mx-1 flex gap-1 overflow-x-auto px-1 pb-1">
          {shown.map((b) => {
            const on = b.id === banner.id;
            const art = shownPicture(b.artUrl);
            return (
              <button
                key={b.id}
                type="button"
                role="tab"
                aria-selected={on}
                onClick={() => {
                  if (on) return;
                  gachaCue("tap");
                  setPicked(b.id);
                }}
                className={cn(
                  "relative flex h-11 w-44 shrink-0 items-center gap-2 rounded-2xl px-2.5 text-sm font-extrabold transition-colors active:scale-[0.97]",
                  on ? "text-foreground" : "text-muted-foreground hover:bg-muted/70",
                )}
              >
                {on && <motion.span layoutId={`gacha-pick-${serverId}`} transition={SPRING} className="absolute inset-0 rounded-2xl border bg-background shadow-sm" />}
                <span className="relative grid size-7 place-items-center">
                  {art ? <img src={art} alt="" draggable={false} className="size-7 object-contain" /> : <Pack glow={glowOf(b)} className="h-7 w-5.5 rounded-md border" />}
                </span>
                <span className="relative min-w-0 flex-1 truncate text-left">{b.name}</span>
              </button>
            );
          })}
        </div>
      )}
      <AnimatePresence mode="popLayout" initial={false} custom={step}>
        <motion.div
          key={banner.id}
          custom={step}
          variants={{ enter: (s: number) => ({ opacity: 0, x: s * 40 }), rest: { opacity: 1, x: 0 }, leave: (s: number) => ({ opacity: 0, x: s * -40 }) }}
          initial="enter"
          animate="rest"
          exit="leave"
          transition={SPRING}
        >
          <Stage
            instanceKey={instanceKey}
            serverId={serverId}
            banner={banner}
            currency={currencies.find((c) => c.id === banner.currencyId)}
            currencies={currencies}
            balance={num(balanceOf(banner.currencyId)?.amount)}
            progress={progress[banner.id]}
            shop={shop ?? []}
          />
        </motion.div>
      </AnimatePresence>
    </div>
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
    name: (listingId: string) =>
      listingName(
        shop.find((l) => l.id === listingId),
        roles,
        items,
        t("chattools.economy.somethingFromShop"),
      ),
  };
}

/** The box or card pack: the server's picture, or a pack drawn in the banner's color. */
function Pack({ art, glow, className, style }: { art?: string | null; glow: string; className?: string; style?: CSSProperties }) {
  if (art) return <img src={art} alt="" aria-hidden draggable={false} className={cn("object-contain", className)} style={{ filter: `drop-shadow(0 10px 24px ${glow}88)`, ...style }} />;
  return (
    <span
      aria-hidden
      className={cn("relative grid place-items-center overflow-hidden rounded-2xl border-2 border-white/40 text-white shadow-xl", className)}
      style={{ background: `linear-gradient(160deg, ${glow}, ${glow}aa 55%, #1e1b2e)`, boxShadow: `0 10px 28px ${glow}66`, ...style }}
    >
      <span className="absolute inset-x-0 top-[13%] h-[7%] bg-white/30" />
      <span className="absolute inset-x-0 bottom-[13%] h-[3%] bg-white/20" />
      <SparklesIcon className="size-2/5" />
    </span>
  );
}

/** Light rays turning slowly behind something: `count` bars from the middle out, fading as they go. */
function Rays({ color, className, count = 12, period = 40, opacity = 0.35 }: { color: string; className?: string; count?: number; period?: number; opacity?: number }) {
  return (
    <motion.span
      aria-hidden
      className={cn("pointer-events-none absolute aspect-square", className)}
      style={{ opacity }}
      animate={{ rotate: 360 }}
      transition={{ duration: period, repeat: Infinity, ease: "linear" }}
    >
      {Array.from({ length: count }, (_, n) => (
        <span
          key={n}
          className="absolute top-1/2 left-1/2 h-1/2 w-[9%] origin-top -translate-x-1/2"
          style={{ rotate: `${(n * 360) / count}deg`, background: `linear-gradient(to bottom, ${color}, transparent)` }}
        />
      ))}
    </motion.span>
  );
}

function SoundSwitch({ className }: { className?: string }) {
  const { t } = useI18n();
  const on = usePrefs((p) => p.gachaSounds);
  const label = on ? t("chattools.economy.gacha.soundsOn") : t("chattools.economy.gacha.soundsOff");
  return (
    <button
      type="button"
      aria-pressed={on}
      aria-label={label}
      title={label}
      onClick={() => {
        setPrefs({ gachaSounds: !on });
        if (!on) setTimeout(() => gachaCue("tap"));
      }}
      className={cn("grid size-8 place-items-center rounded-full bg-black/25 text-white backdrop-blur transition hover:bg-black/40 active:scale-90", className)}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span key={String(on)} initial={{ scale: 0.4, opacity: 0 }} animate={{ scale: 1, opacity: 1 }} exit={{ scale: 0.4, opacity: 0 }} transition={SPRING}>
          {on ? <Volume2Icon className="size-4" /> : <VolumeXIcon className="size-4" />}
        </motion.span>
      </AnimatePresence>
    </button>
  );
}

function Stage({
  instanceKey,
  serverId,
  banner,
  currency,
  currencies,
  balance,
  progress,
  shop,
}: {
  instanceKey: string;
  serverId: string;
  banner: GachaBanner;
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
  const { name } = usePrizeName(instanceKey, serverId, shop);
  const [rates, setRates] = useState(false);
  const [busy, setBusy] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<GachaPull | null>(null);
  const open = openness(banner, now);
  const pity = pityIn(banner, progress);
  const left = pullsLeft(banner, progress);
  const pityIndex = banner.tiers.findIndex((t) => t.id === banner.pityTierId);
  const pityTier = banner.tiers[pityIndex];
  const glow = glowOf(banner);
  const lead = Math.max(
    0,
    banner.tiers.findIndex((t) => t.rate > 0),
  );
  const held = access.pending ? t("chattools.economy.agreeFirst") : timedOutUntil(member, now) ? t("chattools.economy.timedOut") : null;
  const counts = [1, ...(banner.multiPull >= 2 ? [banner.multiPull] : [])];
  const art = shownPicture(banner.artUrl);
  const top = ratesTable(banner)[lead];

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

  const why = (count: number) => {
    const price = pullCost(banner, count) ?? 0;
    if (open.state !== "open") return t("chattools.economy.gacha.notOpen");
    return held ?? (left !== null && left < count ? t("chattools.economy.gacha.noPulls") : balance < price ? t("chattools.economy.notEnough", { currency: currency?.name ?? "" }) : null);
  };
  const blocked = counts.map(why).find((w) => w);

  return (
    <div className="flex flex-col gap-3">
      {/* The stage: the pack over its rays, the banner's name and when it closes. */}
      <div className="relative isolate min-h-56 overflow-hidden rounded-3xl border" style={{ background: `linear-gradient(120deg, ${glow}38, transparent 65%), var(--card)` }}>
        <span aria-hidden className="absolute -right-16 top-1/2 size-96 -translate-y-1/2 rounded-full blur-3xl" style={{ background: `${glow}40` }} />
        <Rays color={glow} className="top-1/2 right-[-6rem] w-[26rem] -translate-y-1/2" />
        <motion.div
          className="absolute top-1/2 right-6 -translate-y-1/2 sm:right-10"
          animate={{ y: [0, -8, 0], rotate: [-2, 2, -2] }}
          transition={{ duration: 3.2, repeat: Infinity, ease: "easeInOut" }}
          whileHover={{ scale: 1.06, rotate: -4 }}
        >
          <Pack art={art} glow={glow} className={art ? "h-44 w-36" : "h-36 w-28"} />
        </motion.div>
        <SoundSwitch className="absolute top-3 right-3 z-10" />
        <div className="relative flex max-w-[60%] flex-col items-start gap-2 p-5">
          {top && (
            <span className="flex items-center gap-1 rounded-full px-2 py-0.5 text-[0.7rem] font-extrabold tracking-wide text-white uppercase shadow-sm" style={{ backgroundColor: glow }}>
              ★ {top.tier.name} · {percent(top.tier.rate)}
            </span>
          )}
          <h3 className="text-2xl leading-tight font-extrabold tracking-tight break-words sm:text-3xl">{banner.name}</h3>
          <span className="flex items-center gap-1 rounded-full bg-background/60 px-2 py-0.5 text-xs font-bold text-muted-foreground backdrop-blur">
            <ClockIcon className="size-3" />
            {open.state === "soon"
              ? t("chattools.economy.gacha.opensIn", { time: formatLeft(lang, open.at - now) })
              : open.at
                ? t("chattools.economy.gacha.closesIn", { time: formatLeft(lang, open.at - now) })
                : t("chattools.economy.gacha.always")}
          </span>
          {banner.description && <p className="line-clamp-3 text-sm whitespace-pre-line text-muted-foreground">{banner.description}</p>}
        </div>
      </div>

      {/* What the rarest tier holds. */}
      {top && !!top.prizes.length && (
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="mr-1 text-xs font-extrabold text-muted-foreground">{t("chattools.economy.gacha.topPrizes")}</span>
          {top.prizes.slice(0, 4).map((p, n) => {
            const cur = currencies.find((c) => c.id === p.currencyId);
            return (
              <motion.span
                key={p.id}
                initial={{ opacity: 0, scale: 0.8 }}
                animate={{ opacity: 1, scale: 1, transition: { ...SPRING, delay: 0.08 + n * 0.05 } }}
                className="flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-xs font-bold"
                style={{ borderColor: `${glow}66`, backgroundColor: `${glow}14` }}
              >
                <SparklesIcon className="size-3" style={{ color: glow }} />
                <span className="max-w-40 truncate">
                  {p.listingId ? name(p.listingId) : <Amount instanceKey={instanceKey} currency={cur} value={num(p.amount)} className="font-bold" iconClassName="size-3" />}
                </span>
                <span className="text-muted-foreground tabular-nums">{percent(p.chance)}</span>
              </motion.span>
            );
          })}
        </div>
      )}

      {/* Pity, as a meter filling toward the guarantee. */}
      {pity !== null && pityTier && banner.pityPulls > 0 && (
        <div className="flex flex-col gap-2 rounded-2xl border bg-background/50 p-3" title={t("chattools.economy.gacha.pityAbout", { tier: pityTier.name, count: banner.pityPulls })}>
          <div className="flex items-center gap-2 text-xs font-bold text-muted-foreground">
            <ShieldCheckIcon className="size-4" style={{ color: tierCss(banner, pityIndex) }} />
            <span className="min-w-0 flex-1">
              <T k="chattools.economy.gacha.pityIn" values={{ count: <Count value={pity} />, tier: <b style={{ color: tierCss(banner, pityIndex) }}>{pityTier.name}</b> }} count={pity} />
            </span>
            <span className="tabular-nums">{t("chattools.economy.gacha.pityMeter", { done: banner.pityPulls - pity, total: banner.pityPulls })}</span>
          </div>
          <div className="relative h-2 overflow-hidden rounded-full bg-muted" role="progressbar" aria-valuemin={0} aria-valuemax={banner.pityPulls} aria-valuenow={banner.pityPulls - pity}>
            <motion.div
              className="relative h-full overflow-hidden rounded-full"
              style={{ backgroundColor: tierCss(banner, pityIndex) }}
              initial={false}
              animate={{ width: `${Math.max(3, ((banner.pityPulls - pity) / banner.pityPulls) * 100)}%` }}
              transition={SPRING}
            >
              <motion.span
                className="absolute inset-y-0 w-10 bg-gradient-to-r from-transparent via-white/60 to-transparent"
                animate={{ left: ["-40px", "110%"] }}
                transition={{ duration: 1.8, repeat: Infinity, repeatDelay: 1.2, ease: "easeInOut" }}
              />
            </motion.div>
          </div>
        </div>
      )}

      {/* Pull once, and the multi-pull. */}
      <div className={cn("grid gap-2", counts.length > 1 && "grid-cols-2")}>
        {counts.map((count) => {
          const price = pullCost(banner, count) ?? 0;
          const no = why(count);
          const multi = count > 1;
          return (
            <motion.button
              key={count}
              type="button"
              disabled={!!no || !!busy}
              title={no ?? undefined}
              onClick={() => void go(count)}
              whileHover={no || busy ? undefined : { y: -2 }}
              whileTap={no || busy ? undefined : { scale: 0.97 }}
              transition={SPRING}
              className={cn(
                "group relative flex h-16 flex-col items-center justify-center overflow-hidden rounded-2xl border font-extrabold shadow-sm transition-opacity disabled:cursor-not-allowed disabled:opacity-50",
                multi ? "border-transparent text-white" : "bg-background hover:bg-muted/60",
              )}
              style={multi ? { background: `linear-gradient(135deg, ${glow}, var(--primary))`, boxShadow: `0 8px 24px ${glow}55` } : undefined}
            >
              {multi && !no && (
                <motion.span
                  aria-hidden
                  className="absolute inset-y-0 w-16 -skew-x-12 bg-white/25"
                  animate={{ left: ["-30%", "130%"] }}
                  transition={{ duration: 1.6, repeat: Infinity, repeatDelay: 2.4, ease: "easeInOut" }}
                />
              )}
              <span className="relative flex items-center gap-1.5 text-base">
                {busy === count ? <LoaderCircleIcon className="size-4 animate-spin" /> : <DicesIcon className="size-4 transition-transform group-hover:rotate-45" />}
                {multi ? t("chattools.economy.gacha.pullMany", { count }) : t("chattools.economy.gacha.pullOnce")}
              </span>
              <Amount instanceKey={instanceKey} currency={currency} value={price} className="relative text-xs font-bold opacity-85" iconClassName="size-3" />
            </motion.button>
          );
        })}
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs font-bold text-muted-foreground">
        <span className="flex items-center gap-1">
          <T
            k="chattools.economy.gacha.youHave"
            values={{ amount: <Amount instanceKey={instanceKey} currency={currency} value={balance} className="font-extrabold text-foreground" iconClassName="size-3" /> }}
          />
        </span>
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
        <button
          type="button"
          className="ml-auto flex items-center gap-1 rounded-lg px-2 py-1 transition hover:bg-muted hover:text-foreground"
          aria-expanded={rates}
          onClick={() => setRates((r) => !r)}
        >
          <PercentIcon className="size-3.5" /> {t("chattools.economy.gacha.rates")}
          <ChevronDownIcon className={cn("size-3.5 transition-transform", rates && "rotate-180")} />
        </button>
      </div>
      <AnimatePresence mode="popLayout" initial={false}>
        {(error || (blocked && !busy)) && (
          <motion.p
            key={error ?? blocked}
            {...SLIDE_IN}
            transition={SPRING}
            role={error ? "alert" : undefined}
            className={cn("text-xs first-letter:uppercase", error ? "text-destructive" : "text-muted-foreground")}
          >
            {error ?? blocked}
          </motion.p>
        )}
        {rates && (
          <motion.div key="rates" {...SLIDE_IN} transition={SPRING}>
            <Rates instanceKey={instanceKey} serverId={serverId} banner={banner} currencies={currencies} shop={shop} />
          </motion.div>
        )}
      </AnimatePresence>
      <PullShow
        instanceKey={instanceKey}
        serverId={serverId}
        banner={banner}
        result={result}
        currencies={currencies}
        currency={currency}
        shop={shop}
        busy={busy}
        onClose={() => setResult(null)}
        again={(count) => (why(count) ? null : () => void go(count))}
      />
    </div>
  );
}

/** Every tier with its rate (a bar of them all first), and every prize in it with its own chance. */
function Rates({ instanceKey, serverId, banner, currencies, shop }: { instanceKey: string; serverId: string; banner: GachaBanner; currencies: Currency[]; shop: ShopListing[] }) {
  const { t } = useI18n();
  const { name } = usePrizeName(instanceKey, serverId, shop);
  const table = ratesTable(banner);
  return (
    <div className="flex flex-col gap-3 rounded-2xl border bg-background/50 p-3">
      <div className="flex h-2.5 gap-0.5 overflow-hidden rounded-full" aria-hidden>
        {table.map(({ tier }, i) => (
          <motion.span
            key={tier.id}
            className="h-full min-w-1 rounded-full"
            style={{ backgroundColor: tierCss(banner, i) }}
            initial={{ flexGrow: 0 }}
            animate={{ flexGrow: tier.rate }}
            transition={{ ...SPRING, delay: i * 0.04 }}
          />
        ))}
      </div>
      {table.map(({ tier, prizes }, i) => (
        <div key={tier.id} className="flex flex-col gap-1">
          <p className="flex items-center gap-2 text-sm font-extrabold">
            <span className="size-2.5 rounded-full" style={{ backgroundColor: tierCss(banner, i) }} />
            <span style={{ color: tierCss(banner, i) }}>{tier.name}</span>
            <span className="text-[0.625rem] leading-none" style={{ color: tierCss(banner, i) }} aria-label={t("chattools.economy.gacha.stars", { count: stars(banner, tier.id) })}>
              {"★".repeat(stars(banner, tier.id))}
            </span>
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

/** A pull playing out over the whole window, then what dropped, with Pull again and Done. */
function PullShow({
  result,
  onClose,
  ...rest
}: {
  instanceKey: string;
  serverId: string;
  banner: GachaBanner;
  result: GachaPull | null;
  currencies: Currency[];
  currency: Currency | undefined;
  shop: ShopListing[];
  busy: number;
  onClose: () => void;
  again: (count: number) => (() => void) | null;
}) {
  const [kept, setKept] = useState<GachaPull | null>(result);
  if (result && result !== kept) setKept(result);
  const shown = result ?? kept;
  return (
    <DialogPrimitive.Root open={!!result} onOpenChange={(o) => !o && onClose()}>
      <AnimatePresence>
        {result && shown && (
          <DialogPrimitive.Portal forceMount>
            <DialogPrimitive.Content asChild forceMount onEscapeKeyDown={(e) => skipping.current?.() && e.preventDefault()}>
              <motion.div
                className="fixed inset-0 z-[60] overflow-y-auto text-white outline-none"
                initial={{ opacity: 0 }}
                animate={{ opacity: 1 }}
                exit={{ opacity: 0 }}
                transition={{ duration: 0.2 }}
              >
                <Show key={shown.id} pull={shown} onClose={onClose} {...rest} />
              </motion.div>
            </DialogPrimitive.Content>
          </DialogPrimitive.Portal>
        )}
      </AnimatePresence>
    </DialogPrimitive.Root>
  );
}

/** Escape skips to the end while cards are still turning (true), and closes after. */
const skipping: { current: (() => boolean) | null } = { current: null };

function Show({
  instanceKey,
  serverId,
  banner,
  pull: shown,
  currencies,
  currency,
  shop,
  busy,
  onClose,
  again,
}: {
  instanceKey: string;
  serverId: string;
  banner: GachaBanner;
  pull: GachaPull;
  currencies: Currency[];
  currency: Currency | undefined;
  shop: ShopListing[];
  busy: number;
  onClose: () => void;
  again: (count: number) => (() => void) | null;
}) {
  const { t } = useI18n();
  const reduced = !!useReducedMotion();
  const { name, roles } = usePrizeName(instanceKey, serverId, shop);
  const art = shownPicture(banner.artUrl);
  const ranks = useMemo(() => shown.results.map((r) => rarity(banner, r.tierId)), [banner, shown]);
  const plan = useMemo(() => revealPlan(ranks, reduced), [ranks, reduced]);
  const best = bestTier(banner, shown.results);
  const color = tierCss(banner, best);
  const [stage, setStage] = useState<"charge" | "burst" | "reveal">(reduced ? "reveal" : "charge");
  const [turned, setTurned] = useState(reduced ? ranks.length : 0);
  const [done, setDone] = useState(reduced);
  const timers = useRef<ReturnType<typeof setTimeout>[]>([]);

  useEffect(() => {
    const at = (ms: number, fn: () => void) => timers.current.push(setTimeout(fn, ms));
    if (reduced) {
      gachaCue(RARITY_CUES[Math.max(0, ...ranks)]!);
    } else {
      gachaCue("charge");
      at(plan.burst, () => {
        setStage("burst");
        gachaCue("burst");
      });
      at(plan.deal, () => setStage("reveal"));
      plan.flips.forEach((ms, n) =>
        at(ms, () => {
          setTurned(n + 1);
          gachaCue("flip");
          gachaCue(RARITY_CUES[ranks[n]!]!);
        }),
      );
      at(plan.done, () => setDone(true));
    }
    const list = timers.current;
    return () => list.forEach(clearTimeout);
  }, [plan, ranks, reduced]);

  function skip() {
    if (done) return false;
    timers.current.forEach(clearTimeout);
    setStage("reveal");
    setTurned(ranks.length);
    setDone(true);
    gachaCue(RARITY_CUES[Math.max(0, ...ranks)]!);
    return true;
  }
  useEffect(() => {
    skipping.current = skip;
    return () => {
      if (skipping.current === skip) skipping.current = null;
    };
  });

  const count = shown.results.length;
  const price = pullCost(banner, count);
  const more = again(count);
  return (
    <div className="relative flex min-h-full flex-col items-center px-4 py-6" style={{ background: `radial-gradient(circle at 50% 45%, ${color}55, #0b0a14 70%)`, backgroundColor: "#0b0a14" }}>
      <Rays
        color={color}
        className="top-1/2 left-1/2 w-[60rem] max-w-none -translate-x-1/2 -translate-y-1/2"
        count={16}
        period={stage === "reveal" ? 60 : 8}
        opacity={stage === "reveal" ? 0.18 : 0.3}
      />
      <header className="relative z-10 flex w-full max-w-4xl items-center gap-3">
        <div className="min-w-0 flex-1">
          <DialogPrimitive.Title className="truncate text-xl font-extrabold tracking-tight">{banner.name}</DialogPrimitive.Title>
          <DialogPrimitive.Description className="text-sm text-white/70">
            {stage === "reveal" ? t("chattools.economy.gacha.revealAbout", { count }) : t("chattools.economy.gacha.opening", { banner: banner.name })}
          </DialogPrimitive.Description>
        </div>
        <SoundSwitch />
        {!done && (
          <button type="button" onClick={skip} className="rounded-full bg-white/10 px-3.5 py-1.5 text-sm font-bold backdrop-blur transition hover:bg-white/20 active:scale-95">
            {t("chattools.economy.gacha.skip")}
          </button>
        )}
      </header>

      <div className="relative z-10 flex w-full max-w-4xl flex-1 items-center justify-center py-6" onClick={() => stage !== "reveal" && skip()}>
        <AnimatePresence mode="wait">
          {stage !== "reveal" ? (
            <motion.div key="pack" className="relative grid place-items-center" exit={{ scale: 1.8, opacity: 0, transition: { duration: REVEAL.burst / 1000, ease: "easeIn" } }}>
              <motion.span
                aria-hidden
                className="absolute size-80 rounded-full blur-3xl"
                style={{ background: color }}
                initial={{ opacity: 0.15, scale: 0.5 }}
                animate={{ opacity: 0.55, scale: 1.1 }}
                transition={{ duration: REVEAL.charge / 1000, ease: "easeIn" }}
              />
              {/* Two sparks circling ever faster. */}
              <motion.span aria-hidden className="absolute size-72" animate={{ rotate: 720 }} transition={{ duration: REVEAL.charge / 1000, ease: "easeIn" }}>
                <span className="absolute top-0 left-1/2 size-3 -translate-x-1/2 rounded-full bg-white" style={{ boxShadow: `0 0 18px 6px ${color}` }} />
                <span className="absolute bottom-0 left-1/2 size-2 -translate-x-1/2 rounded-full bg-white" style={{ boxShadow: `0 0 14px 5px ${color}` }} />
              </motion.span>
              <motion.div
                initial={{ scale: 0.8, rotate: 0 }}
                animate={{ scale: [0.8, 1, 1.06], rotate: [0, -3, 3, -5, 5, -8, 8, -10, 10, 0] }}
                transition={{ duration: REVEAL.charge / 1000, ease: "easeIn" }}
              >
                <Pack art={art} glow={color} className={art ? "h-64 w-52" : "h-56 w-44"} />
              </motion.div>
            </motion.div>
          ) : (
            <motion.ul key="cards" className={cn("grid w-full gap-3", count > 1 ? "grid-cols-3 sm:grid-cols-5" : "w-52 grid-cols-1")} initial={{ opacity: 1 }} animate={{ opacity: 1 }}>
              {shown.results.map((r, n) => (
                <Card
                  key={n}
                  instanceKey={instanceKey}
                  result={r}
                  index={n}
                  rank={ranks[n]!}
                  turned={n < turned}
                  next={n === turned && !done}
                  banner={banner}
                  art={art}
                  currencies={currencies}
                  shop={shop}
                  roles={roles}
                  name={r.listingId ? r.prizeName || name(r.listingId) : r.prizeName}
                />
              ))}
            </motion.ul>
          )}
        </AnimatePresence>
        <AnimatePresence>
          {stage === "burst" && (
            <motion.span
              key="flash"
              aria-hidden
              className="pointer-events-none fixed inset-0 bg-white"
              initial={{ opacity: 0 }}
              animate={{ opacity: [0, 0.85, 0] }}
              transition={{ duration: 0.45, times: [0, 0.25, 1] }}
            />
          )}
        </AnimatePresence>
      </div>

      <AnimatePresence>
        {done && (
          <motion.footer key="end" className="relative z-10 flex flex-wrap justify-center gap-2" initial={{ opacity: 0, y: 16 }} animate={{ opacity: 1, y: 0 }} transition={SPRING}>
            {more && price !== null && (
              <button
                type="button"
                onClick={more}
                disabled={!!busy}
                className="flex h-12 items-center gap-2 rounded-2xl bg-white/10 px-5 font-extrabold backdrop-blur transition hover:bg-white/20 active:scale-95 disabled:opacity-60"
              >
                {busy ? <LoaderCircleIcon className="size-4 animate-spin" /> : <RotateCcwIcon className="size-4" />}
                {t("chattools.economy.gacha.again")}
                <Amount instanceKey={instanceKey} currency={currency} value={price} className="text-sm font-bold text-white/80" iconClassName="size-3.5" />
              </button>
            )}
            <ShareButton instanceKey={instanceKey} serverId={serverId} pullId={shown.id} />
            <SaveButton banner={banner} pull={shown} instanceKey={instanceKey} />
            <button
              type="button"
              onClick={onClose}
              className="h-12 rounded-2xl px-7 font-extrabold text-white shadow-lg transition hover:brightness-110 active:scale-95"
              style={{ background: `linear-gradient(135deg, ${color}, var(--primary))` }}
            >
              {t("chattools.economy.gacha.done")}
            </button>
          </motion.footer>
        )}
      </AnimatePresence>
    </div>
  );
}

const FOOT_BUTTON = "flex h-12 items-center gap-2 rounded-2xl bg-white/10 px-5 font-extrabold backdrop-blur transition hover:bg-white/20 active:scale-95 disabled:opacity-60";

/** Shows the pull in one of the server's channels you can post in (Message.gacha). */
function ShareButton({ instanceKey, serverId, pullId }: { instanceKey: string; serverId: string; pullId: string }) {
  const { t } = useI18n();
  const access = useAccess(instanceKey, serverId);
  const all = useFuwa((s) => s.instances[instanceKey]?.channels[serverId]);
  const [sending, setSending] = useState(false);
  const [sharedIn, setSharedIn] = useState<string | null>(null);
  const channels = useMemo(
    () => (all ?? []).filter((c) => (c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT) && !c.shared && !access.pending && hasIn(access, c.id, P.SEND_MESSAGES)),
    [all, access],
  );
  if (!shareHere(instanceKey)) return null;
  async function share(channelId: string, name: string) {
    setSending(true);
    try {
      await sharePull(instanceKey, serverId, channelId, pullId);
      setSharedIn(name);
      toast(t("chattools.economy.gacha.sharedIn", { channel: name }));
    } catch (err) {
      toast(economyMessage(err));
    } finally {
      setSending(false);
    }
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" disabled={sending} className={FOOT_BUTTON}>
          {sending ? <LoaderCircleIcon className="size-4 animate-spin" /> : sharedIn ? <CheckIcon className="size-4" /> : <Share2Icon className="size-4" />}
          {t("chattools.economy.gacha.share")}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="center" side="top" className="z-[70] max-h-72 w-60">
        <DropdownMenuLabel>{t("chattools.economy.gacha.shareIn")}</DropdownMenuLabel>
        {channels.length ? (
          channels.map((c) => (
            <DropdownMenuItem key={c.id} onSelect={() => void share(c.id, c.name)}>
              <HashIcon className="size-4 text-muted-foreground" />
              <span className="truncate">{c.name}</span>
            </DropdownMenuItem>
          ))
        ) : (
          <p className="px-2 py-1.5 text-sm text-muted-foreground">{t("chattools.economy.gacha.noChannels")}</p>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Saves a picture of the pull's cards. */
function SaveButton({ instanceKey, banner, pull: shown }: { instanceKey: string; banner: GachaBanner; pull: GachaPull }) {
  const { t } = useI18n();
  const me = useFuwa((s) => s.instances[instanceKey]?.me);
  const save = useSaveImage();
  const [saving, setSaving] = useState(false);
  return (
    <button
      type="button"
      disabled={saving}
      onClick={() => {
        setSaving(true);
        void save(cardsOfPull(shown, banner), displayName(me ?? undefined)).finally(() => setSaving(false));
      }}
      className={FOOT_BUTTON}
    >
      {saving ? <LoaderCircleIcon className="size-4 animate-spin" /> : <DownloadIcon className="size-4" />}
      {t("chattools.economy.gacha.saveImage")}
    </button>
  );
}

/** Where a card's sparks fly when a rare one turns. */
const SPARKS = Array.from({ length: 10 }, (_, n) => {
  const a = (n / 10) * Math.PI * 2;
  return { x: Math.cos(a), y: Math.sin(a), d: 60 + (n % 3) * 18 };
});

function Card({
  instanceKey,
  result,
  index,
  rank,
  turned,
  next,
  banner,
  art,
  currencies,
  shop,
  roles,
  name,
}: {
  instanceKey: string;
  result: GachaResult;
  index: number;
  /** How grand its moment is (`rarity`). */
  rank: number;
  turned: boolean;
  /** It turns next: a rare one trembles in its color first. */
  next: boolean;
  banner: GachaBanner;
  art: string | null | undefined;
  currencies: Currency[];
  shop: ShopListing[];
  roles: Role[];
  name: string;
}) {
  const { t } = useI18n();
  const tierIndex = banner.tiers.findIndex((tier) => tier.id === result.tierId);
  const color = tierCss(banner, tierIndex);
  const glow = glowOf(banner);
  const currency = currencies.find((c) => c.id === result.currencyId);
  const listing = shop.find((l) => l.id === result.listingId);
  const role = listing?.kind === LISTING_ROLE ? roles.find((r) => r.id === listing.targetId) : undefined;
  const flip = 0.11;
  const n = stars(banner, result.tierId);
  let glyph: ReactNode;
  if (!result.listingId) glyph = <CurrencyIcon instanceKey={instanceKey} currency={currency} className="size-10 text-4xl" />;
  else if (listing?.kind === LISTING_ROLE) glyph = <ShieldIcon className="size-9" style={role?.color !== undefined ? { color: cssColor(role.color) } : undefined} />;
  else if (listing?.kind === LISTING_PROFILE_ITEM) glyph = <SparklesIcon className="size-9" />;
  else glyph = <PackageIcon className="size-9" />;
  return (
    <motion.li
      className="relative aspect-[3/4]"
      initial={{ opacity: 0, y: 60, scale: 0.6, rotate: index % 2 ? 8 : -8 }}
      animate={{ opacity: 1, y: 0, scale: 1, rotate: 0 }}
      transition={{ ...SPRING, delay: (index * REVEAL.deal) / 1000 }}
    >
      {turned && rank >= 3 && <Rays color={color} className="top-1/2 left-1/2 w-[260%] max-w-none -translate-x-1/2 -translate-y-1/2" count={12} period={10} opacity={0.5} />}
      {turned &&
        rank >= 2 &&
        SPARKS.map((s, k) => (
          <motion.span
            key={k}
            aria-hidden
            className="pointer-events-none absolute top-1/2 left-1/2 z-20 text-sm"
            style={{ color }}
            initial={{ x: 0, y: 0, opacity: 0, scale: 0.4 }}
            animate={{ x: s.x * s.d, y: s.y * s.d, opacity: [0, 1, 0], scale: [0.4, 1.2, 0.6] }}
            transition={{ duration: 0.8, ease: "easeOut" }}
          >
            ✦
          </motion.span>
        ))}
      {/* The back, until it turns. */}
      <motion.div
        aria-hidden
        className="absolute inset-0 grid place-items-center overflow-hidden rounded-2xl border-2 border-white/25 shadow-lg"
        style={{ background: `linear-gradient(160deg, ${glow}, #2a2540)`, boxShadow: next && rank >= 2 ? `0 0 28px ${color}` : undefined }}
        initial={false}
        animate={{ scaleX: turned ? 0 : 1, rotate: next && rank >= 2 ? [0, -3, 3, -3, 3, 0] : 0 }}
        transition={{ scaleX: { duration: flip, ease: "easeIn" }, rotate: { duration: 0.4, repeat: next && rank >= 2 ? Infinity : 0 } }}
      >
        <span className="absolute inset-2 rounded-xl border border-white/20" />
        {art ? <img src={art} alt="" draggable={false} className="size-1/2 object-contain opacity-80" /> : <SparklesIcon className="size-1/3 text-white/80" />}
      </motion.div>
      {/* The face. */}
      <motion.div
        className="absolute inset-0 flex flex-col items-center overflow-hidden rounded-2xl border-2 bg-[#15131f] p-2 text-center"
        style={{ borderColor: color, boxShadow: rank >= 1 ? `0 0 ${8 + rank * 10}px ${color}${rank >= 2 ? "aa" : "66"}` : undefined }}
        initial={false}
        animate={turned ? { scaleX: 1, scale: rank >= 2 ? [1, 1.1, 1] : 1 } : { scaleX: 0 }}
        transition={{ scaleX: { duration: flip, delay: turned ? flip : 0, ease: "easeOut" }, scale: { duration: 0.45, delay: flip } }}
      >
        <span aria-hidden className="absolute inset-0 opacity-30" style={{ background: `radial-gradient(circle at 50% 38%, ${color}, transparent 65%)` }} />
        {rank >= 3 && turned && (
          <motion.span
            aria-hidden
            className="absolute inset-y-0 w-1/2 -skew-x-12 bg-gradient-to-r from-transparent via-white/30 to-transparent"
            animate={{ left: ["-60%", "130%"] }}
            transition={{ duration: 1.4, repeat: Infinity, repeatDelay: 0.8 }}
          />
        )}
        <span className="relative text-xs leading-none" style={{ color }} aria-label={t("chattools.economy.gacha.stars", { count: n })}>
          {"★".repeat(n)}
        </span>
        <span className="relative my-auto grid size-14 place-items-center rounded-full" style={{ backgroundColor: `${color}2e`, color }}>
          {glyph}
        </span>
        <span className="relative line-clamp-2 text-xs leading-tight font-extrabold break-words sm:text-sm">{name}</span>
        <span className="relative mt-0.5 text-[0.6rem] font-extrabold tracking-wide uppercase" style={{ color }}>
          {result.tierName}
        </span>
        {(result.outcome !== OUTCOME_WON || result.guaranteed) && (
          <span className="relative mt-1 rounded-full bg-white/10 px-1.5 text-[0.6rem] font-bold text-white/80">
            {result.outcome !== OUTCOME_WON ? (
              <>
                {t(outcomeLabel(result.outcome))}
                {result.amount > 0n && (
                  <>
                    {" "}
                    <Amount instanceKey={instanceKey} currency={currency} value={num(result.amount)} className="font-bold" iconClassName="size-2.5" />
                  </>
                )}
              </>
            ) : (
              t("chattools.economy.gacha.guaranteed")
            )}
          </span>
        )}
      </motion.div>
    </motion.li>
  );
}
