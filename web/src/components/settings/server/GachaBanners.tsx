import { ArrowUpIcon, CircleCheckIcon, CircleXIcon, DicesIcon, EyeOffIcon, ImageUpIcon, InfinityIcon, LoaderCircleIcon, PlusIcon, Trash2Icon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useMemo, useRef, useState } from "react";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import type { Currency, GachaBanner, ShopListing } from "@/gen/fuwa/v1/types_pb";
import { createBanner, deleteBanner, economyMessage, updateBanner, useEconomy } from "@/fuwa/economy";
import { run, uploadPicture } from "@/fuwa/actions";
import { useInstance } from "@/fuwa/hooks";
import { Amount, CurrencyIcon, listingName } from "@/components/economy/shared";
import { confirmFirst } from "@/components/menus/dialogs";
import { Count } from "@/components/motion";
import { Segmented } from "@/components/settings/account/common";
import { SaveBar, Toggle } from "@/components/settings/controls";
import { Card, Empty, Field, Section, Shimmer } from "@/components/settings/server/Economy";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { T, useI18n, type Key } from "@/i18n/react";
import { num } from "@/lib/economy";
import {
  BANNER_DESCRIPTION_MAX,
  BANNER_NAME_MAX,
  bannerDraft,
  bannerValue,
  draftChanged,
  draftChances,
  DUPLICATES_KEEP,
  DUPLICATES_REFUND,
  DUPLICATES_SKIP_OWNED,
  newTierId,
  openness,
  percent,
  prizeListings,
  PRIZES_MAX,
  ratesTotal,
  TIER_NAME_MAX,
  tierColor,
  TIERS_MAX,
  WHOLE,
  type BannerDraft,
  type BannerProblem,
  type PrizeDraft,
  type TierDraft,
} from "@/lib/gacha";
import { useNow } from "@/lib/notifications";
import { cssColor } from "@/lib/permissions";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { PICTURE_TYPES } from "@/lib/pictures";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

const NEW = "new";

const PROBLEM: Record<BannerProblem, Key> = {
  name: "serversettings.economy.gacha.problem.name",
  currency: "serversettings.economy.problem.currency",
  price: "serversettings.economy.problem.price",
  multi: "serversettings.economy.gacha.problem.multi",
  multiPrice: "serversettings.economy.gacha.problem.multiPrice",
  tiers: "serversettings.economy.gacha.problem.tiers",
  tierName: "serversettings.economy.gacha.problem.tierName",
  tierRate: "serversettings.economy.gacha.problem.tierRate",
  ratesTotal: "serversettings.economy.gacha.problem.ratesTotal",
  prizes: "serversettings.economy.gacha.problem.prizes",
  prizeWhat: "serversettings.economy.gacha.problem.prizeWhat",
  prizeAmount: "serversettings.economy.gacha.problem.prizeAmount",
  prizeWeight: "serversettings.economy.gacha.problem.prizeWeight",
  emptyTier: "serversettings.economy.gacha.problem.emptyTier",
  pity: "serversettings.economy.gacha.problem.pity",
  refund: "serversettings.economy.gacha.problem.refund",
  maxPulls: "serversettings.economy.gacha.problem.maxPulls",
  dates: "serversettings.economy.gacha.problem.dates",
  position: "serversettings.economy.problem.position",
};

const SELECT = "h-10 w-full rounded-xl border bg-background px-3 text-sm font-bold outline-none focus-visible:ring-2 focus-visible:ring-ring";

/**
 * Server settings > Economy's gacha banners (docs/economy.md), for Manage
 * Economy: each banner's price and multi-pull, its rarity tiers and their
 * rates, the prizes in each with their weights (and each one's chance as it
 * stands), pity, what duplicates do, limits and when it's open.
 */
export function BannersSection({ instanceKey, serverId, currencies, shop }: { instanceKey: string; serverId: string; currencies: Currency[] | null; shop: ShopListing[] | null }) {
  const { t } = useI18n();
  const { banners } = useEconomy(instanceKey, serverId, true);
  const [open, setOpen] = useState<string | null>(null);
  return (
    <Section
      title={t("serversettings.economy.gacha.title")}
      count={banners?.length ?? 0}
      action={
        <Button type="button" size="sm" className="btn rounded-xl font-bold" disabled={!currencies?.length || !banners || open === NEW} onClick={() => setOpen(NEW)}>
          <PlusIcon /> {t("serversettings.economy.gacha.new")}
        </Button>
      }
    >
      <p className="-mt-1 text-xs text-muted-foreground">{t("serversettings.economy.gacha.intro")}</p>
      {!banners || !currencies || !shop ? (
        <Shimmer />
      ) : (
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false}>
            {open === NEW && (
              <BannerCard key="new" instanceKey={instanceKey} serverId={serverId} currencies={currencies} shop={shop} position={banners.length} open onToggle={() => setOpen(null)} onCreated={(b) => setOpen(b.id)} />
            )}
            {banners.map((b) => (
              <BannerCard key={b.id} instanceKey={instanceKey} serverId={serverId} banner={b} currencies={currencies} shop={shop} position={b.position} open={open === b.id} onToggle={() => setOpen((o) => (o === b.id ? null : b.id))} />
            ))}
          </AnimatePresence>
          {!banners.length && open !== NEW && <Empty icon="🎰" title={t("serversettings.economy.gacha.none")} hint={currencies.length ? t("serversettings.economy.gacha.noneHint") : t("serversettings.economy.currencyFirst")} />}
        </ul>
      )}
    </Section>
  );
}

/**
 * The box or card pack members see on the banner, uploaded here for the
 * server. Optional: without one the banner shows its lead tier's color.
 */
function BannerArt({ instanceKey, serverId, url, glow, onChange }: { instanceKey: string; serverId: string; url: string; glow: string; onChange: (url: string) => void }) {
  const { t } = useI18n();
  const input = useRef<HTMLInputElement>(null);
  const [sent, setSent] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const shown = shownPicture(url);

  async function send(file: File | undefined) {
    if (!file) return;
    if (!PICTURE_TYPES.includes(file.type)) {
      setError(t("serversettings.economy.gacha.artType"));
      return;
    }
    setError(null);
    setSent(0);
    try {
      onChange(await run(uploadPicture(instanceKey, MediaPurpose.SHOP_ART, file, setSent, serverId)));
    } catch (err) {
      setError((err as Error).message || t("serversettings.economy.gacha.artFailed"));
    } finally {
      setSent(null);
    }
  }

  return (
    <div className="flex flex-col gap-1">
      <span className="text-xs font-bold text-muted-foreground uppercase">{t("serversettings.economy.gacha.art")}</span>
      <div
        className="flex items-center gap-3"
        onDragOver={(e) => {
          if (e.dataTransfer.types.includes("Files")) e.preventDefault();
        }}
        onDrop={(e) => {
          e.preventDefault();
          void send(e.dataTransfer.files[0]);
        }}
      >
        <button
          type="button"
          disabled={sent !== null}
          onClick={() => input.current?.click()}
          aria-label={t("serversettings.economy.gacha.artUpload")}
          className="relative grid h-24 w-20 shrink-0 place-items-center overflow-hidden rounded-2xl border-2 border-dashed transition-colors hover:border-primary focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
          style={shown ? { borderStyle: "solid", borderColor: "transparent", background: `radial-gradient(circle, ${glow}33, transparent 70%)` } : undefined}
        >
          {shown ? <img src={shown} alt="" className="size-full object-contain" /> : <ImageUpIcon className="size-6 text-muted-foreground" />}
          {sent !== null && (
            <span className="absolute inset-0 grid place-items-center bg-background/70">
              <LoaderCircleIcon className="size-5 animate-spin" />
              <span className="absolute inset-x-2 bottom-2 h-1 overflow-hidden rounded-full bg-muted">
                <span className="block h-full bg-primary transition-[width]" style={{ width: `${Math.round(sent * 100)}%` }} />
              </span>
            </span>
          )}
        </button>
        <div className="flex min-w-0 flex-col items-start gap-1.5">
          <span className="text-xs text-muted-foreground">{t("serversettings.economy.gacha.artHint")}</span>
          <div className="flex gap-2">
            <Button type="button" size="sm" variant="outline" className="rounded-xl" disabled={sent !== null} onClick={() => input.current?.click()}>
              <ImageUpIcon /> {url ? t("serversettings.economy.gacha.artReplace") : t("serversettings.economy.gacha.artUpload")}
            </Button>
            {url && (
              <Button type="button" size="sm" variant="ghost" className="rounded-xl text-destructive" disabled={sent !== null} onClick={() => onChange("")}>
                <Trash2Icon /> {t("serversettings.economy.gacha.artRemove")}
              </Button>
            )}
          </div>
          {error && <span className="text-xs font-bold text-destructive">{error}</span>}
        </div>
      </div>
      <input
        ref={input}
        type="file"
        accept={PICTURE_TYPES.join(",")}
        className="hidden"
        onChange={(e) => {
          void send(e.target.files?.[0]);
          e.target.value = "";
        }}
      />
    </div>
  );
}

function BannerCard({
  instanceKey,
  serverId,
  banner,
  currencies,
  shop,
  position,
  open,
  onToggle,
  onCreated,
}: {
  instanceKey: string;
  serverId: string;
  banner?: GachaBanner;
  currencies: Currency[];
  shop: ShopListing[];
  position: number;
  open: boolean;
  onToggle: () => void;
  onCreated?: (b: GachaBanner) => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const now = useNow(60_000);
  const starters: [string, string, string] = [t("serversettings.economy.gacha.starter.legendary"), t("serversettings.economy.gacha.starter.rare"), t("serversettings.economy.gacha.starter.common")];
  const fresh = () => bannerDraft(banner, currencies[0]?.id, position, starters);
  const [draft, setDraft] = useState<BannerDraft>(fresh);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [base, setBase] = useState(banner);
  if (banner !== base) {
    setBase(banner);
    if (!draftChanged(base, draft)) setDraft(bannerDraft(banner, currencies[0]?.id, position, starters));
  }
  const checked = bannerValue(draft, banner?.id);
  const problem = "problem" in checked ? checked : null;
  const changed = draftChanged(banner, draft);
  const set = (patch: Partial<BannerDraft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    setError(null);
  };
  const currency = currencies.find((c) => c.id === draft.currencyId);
  const glow = cssColor(tierColor(banner?.tiers.find((x) => x.rate > 0) ?? {}, 0));
  const state = banner ? openness(banner, now).state : null;

  async function save() {
    if ("problem" in checked) {
      setError(t(PROBLEM[checked.problem]));
      return;
    }
    setSaving(true);
    setError(null);
    try {
      if (banner) await updateBanner(instanceKey, serverId, checked.banner);
      else {
        const made = await createBanner(instanceKey, serverId, checked.banner);
        if (made) onCreated?.(made);
      }
    } catch (err) {
      setError(economyMessage(err));
    } finally {
      setSaving(false);
    }
  }

  function remove() {
    if (!banner) return;
    confirmFirst({
      title: t("serversettings.economy.gacha.deleteTitle", { name: banner.name }),
      body: t("serversettings.economy.gacha.deleteBody"),
      action: t("serversettings.economy.delete"),
      run: () => deleteBanner(instanceKey, serverId, banner.id),
    });
  }

  return (
    <Card
      open={open}
      onToggle={onToggle}
      summary={
        <>
          {shownPicture(banner?.artUrl) ? (
            <img src={shownPicture(banner?.artUrl)} alt="" className="size-10 shrink-0 rounded-2xl object-contain" />
          ) : (
            <span className="grid size-10 shrink-0 place-items-center rounded-2xl text-white" style={{ backgroundColor: glow }}>
              <DicesIcon className="size-5" />
            </span>
          )}
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="truncate font-bold">{banner?.name || t("serversettings.economy.gacha.new")}</span>
              {banner && !banner.available && (
                <span className="flex items-center gap-0.5 rounded bg-muted px-1 text-[0.6rem] font-extrabold tracking-wide text-muted-foreground uppercase">
                  <EyeOffIcon className="size-2.5" /> {t("serversettings.economy.gacha.hidden")}
                </span>
              )}
              {banner?.available && state !== "open" && (
                <span className="rounded bg-muted px-1 text-[0.6rem] font-extrabold tracking-wide text-muted-foreground uppercase">
                  {state === "soon" ? t("serversettings.economy.gacha.soon") : t("serversettings.economy.gacha.closed")}
                </span>
              )}
            </span>
            {banner && (
              <span className="flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
                <Amount instanceKey={instanceKey} currency={currencies.find((c) => c.id === banner.currencyId)} value={num(banner.pullPrice)} iconClassName="size-3" />
                <span>
                  <T k="serversettings.economy.gacha.tierCount" values={{ count: <Count value={banner.tiers.length} /> }} count={banner.tiers.length} />
                </span>
                <span>
                  <T k="serversettings.economy.gacha.prizeCount" values={{ count: <Count value={banner.prizes.length} /> }} count={banner.prizes.length} />
                </span>
              </span>
            )}
          </span>
        </>
      }
    >
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("serversettings.economy.gacha.name")} wrong={problem?.problem === "name"}>
          <Input value={draft.name} maxLength={BANNER_NAME_MAX} onChange={(e) => set({ name: e.target.value })} placeholder={t("serversettings.economy.gacha.namePlaceholder")} className="h-10 rounded-xl font-bold" />
        </Field>
        <Field label={t("serversettings.economy.gacha.price")} wrong={problem?.problem === "price"}>
          <div className="relative">
            <span className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2">
              <CurrencyIcon instanceKey={instanceKey} currency={currency} />
            </span>
            <Input inputMode="numeric" value={draft.price} onChange={(e) => set({ price: e.target.value })} className="h-10 rounded-xl pl-9 font-bold tabular-nums" />
          </div>
        </Field>
      </div>
      {currencies.length > 1 && (
        <Field label={t("chattools.economy.currency")}>
          <Segmented label={t("chattools.economy.currency")} value={draft.currencyId} onChange={(currencyId) => set({ currencyId })} options={currencies.map((c) => ({ value: c.id, label: c.name }))} className="self-start" />
        </Field>
      )}
      <BannerArt instanceKey={instanceKey} serverId={serverId} url={draft.artUrl} glow={glow} onChange={(artUrl) => set({ artUrl })} />
      <Field label={t("serversettings.economy.description")}>
        <Textarea rows={2} value={draft.description} maxLength={BANNER_DESCRIPTION_MAX} onChange={(e) => set({ description: e.target.value })} placeholder={t("serversettings.economy.gacha.descriptionPlaceholder")} className="rounded-xl" />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("serversettings.economy.gacha.multi")} wrong={problem?.problem === "multi"} hint={t("serversettings.economy.gacha.multiHint")}>
          <Input inputMode="numeric" value={draft.multi} placeholder={t("serversettings.economy.gacha.none")} onChange={(e) => set({ multi: e.target.value })} className="h-10 rounded-xl tabular-nums" />
        </Field>
        <Field label={t("serversettings.economy.gacha.multiPrice")} wrong={problem?.problem === "multiPrice"} hint={t("serversettings.economy.gacha.multiPriceHint")}>
          <Input inputMode="numeric" value={draft.multiPrice} disabled={!draft.multi.trim()} placeholder={draft.multi.trim() && Number(draft.multi) ? String(Number(draft.price || 0) * Number(draft.multi)) : ""} onChange={(e) => set({ multiPrice: e.target.value })} className="h-10 rounded-xl tabular-nums" />
        </Field>
      </div>

      <Tiers draft={draft} set={set} wrong={problem && ["tierName", "tierRate", "ratesTotal", "tiers"].includes(problem.problem) ? problem.index ?? -1 : null} />
      <Prizes instanceKey={instanceKey} serverId={serverId} draft={draft} set={set} currencies={currencies} shop={shop} wrong={problem && problem.problem.startsWith("prize") ? problem.index ?? -1 : null} emptyTier={problem?.problem === "emptyTier" ? problem.index ?? -1 : null} />

      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("serversettings.economy.gacha.pity")} wrong={problem?.problem === "pity"} hint={t("serversettings.economy.gacha.pityHint")}>
          <Input inputMode="numeric" value={draft.pityPulls} placeholder={t("serversettings.economy.gacha.none")} onChange={(e) => set({ pityPulls: e.target.value })} className="h-10 rounded-xl tabular-nums" />
        </Field>
        {draft.pityPulls.trim() !== "" && (
          <Field label={t("serversettings.economy.gacha.pityTier")}>
            <select value={draft.pityTierId} onChange={(e) => set({ pityTierId: e.target.value })} className={SELECT}>
              {draft.tiers.map((tier) => (
                <option key={tier.id} value={tier.id}>
                  {tier.name || tier.id}
                </option>
              ))}
            </select>
          </Field>
        )}
      </div>

      <Field label={t("serversettings.economy.gacha.duplicates")} wrong={problem?.problem === "refund"}>
        <Segmented
          label={t("serversettings.economy.gacha.duplicates")}
          value={draft.duplicates}
          onChange={(duplicates) => set({ duplicates })}
          className="self-start"
          options={[
            { value: DUPLICATES_KEEP, label: t("serversettings.economy.gacha.dupKeep") },
            { value: DUPLICATES_REFUND, label: t("serversettings.economy.gacha.dupRefund") },
            { value: DUPLICATES_SKIP_OWNED, label: t("serversettings.economy.gacha.dupSkip") },
          ]}
        />
        <span className="text-xs text-muted-foreground">
          {t(draft.duplicates === DUPLICATES_REFUND ? "serversettings.economy.gacha.dupRefundHint" : draft.duplicates === DUPLICATES_SKIP_OWNED ? "serversettings.economy.gacha.dupSkipHint" : "serversettings.economy.gacha.dupKeepHint")}
        </span>
      </Field>
      {draft.duplicates === DUPLICATES_REFUND && (
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("serversettings.economy.gacha.refund")} wrong={problem?.problem === "refund"}>
            <div className="relative">
              <span className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2">
                <CurrencyIcon instanceKey={instanceKey} currency={currencies.find((c) => c.id === draft.refundCurrencyId)} />
              </span>
              <Input inputMode="numeric" value={draft.refundAmount} onChange={(e) => set({ refundAmount: e.target.value })} className="h-10 rounded-xl pl-9 tabular-nums" />
            </div>
          </Field>
          {currencies.length > 1 && (
            <Field label={t("serversettings.economy.gacha.refundIn")}>
              <select value={draft.refundCurrencyId} onChange={(e) => set({ refundCurrencyId: e.target.value })} className={SELECT}>
                {currencies.map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
              </select>
            </Field>
          )}
        </div>
      )}

      <div className="grid gap-3 sm:grid-cols-3">
        <Field label={t("serversettings.economy.gacha.maxPulls")} wrong={problem?.problem === "maxPulls"} hint={t("serversettings.economy.gacha.maxPullsHint")}>
          <div className="flex items-center gap-2">
            <Input inputMode="numeric" value={draft.maxPulls} placeholder={t("settings.controls.noLimit")} onChange={(e) => set({ maxPulls: e.target.value })} className="h-10 flex-1 rounded-xl tabular-nums" />
            <Button type="button" variant={draft.maxPulls === "" ? "secondary" : "outline"} size="icon" className="size-10 rounded-xl" title={t("settings.controls.noLimit")} aria-label={t("settings.controls.noLimit")} onClick={() => set({ maxPulls: "" })}>
              <InfinityIcon />
            </Button>
          </div>
        </Field>
        <Field label={t("serversettings.economy.gacha.startsAt")} wrong={problem?.problem === "dates"} hint={t("serversettings.economy.gacha.timeHint")}>
          <Input type="datetime-local" value={draft.startsAt} onChange={(e) => set({ startsAt: e.target.value })} className="h-10 rounded-xl" />
        </Field>
        <Field label={t("serversettings.economy.gacha.endsAt")} wrong={problem?.problem === "dates"}>
          <Input type="datetime-local" value={draft.endsAt} onChange={(e) => set({ endsAt: e.target.value })} className="h-10 rounded-xl" />
        </Field>
      </div>
      <Field label={t("serversettings.economy.position")} wrong={problem?.problem === "position"} hint={t("serversettings.economy.positionHint")}>
        <Input inputMode="numeric" value={draft.position} onChange={(e) => set({ position: e.target.value })} className="h-10 max-w-40 rounded-xl tabular-nums" />
      </Field>
      <Toggle checked={draft.available} onChange={(available) => set({ available })} label={t("serversettings.economy.gacha.available")} hint={t("serversettings.economy.gacha.availableHint")} />
      {banner && (
        <div className="flex justify-end">
          <Button type="button" variant="ghost" size="sm" className="rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive" onClick={remove}>
            <Trash2Icon /> {t("serversettings.economy.gacha.delete")}
          </Button>
        </div>
      )}
      <SaveBar
        count={open && changed ? 1 : 0}
        saving={saving}
        error={error}
        onSave={() => void save()}
        onDiscard={() => {
          setDraft(fresh());
          setError(null);
          if (!banner) onToggle();
        }}
      />
    </Card>
  );
}

/** The rarity tiers, rarest first, with their rates and what they add up to. */
function Tiers({ draft, set, wrong }: { draft: BannerDraft; set: (patch: Partial<BannerDraft>) => void; wrong: number | null }) {
  const { t } = useI18n();
  const total = ratesTotal(draft.tiers);
  const whole = total === WHOLE;
  const change = (i: number, patch: Partial<TierDraft>) => set({ tiers: draft.tiers.map((x, n) => (n === i ? { ...x, ...patch } : x)) });
  const remove = (i: number) => {
    const gone = draft.tiers[i]!;
    const tiers = draft.tiers.filter((_, n) => n !== i);
    set({ tiers, prizes: draft.prizes.filter((p) => p.tierId !== gone.id), pityTierId: draft.pityTierId === gone.id ? (tiers[0]?.id ?? "") : draft.pityTierId });
  };
  const up = (i: number) => {
    const tiers = [...draft.tiers];
    [tiers[i - 1], tiers[i]] = [tiers[i]!, tiers[i - 1]!];
    set({ tiers });
  };
  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <span className={cn("flex-1 text-xs font-bold text-muted-foreground uppercase", wrong !== null && "text-destructive")}>{t("serversettings.economy.gacha.tiers")}</span>
        <span className={cn("flex items-center gap-1 text-xs font-bold tabular-nums", whole ? "text-emerald-600 dark:text-emerald-400" : "text-destructive")}>
          {whole ? <CircleCheckIcon className="size-3.5" /> : <CircleXIcon className="size-3.5" />}
          {t("serversettings.economy.gacha.total", { total: total === null ? "?" : percent(total) })}
        </span>
      </div>
      <p className="text-xs text-muted-foreground">{t("serversettings.economy.gacha.tiersHint")}</p>
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false}>
          {draft.tiers.map((tier, i) => (
            <motion.li key={tier.id} layout="position" {...SLIDE_IN} transition={SPRING} className={cn("flex items-center gap-2 rounded-xl border p-1.5", wrong === i && "border-destructive")}>
              <label className="relative size-8 shrink-0 cursor-pointer overflow-hidden rounded-lg" style={{ backgroundColor: cssColor(tierColor({ color: tier.color ?? undefined }, i)) }} title={t("serversettings.economy.gacha.tierColor")}>
                <input
                  type="color"
                  value={cssColor(tierColor({ color: tier.color ?? undefined }, i))}
                  onChange={(e) => change(i, { color: parseInt(e.target.value.slice(1), 16) })}
                  className="absolute inset-0 size-full cursor-pointer opacity-0"
                  aria-label={t("serversettings.economy.gacha.tierColor")}
                />
              </label>
              <Input value={tier.name} maxLength={TIER_NAME_MAX} aria-label={t("serversettings.economy.gacha.tierName")} onChange={(e) => change(i, { name: e.target.value })} className="h-8 min-w-0 flex-1 rounded-lg font-bold" />
              <div className="relative w-28 shrink-0">
                <Input inputMode="decimal" value={tier.rate} aria-label={t("serversettings.economy.gacha.tierRate")} onChange={(e) => change(i, { rate: e.target.value })} className="h-8 rounded-lg pr-7 text-right tabular-nums" />
                <span className="pointer-events-none absolute top-1/2 right-2.5 -translate-y-1/2 text-xs text-muted-foreground">%</span>
              </div>
              <Button type="button" variant="ghost" size="icon" className="size-8 rounded-lg" disabled={i === 0} aria-label={t("serversettings.economy.gacha.rarer")} title={t("serversettings.economy.gacha.rarer")} onClick={() => up(i)}>
                <ArrowUpIcon />
              </Button>
              <Button type="button" variant="ghost" size="icon" className="size-8 rounded-lg" disabled={draft.tiers.length <= 1} aria-label={t("serversettings.economy.gacha.removeTier")} onClick={() => remove(i)}>
                <XIcon />
              </Button>
            </motion.li>
          ))}
        </AnimatePresence>
      </ul>
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="self-start rounded-xl"
        disabled={draft.tiers.length >= TIERS_MAX}
        onClick={() => set({ tiers: [...draft.tiers, { id: newTierId(draft.tiers), name: "", color: null, rate: "0" }] })}
      >
        <PlusIcon /> {t("serversettings.economy.gacha.addTier")}
      </Button>
    </div>
  );
}

let fresh = 0;

/** The prizes, under their tiers, each with its weight and chance as it stands. */
function Prizes({
  instanceKey,
  serverId,
  draft,
  set,
  currencies,
  shop,
  wrong,
  emptyTier,
}: {
  instanceKey: string;
  serverId: string;
  draft: BannerDraft;
  set: (patch: Partial<BannerDraft>) => void;
  currencies: Currency[];
  shop: ShopListing[];
  wrong: number | null;
  emptyTier: number | null;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const roles = useMemo(() => inst?.roles[serverId] ?? [], [inst?.roles, serverId]);
  const items = inst?.serverProfileItems[serverId];
  const chances = draftChances(draft);
  const listings = prizeListings(shop);
  const nameOf = (l: ShopListing) => listingName(l, roles, items, t("chattools.economy.somethingFromShop"));
  const change = (id: string, patch: Partial<PrizeDraft>) => set({ prizes: draft.prizes.map((p) => (p.id === id ? { ...p, ...patch } : p)) });
  const add = (tierId: string, what: string) => {
    const prize: PrizeDraft =
      what === "currency"
        ? { id: `new:${++fresh}`, tierId, listingId: "", currencyId: draft.currencyId || currencies[0]?.id || "", amount: "10", weight: "1" }
        : { id: `new:${++fresh}`, tierId, listingId: what, currencyId: "", amount: "", weight: "1" };
    set({ prizes: [...draft.prizes, prize] });
  };
  return (
    <div className="flex flex-col gap-2">
      <span className={cn("text-xs font-bold text-muted-foreground uppercase", (wrong !== null || emptyTier !== null) && "text-destructive")}>{t("serversettings.economy.gacha.prizes")}</span>
      <p className="text-xs text-muted-foreground">{t("serversettings.economy.gacha.prizesHint")}</p>
      {draft.tiers.map((tier, ti) => {
        const inTier = draft.prizes.filter((p) => p.tierId === tier.id);
        return (
          <div key={tier.id} className={cn("flex flex-col gap-1.5 rounded-2xl border border-dashed p-2", emptyTier === ti && "border-destructive")}>
            <p className="flex items-center gap-2 px-1 text-sm font-extrabold">
              <span className="size-2.5 rounded-full" style={{ backgroundColor: cssColor(tierColor({ color: tier.color ?? undefined }, ti)) }} />
              {tier.name || t("serversettings.economy.gacha.unnamedTier")}
              <span className="ml-auto text-xs font-bold text-muted-foreground tabular-nums">{tier.rate || "0"}%</span>
            </p>
            <ul className="flex flex-col gap-1">
              {inTier.map((p) => {
                const index = draft.prizes.indexOf(p);
                const listing = shop.find((l) => l.id === p.listingId);
                return (
                  <li key={p.id} className={cn("flex flex-wrap items-center gap-2 rounded-xl bg-muted/40 p-1.5", wrong === index && "ring-2 ring-destructive")}>
                    {p.listingId ? (
                      <span className="flex min-w-0 flex-1 items-center gap-1.5 truncate text-sm font-bold">
                        {listing ? nameOf(listing) : t("chattools.economy.somethingFromShop")}
                        {listing?.gachaOnly && <DicesIcon className="size-3.5 shrink-0 text-violet-500" aria-label={t("chattools.economy.gacha.only")} />}
                      </span>
                    ) : (
                      <span className="flex min-w-0 flex-1 items-center gap-1.5">
                        <Input inputMode="numeric" value={p.amount} aria-label={t("serversettings.economy.gacha.amount")} onChange={(e) => change(p.id, { amount: e.target.value })} className="h-8 w-24 rounded-lg tabular-nums" />
                        {currencies.length > 1 ? (
                          <select value={p.currencyId} aria-label={t("chattools.economy.currency")} onChange={(e) => change(p.id, { currencyId: e.target.value })} className={cn(SELECT, "h-8 w-auto")}>
                            {currencies.map((c) => (
                              <option key={c.id} value={c.id}>
                                {c.name}
                              </option>
                            ))}
                          </select>
                        ) : (
                          <span className="text-sm font-bold">{currencies[0]?.name}</span>
                        )}
                      </span>
                    )}
                    <label className="flex items-center gap-1 text-xs text-muted-foreground">
                      {t("serversettings.economy.gacha.weight")}
                      <Input inputMode="numeric" value={p.weight} onChange={(e) => change(p.id, { weight: e.target.value })} className="h-8 w-20 rounded-lg tabular-nums" />
                    </label>
                    <span className="w-16 text-right text-xs font-bold tabular-nums" title={t("serversettings.economy.gacha.chance")}>
                      {percent(chances[p.id] ?? 0)}
                    </span>
                    <Button type="button" variant="ghost" size="icon" className="size-8 rounded-lg" aria-label={t("serversettings.economy.gacha.removePrize")} onClick={() => set({ prizes: draft.prizes.filter((x) => x.id !== p.id) })}>
                      <XIcon />
                    </Button>
                  </li>
                );
              })}
            </ul>
            <select
              value=""
              aria-label={t("serversettings.economy.gacha.addPrize")}
              disabled={draft.prizes.length >= PRIZES_MAX}
              onChange={(e) => e.target.value && add(tier.id, e.target.value)}
              className={cn(SELECT, "h-8 text-xs font-normal text-muted-foreground")}
            >
              <option value="">{t("serversettings.economy.gacha.addPrize")}</option>
              <option value="currency">{t("serversettings.economy.gacha.currencyPrize")}</option>
              {listings.map((l) => (
                <option key={l.id} value={l.id}>
                  {nameOf(l)}
                  {l.gachaOnly ? ` · ${t("chattools.economy.gacha.only")}` : ""}
                </option>
              ))}
            </select>
          </div>
        );
      })}
    </div>
  );
}
