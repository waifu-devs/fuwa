import { ChevronDownIcon, CoinsIcon, DicesIcon, EyeOffIcon, InfinityIcon, PlusIcon, ShieldIcon, SparklesIcon, StoreIcon, Trash2Icon, TriangleAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useMemo, useState, type ReactNode } from "react";
import { Permission, type Currency, type ProfileItem, type Role, type ShopListing } from "@/gen/fuwa/v1/types_pb";
import { createCurrency, createListing, deleteCurrency, deleteListing, economyMessage, updateCurrency, updateListing, useEconomy } from "@/fuwa/economy";
import { useAccess, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { BannersSection } from "@/components/settings/server/GachaBanners";
import { EmojiPicker } from "@/components/EmojiPicker";
import { Amount, CurrencyIcon, ListingVisual, listingName } from "@/components/economy/shared";
import { confirmFirst } from "@/components/menus/dialogs";
import { Count, SwapText } from "@/components/motion";
import { Segmented } from "@/components/settings/account/common";
import { SaveBar, Toggle } from "@/components/settings/controls";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { T, useI18n, type Key } from "@/i18n/react";
import {
  ACTIONS,
  currencyChange,
  currencyDraft,
  DESCRIPTION_MAX,
  FALLBACK_ICON,
  LISTING_CUSTOM,
  LISTING_NAME_MAX,
  LISTING_PROFILE_ITEM,
  LISTING_ROLE,
  listingChange,
  listingDraft,
  NAME_MAX,
  num,
  STARTER,
  stockLeft,
  type CurrencyDraft,
  type ListingDraft,
  type ListingProblem,
  type RuleForm,
  type RuleProblem,
} from "@/lib/economy";
import { instanceHas } from "@/lib/compat";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { above, cssColor, harmless, has, sortRoles } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** What's open: a currency or listing by id, or a new one. */
type Open = { kind: "currency" | "listing"; id: string } | null;
const NEW = "new";

/**
 * Server settings > Economy (docs/economy.md), for Manage Economy: the
 * server's currencies (name, icon, line, whether members may send it, and
 * what earns it) and its shop's listings. One thing is edited at a time,
 * saved from the floating bar.
 */
export function Economy({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const { currencies, shop } = useEconomy(instanceKey, serverId, true);
  const gacha = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "gacha"));
  const [open, setOpen] = useState<Open>(null);
  const toggle = (next: NonNullable<Open>) => setOpen((o) => (o?.kind === next.kind && o.id === next.id ? null : next));
  const isOpen = (kind: "currency" | "listing", id: string) => open?.kind === kind && open.id === id;

  return (
    <div className="flex flex-col gap-6">
      <div className="relative overflow-hidden rounded-3xl border bg-gradient-to-br from-amber-500/10 via-transparent to-transparent p-5">
        <Coins />
        <div className="relative flex flex-wrap items-center gap-4">
          <motion.span
            animate={{ rotateY: [0, 180, 360] }}
            transition={{ duration: 2.4, repeat: Infinity, repeatDelay: 3, ease: "easeInOut" }}
            className="grid size-12 shrink-0 place-items-center rounded-2xl bg-amber-500/15 text-amber-500"
          >
            <CoinsIcon className="size-6" />
          </motion.span>
          <div className="min-w-0 flex-1 basis-60">
            <p className="font-extrabold">{t("serversettings.economy.title")}</p>
            <p className="text-sm text-muted-foreground">{t("serversettings.economy.intro")}</p>
          </div>
        </div>
      </div>

      <Section
        title={t("serversettings.economy.currencies")}
        count={currencies?.length ?? 0}
        action={
          <Button type="button" size="sm" className="btn rounded-xl font-bold" disabled={!currencies || isOpen("currency", NEW)} onClick={() => setOpen({ kind: "currency", id: NEW })}>
            <PlusIcon /> {t("serversettings.economy.newCurrency")}
          </Button>
        }
      >
        {!currencies ? (
          <Shimmer />
        ) : (
          <ul className="flex flex-col gap-2">
            <AnimatePresence initial={false}>
              {isOpen("currency", NEW) && (
                <CurrencyCard key="new" instanceKey={instanceKey} serverId={serverId} position={currencies.length} open onToggle={() => setOpen(null)} onCreated={(c) => setOpen({ kind: "currency", id: c.id })} />
              )}
              {currencies.map((c) => (
                <CurrencyCard key={c.id} instanceKey={instanceKey} serverId={serverId} currency={c} position={c.position} open={isOpen("currency", c.id)} onToggle={() => toggle({ kind: "currency", id: c.id })} />
              ))}
            </AnimatePresence>
            {!currencies.length && !isOpen("currency", NEW) && <Empty icon={FALLBACK_ICON} title={t("serversettings.economy.noCurrencies")} hint={t("serversettings.economy.noCurrenciesHint")} />}
          </ul>
        )}
      </Section>

      <Section
        title={t("serversettings.economy.shop")}
        count={shop?.length ?? 0}
        action={
          <Button type="button" size="sm" className="btn rounded-xl font-bold" disabled={!currencies?.length || !shop || isOpen("listing", NEW)} onClick={() => setOpen({ kind: "listing", id: NEW })}>
            <PlusIcon /> {t("serversettings.economy.newListing")}
          </Button>
        }
      >
        {!shop || !currencies ? (
          <Shimmer />
        ) : (
          <ul className="flex flex-col gap-2">
            <AnimatePresence initial={false}>
              {isOpen("listing", NEW) && (
                <ListingCard key="new" instanceKey={instanceKey} serverId={serverId} currencies={currencies} position={shop.length} open onToggle={() => setOpen(null)} onCreated={(l) => setOpen({ kind: "listing", id: l.id })} />
              )}
              {shop.map((l) => (
                <ListingCard key={l.id} instanceKey={instanceKey} serverId={serverId} listing={l} currencies={currencies} position={l.position} open={isOpen("listing", l.id)} onToggle={() => toggle({ kind: "listing", id: l.id })} />
              ))}
            </AnimatePresence>
            {!shop.length && !isOpen("listing", NEW) && (
              <Empty icon="🛍️" title={t("serversettings.economy.noListings")} hint={currencies.length ? t("serversettings.economy.noListingsHint") : t("serversettings.economy.currencyFirst")} />
            )}
          </ul>
        )}
      </Section>

      {gacha && <BannersSection instanceKey={instanceKey} serverId={serverId} currencies={currencies} shop={shop} />}
    </div>
  );
}

/** Coins drifting up behind the header. */
function Coins() {
  return (
    <div aria-hidden className="pointer-events-none absolute inset-0 overflow-hidden">
      {[12, 34, 58, 80].map((left, i) => (
        <motion.span
          key={left}
          className="absolute bottom-0 text-lg opacity-30"
          style={{ left: `${left}%` }}
          initial={{ y: 30, opacity: 0 }}
          animate={{ y: -110, opacity: [0, 0.35, 0] }}
          transition={{ duration: 5 + i, repeat: Infinity, delay: i * 1.2, ease: "easeOut" }}
        >
          {FALLBACK_ICON}
        </motion.span>
      ))}
    </div>
  );
}

export function Section({ title, count, action, children }: { title: string; count: number; action: ReactNode; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <h3 className="flex flex-1 items-center gap-2 text-xs font-extrabold tracking-wide text-muted-foreground uppercase">
          {title}
          <span className="rounded-full bg-muted px-2 py-0.5 tabular-nums">
            <Count value={count} />
          </span>
        </h3>
        {action}
      </div>
      {children}
    </section>
  );
}

export const Shimmer = () => (
  <div className="flex flex-col gap-2">
    {[0, 1].map((n) => (
      <div key={n} className="shimmer h-16 rounded-2xl" />
    ))}
  </div>
);

export function Empty({ icon, title, hint }: { icon: string; title: string; hint: string }) {
  return (
    <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="flex flex-col items-center gap-1 rounded-3xl border border-dashed py-8 text-center">
      <motion.span animate={{ y: [0, -6, 0], rotate: [0, 6, -6, 0] }} transition={{ duration: 2.6, repeat: Infinity, repeatDelay: 0.8 }} className="text-4xl">
        {icon}
      </motion.span>
      <p className="font-bold">{title}</p>
      <p className="max-w-sm text-sm text-muted-foreground">{hint}</p>
    </motion.div>
  );
}

/** A card that folds open to edit what it shows. */
export function Card({ open, summary, onToggle, children }: { open: boolean; summary: ReactNode; onToggle: () => void; children: ReactNode }) {
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, y: 10, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.9, filter: "blur(4px)" }}
      transition={SPRING}
      className={cn("rounded-2xl border bg-background/50 transition-colors", open ? "border-primary/40 shadow-lg shadow-primary/5" : "hover:border-primary/30")}
    >
      <button type="button" onClick={onToggle} aria-expanded={open} className="flex w-full items-center gap-3 p-3 text-left">
        {summary}
        <ChevronDownIcon className={cn("size-4 shrink-0 text-muted-foreground transition-transform duration-300", open && "rotate-180")} />
      </button>
      <AnimatePresence mode="popLayout" initial={false}>
        {open && (
          <motion.div {...SLIDE_IN} transition={SPRING}>
            <div className="flex flex-col gap-4 border-t p-4">{children}</div>
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

export function Field({ label, hint, children, wrong }: { label: string; hint?: ReactNode; children: ReactNode; wrong?: boolean }) {
  return (
    <label className="flex flex-col gap-1">
      <span className={cn("text-xs font-bold text-muted-foreground uppercase", wrong && "text-destructive")}>{label}</span>
      {children}
      {hint && <span className="text-xs text-muted-foreground">{hint}</span>}
    </label>
  );
}

// ── Currencies ───────────────────────────────────────────────────────────

/** Which actions a currency is earned by, in words. */
function earnSummary(t: (k: Key) => string, currency: Currency) {
  const names = ACTIONS.filter((a) => currency.earn.some((r) => r.action === a.action)).map((a) => t(a.label));
  return names.length ? names.join(" · ") : t("serversettings.economy.notEarned");
}

function CurrencyCard({
  instanceKey,
  serverId,
  currency,
  position,
  open,
  onToggle,
  onCreated,
}: {
  instanceKey: string;
  serverId: string;
  currency?: Currency;
  position: number;
  open: boolean;
  onToggle: () => void;
  onCreated?: (c: Currency) => void;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const server = inst?.servers.find((s) => s.id === serverId);
  const emojis = inst?.emojis[serverId];
  const [draft, setDraft] = useState<CurrencyDraft>(() => currencyDraft(currency));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The saved currency changed (here or by someone else): a draft with nothing changed follows it.
  const [base, setBase] = useState(currency);
  if (currency !== base) {
    setBase(currency);
    const was = currencyChange(base, draft);
    if ("change" in was && was.count === 0) setDraft(currencyDraft(currency));
  }
  const checked = currencyChange(currency, draft);
  const count = "change" in checked ? (currency ? checked.count : 1) : 1;
  const problem = "problem" in checked ? checked.problem : null;
  const set = (patch: Partial<CurrencyDraft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    setError(null);
  };
  const shown = { serverId, iconEmoji: draft.iconEmoji, iconEmojiId: draft.iconEmojiId };

  async function save() {
    if (!("change" in checked)) {
      setError(problem === "name" ? t("serversettings.economy.nameNeeded") : t("serversettings.economy.ruleWrong"));
      return;
    }
    setSaving(true);
    setError(null);
    try {
      if (currency) await updateCurrency(instanceKey, serverId, currency.id, checked.change);
      else {
        const made = await createCurrency(instanceKey, serverId, { ...checked.change, position });
        if (made) onCreated?.(made);
      }
    } catch (err) {
      setError(economyMessage(err));
    } finally {
      setSaving(false);
    }
  }

  function remove() {
    if (!currency) return;
    confirmFirst({
      title: t("serversettings.economy.deleteCurrencyTitle", { name: currency.name }),
      body: t("serversettings.economy.deleteCurrencyBody"),
      action: t("serversettings.economy.delete"),
      run: () => deleteCurrency(instanceKey, serverId, currency.id),
    });
  }

  return (
    <Card
      open={open}
      onToggle={onToggle}
      summary={
        <>
          <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-amber-500/10 text-xl">
            <CurrencyIcon instanceKey={instanceKey} currency={currency ?? shown} className="size-6 text-xl" />
          </span>
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="truncate font-bold">{currency ? <SwapText>{currency.name}</SwapText> : t("serversettings.economy.newCurrency")}</span>
              {currency?.transferable && <span className="rounded bg-sky-500/15 px-1 text-[0.6rem] font-extrabold tracking-wide text-sky-600 uppercase dark:text-sky-400">{t("serversettings.economy.sendable")}</span>}
            </span>
            <span className="block truncate text-xs text-muted-foreground">{currency ? earnSummary(t, currency) : t("serversettings.economy.newCurrencyHint")}</span>
          </span>
        </>
      }
    >
      <div className="flex flex-wrap items-start gap-3">
        <EmojiPicker
          emojis={emojis}
          server={server}
          placement="bottom-start"
          onPick={(e) => set(e.custom ? { iconEmojiId: e.custom.emoji.id, iconEmoji: draft.iconEmoji || FALLBACK_ICON } : { iconEmoji: e.text, iconEmojiId: "" })}
        >
          {(picking) => (
            <motion.button
              type="button"
              whileHover={{ scale: 1.08, rotate: -6 }}
              whileTap={{ scale: 0.9 }}
              aria-label={t("serversettings.economy.pickIcon")}
              title={t("serversettings.economy.pickIcon")}
              className={cn("grid size-14 shrink-0 place-items-center rounded-2xl border text-3xl transition-colors", picking ? "border-primary/60 bg-primary/10" : "hover:border-primary/40")}
            >
              <CurrencyIcon instanceKey={instanceKey} currency={shown} className="size-9 text-3xl" />
            </motion.button>
          )}
        </EmojiPicker>
        <div className="flex min-w-0 flex-1 basis-56 flex-col gap-3">
          <Field label={t("serversettings.economy.name")} wrong={problem === "name"}>
            <Input value={draft.name} maxLength={NAME_MAX} placeholder={t("serversettings.economy.namePlaceholder")} onChange={(e) => set({ name: e.target.value })} className="h-10 rounded-xl font-bold" />
          </Field>
          <Field label={t("serversettings.economy.description")}>
            <Textarea rows={2} value={draft.description} maxLength={DESCRIPTION_MAX} placeholder={t("serversettings.economy.descriptionPlaceholder")} onChange={(e) => set({ description: e.target.value })} className="rounded-xl" />
          </Field>
        </div>
      </div>
      <Toggle checked={draft.transferable} onChange={(transferable) => set({ transferable })} label={t("serversettings.economy.transferable")} hint={t("serversettings.economy.transferableHint")} />
      <EarnRules instanceKey={instanceKey} draft={draft.rules} problem={problem && problem !== "name" ? problem : null} onChange={(rules) => set({ rules })} />
      {currency && (
        <div className="flex justify-end">
          <Button type="button" variant="ghost" size="sm" className="rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive" onClick={remove}>
            <Trash2Icon /> {t("serversettings.economy.deleteCurrency")}
          </Button>
        </div>
      )}
      <SaveBar
        count={open ? count : 0}
        saving={saving}
        error={error}
        onSave={() => void save()}
        onDiscard={() => {
          setDraft(currencyDraft(currency));
          setError(null);
          if (!currency) onToggle();
        }}
      />
    </Card>
  );
}

/** One row per action a currency can reward: on or off, and its numbers. */
function EarnRules({
  instanceKey,
  draft,
  problem,
  onChange,
}: {
  instanceKey: string;
  draft: Record<number, RuleForm>;
  problem: RuleProblem | null;
  onChange: (rules: Record<number, RuleForm>) => void;
}) {
  const { t } = useI18n();
  const versions = useFuwa((s) => s.instances[instanceKey]?.node?.versions);
  // Actions the instance doesn't know yet stay out, unless a rule already has one.
  const actions = ACTIONS.filter((a) => !a.feature || instanceHas(versions, a.feature) || draft[a.action]?.on);
  const set = (action: number, patch: Partial<RuleForm>) => onChange({ ...draft, [action]: { ...draft[action]!, ...patch } });
  return (
    <div className="flex flex-col gap-2">
      <p className="text-xs font-bold text-muted-foreground uppercase">{t("serversettings.economy.earning")}</p>
      <ul className="flex flex-col gap-1.5">
        {actions.map((meta) => {
          const rule = draft[meta.action]!;
          const wrong = (field: keyof Omit<RuleForm, "on">) => problem?.action === meta.action && problem.field === field;
          return (
            <motion.li key={meta.action} layout="position" transition={SPRING} className={cn("rounded-xl border p-2.5 transition-colors", rule.on ? "border-primary/30 bg-primary/5" : "bg-background/40")}>
              <label className="flex cursor-pointer items-center gap-3">
                <span className="min-w-0 flex-1">
                  <span className="block text-sm font-bold">{t(meta.label)}</span>
                  <span className="block text-xs text-muted-foreground">{t(meta.about)}</span>
                </span>
                <Switch
                  checked={rule.on}
                  onCheckedChange={(on) => set(meta.action, on && !rule.amount ? { on, ...STARTER[meta.action] } : { on })}
                  aria-label={t(meta.label)}
                />
              </label>
              <AnimatePresence mode="popLayout" initial={false}>
                {rule.on && (
                  <motion.div {...SLIDE_IN} transition={SPRING} className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-4">
                    <Field label={t("serversettings.economy.rule.amount")} wrong={wrong("amount")}>
                      <Input inputMode="numeric" value={rule.amount} onChange={(e) => set(meta.action, { amount: e.target.value })} className="h-9 rounded-lg tabular-nums" />
                    </Field>
                    {meta.cooldown && (
                      <Field label={t("serversettings.economy.rule.cooldown")} wrong={wrong("cooldown")}>
                        <Input inputMode="numeric" value={rule.cooldown} placeholder="0" onChange={(e) => set(meta.action, { cooldown: e.target.value })} className="h-9 rounded-lg tabular-nums" />
                      </Field>
                    )}
                    <Field label={t("serversettings.economy.rule.dailyCap")} wrong={wrong("dailyCap")}>
                      <Input inputMode="numeric" value={rule.dailyCap} placeholder={t("serversettings.economy.rule.noCap")} onChange={(e) => set(meta.action, { dailyCap: e.target.value })} className="h-9 rounded-lg tabular-nums" />
                    </Field>
                    {meta.games && (
                      <div className="col-span-full">
                        <Field label={t("serversettings.economy.rule.games")} hint={t("serversettings.economy.rule.gamesHint")} wrong={wrong("games")}>
                          <Textarea
                            value={rule.games}
                            rows={3}
                            placeholder={t("serversettings.economy.rule.gamesAny")}
                            onChange={(e) => set(meta.action, { games: e.target.value })}
                            className="min-h-0 resize-y rounded-lg"
                          />
                        </Field>
                      </div>
                    )}
                    {meta.streak && (
                      <>
                        <Field label={t("serversettings.economy.rule.streakBonus")} wrong={wrong("streakBonus")}>
                          <Input inputMode="numeric" value={rule.streakBonus} placeholder="0" onChange={(e) => set(meta.action, { streakBonus: e.target.value })} className="h-9 rounded-lg tabular-nums" />
                        </Field>
                        <Field label={t("serversettings.economy.rule.streakDays")} wrong={wrong("streakMaxDays")}>
                          <Input inputMode="numeric" value={rule.streakMaxDays} placeholder="0" onChange={(e) => set(meta.action, { streakMaxDays: e.target.value })} className="h-9 rounded-lg tabular-nums" />
                        </Field>
                      </>
                    )}
                    {meta.streak && (
                      <p className="col-span-full text-xs text-muted-foreground">{t("serversettings.economy.rule.streakHint")}</p>
                    )}
                    {meta.cooldown && <p className="col-span-full text-xs text-muted-foreground">{t("serversettings.economy.rule.cooldownHint")}</p>}
                  </motion.div>
                )}
              </AnimatePresence>
            </motion.li>
          );
        })}
      </ul>
    </div>
  );
}

// ── The shop ─────────────────────────────────────────────────────────────

const LISTING_PROBLEM: Record<ListingProblem, Key> = {
  currency: "serversettings.economy.problem.currency",
  target: "serversettings.economy.problem.target",
  name: "serversettings.economy.problem.name",
  price: "serversettings.economy.problem.price",
  stock: "serversettings.economy.problem.stock",
  position: "serversettings.economy.problem.position",
};

function ListingCard({
  instanceKey,
  serverId,
  listing,
  currencies,
  position,
  open,
  onToggle,
  onCreated,
}: {
  instanceKey: string;
  serverId: string;
  listing?: ShopListing;
  currencies: Currency[];
  position: number;
  open: boolean;
  onToggle: () => void;
  onCreated?: (l: ShopListing) => void;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const roles = useMemo(() => inst?.roles[serverId] ?? [], [inst?.roles, serverId]);
  const items = inst?.serverProfileItems[serverId];
  const gacha = instanceHas(inst?.node?.versions, "gacha");
  const [draft, setDraft] = useState<ListingDraft>(() => listingDraft(listing, currencies[0]?.id, position));
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [base, setBase] = useState(listing);
  if (listing !== base) {
    setBase(listing);
    const was = listingChange(base, draft);
    if ("count" in was && was.count === 0) setDraft(listingDraft(listing, currencies[0]?.id, position));
  }
  const checked = listingChange(listing, draft);
  const count = "count" in checked ? (listing ? checked.count : 1) : 1;
  const problem = "problem" in checked ? checked.problem : null;
  const set = (patch: Partial<ListingDraft>) => {
    setDraft((d) => ({ ...d, ...patch }));
    setError(null);
  };
  const currency = currencies.find((c) => c.id === (listing?.currencyId ?? draft.currencyId));
  const name = listing ? listingName(listing, roles, items, t("chattools.economy.somethingFromShop")) : t("serversettings.economy.newListing");
  const left = listing ? stockLeft(listing) : null;

  async function save() {
    if (!("count" in checked)) {
      setError(t(LISTING_PROBLEM[checked.problem]));
      return;
    }
    setSaving(true);
    setError(null);
    try {
      if (listing) await updateListing(instanceKey, serverId, listing.id, checked.change);
      else {
        const made = await createListing(instanceKey, serverId, checked.create);
        if (made) onCreated?.(made);
      }
    } catch (err) {
      setError(economyMessage(err));
    } finally {
      setSaving(false);
    }
  }

  function remove() {
    if (!listing) return;
    confirmFirst({
      title: t("serversettings.economy.deleteListingTitle", { name }),
      body: t("serversettings.economy.deleteListingBody"),
      action: t("serversettings.economy.delete"),
      run: () => deleteListing(instanceKey, serverId, listing.id),
    });
  }

  return (
    <Card
      open={open}
      onToggle={onToggle}
      summary={
        <>
          {listing ? (
            <ListingVisual listing={listing} roles={roles} items={items} me={inst?.me} className="size-10 shrink-0" />
          ) : (
            <span className="grid size-10 shrink-0 place-items-center rounded-2xl bg-primary/10 text-primary">
              <StoreIcon className="size-5" />
            </span>
          )}
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5">
              <span className="truncate font-bold">{name}</span>
              {listing && !listing.available && (
                <span className="flex items-center gap-0.5 rounded bg-muted px-1 text-[0.6rem] font-extrabold tracking-wide text-muted-foreground uppercase">
                  <EyeOffIcon className="size-2.5" /> {t("serversettings.economy.hidden")}
                </span>
              )}
              {listing?.gachaOnly && (
                <span className="flex items-center gap-0.5 rounded bg-violet-500/15 px-1 text-[0.6rem] font-extrabold tracking-wide text-violet-600 uppercase dark:text-violet-400">
                  <DicesIcon className="size-2.5" /> {t("chattools.economy.gacha.only")}
                </span>
              )}
            </span>
            {listing && (
              <span className="flex flex-wrap items-center gap-x-2 text-xs text-muted-foreground">
                <Amount instanceKey={instanceKey} currency={currency} value={num(listing.price)} iconClassName="size-3" />
                <span>
                  <T k="serversettings.economy.sold" values={{ count: <Count value={num(listing.sold)} /> }} count={num(listing.sold)} />
                </span>
                {left !== null && (
                  <span>
                    <T k="chattools.economy.left" values={{ count: <Count value={left} /> }} count={left} />
                  </span>
                )}
              </span>
            )}
          </span>
        </>
      }
    >
      {!listing && (
        <>
          <Segmented
            label={t("serversettings.economy.kind")}
            value={draft.kind}
            onChange={(kind) => set({ kind, targetId: "" })}
            className="self-start"
            options={[
              { value: LISTING_ROLE, label: t("chattools.economy.kindRole"), icon: <ShieldIcon className="size-4" /> },
              { value: LISTING_PROFILE_ITEM, label: t("chattools.economy.kindItem"), icon: <SparklesIcon className="size-4" /> },
              { value: LISTING_CUSTOM, label: t("chattools.economy.kindCustom"), icon: <StoreIcon className="size-4" /> },
            ]}
          />
          {draft.kind === LISTING_ROLE && <RoleTarget roles={roles} serverId={serverId} access={access} value={draft.targetId} onChange={(targetId) => set({ targetId })} wrong={problem === "target"} />}
          {draft.kind === LISTING_PROFILE_ITEM && <ItemTarget items={items ?? []} value={draft.targetId} onChange={(targetId) => set({ targetId })} wrong={problem === "target"} />}
          {currencies.length > 1 && (
            <Field label={t("chattools.economy.currency")}>
              <Segmented label={t("chattools.economy.currency")} value={draft.currencyId} onChange={(currencyId) => set({ currencyId })} options={currencies.map((c) => ({ value: c.id, label: c.name }))} className="self-start" />
            </Field>
          )}
        </>
      )}
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("serversettings.economy.listingName")} wrong={problem === "name"} hint={draft.kind === LISTING_CUSTOM ? undefined : t("serversettings.economy.listingNameHint")}>
          <Input value={draft.name} maxLength={LISTING_NAME_MAX} onChange={(e) => set({ name: e.target.value })} className="h-10 rounded-xl font-bold" />
        </Field>
        <Field label={t("serversettings.economy.price")} wrong={problem === "price"}>
          <div className="relative">
            <span className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2">
              <CurrencyIcon instanceKey={instanceKey} currency={currency} />
            </span>
            <Input inputMode="numeric" value={draft.price} onChange={(e) => set({ price: e.target.value })} className="h-10 rounded-xl pl-9 font-bold tabular-nums" />
          </div>
        </Field>
      </div>
      <Field label={t("serversettings.economy.description")}>
        <Textarea rows={2} value={draft.description} maxLength={DESCRIPTION_MAX} onChange={(e) => set({ description: e.target.value })} placeholder={t("serversettings.economy.listingDescriptionPlaceholder")} className="rounded-xl" />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("serversettings.economy.stock")} wrong={problem === "stock"} hint={t("serversettings.economy.stockHint")}>
          <div className="flex items-center gap-2">
            <Input inputMode="numeric" value={draft.stock} placeholder={t("settings.controls.noLimit")} onChange={(e) => set({ stock: e.target.value })} className="h-10 flex-1 rounded-xl tabular-nums" />
            <Button type="button" variant={draft.stock === "" ? "secondary" : "outline"} size="icon" className="size-10 rounded-xl" title={t("settings.controls.noLimit")} aria-label={t("settings.controls.noLimit")} onClick={() => set({ stock: "" })}>
              <InfinityIcon />
            </Button>
          </div>
        </Field>
        <Field label={t("serversettings.economy.position")} wrong={problem === "position"} hint={t("serversettings.economy.positionHint")}>
          <Input inputMode="numeric" value={draft.position} onChange={(e) => set({ position: e.target.value })} className="h-10 rounded-xl tabular-nums" />
        </Field>
      </div>
      <Toggle checked={draft.available} onChange={(available) => set({ available })} label={t("serversettings.economy.available")} hint={t("serversettings.economy.availableHint")} />
      {(gacha || draft.gachaOnly) && (
        <Toggle checked={draft.gachaOnly} onChange={(gachaOnly) => set({ gachaOnly })} label={t("serversettings.economy.gachaOnly")} hint={t("serversettings.economy.gachaOnlyHint")} />
      )}
      {draft.kind === LISTING_PROFILE_ITEM && <p className="text-xs text-muted-foreground">{t("serversettings.economy.itemLockHint")}</p>}
      {listing && (
        <div className="flex justify-end">
          <Button type="button" variant="ghost" size="sm" className="rounded-xl text-destructive hover:bg-destructive/10 hover:text-destructive" onClick={remove}>
            <Trash2Icon /> {t("serversettings.economy.deleteListing")}
          </Button>
        </div>
      )}
      <SaveBar
        count={open ? count : 0}
        saving={saving}
        error={error}
        onSave={() => void save()}
        onDiscard={() => {
          setDraft(listingDraft(listing, currencies[0]?.id, position));
          setError(null);
          if (!listing) onToggle();
        }}
      />
    </Card>
  );
}

/**
 * The roles a listing may sell, as the server allows: ones that carry no
 * powers (like onboarding's), below the manager's own, with Manage Roles.
 */
function RoleTarget({ roles, serverId, access, value, onChange, wrong }: { roles: Role[]; serverId: string; access: ReturnType<typeof useAccess>; value: string; onChange: (id: string) => void; wrong: boolean }) {
  const { t } = useI18n();
  if (!has(access, Permission.MANAGE_ROLES))
    return (
      <p className="flex items-center gap-2 rounded-xl bg-amber-500/10 p-3 text-sm text-amber-700 dark:text-amber-400">
        <TriangleAlertIcon className="size-4 shrink-0" /> {t("serversettings.economy.needManageRoles")}
      </p>
    );
  const offered = sortRoles(roles).filter((r) => r.id !== serverId);
  return (
    <Field label={t("serversettings.economy.role")} wrong={wrong} hint={t("serversettings.economy.roleHint")}>
      <div role="radiogroup" className="flex flex-wrap gap-1.5">
        {offered.map((r) => {
          const ok = above(access, r.position) && harmless(r);
          const active = r.id === value;
          return (
            <motion.button
              key={r.id}
              type="button"
              role="radio"
              aria-checked={active}
              disabled={!ok}
              title={ok ? r.name : harmless(r) ? t("serversettings.onboarding.aboveYou") : t("serversettings.economy.rolePowers")}
              whileTap={{ scale: 0.94 }}
              onClick={() => onChange(r.id)}
              className={cn(
                "flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs font-bold transition-colors disabled:cursor-not-allowed disabled:opacity-40",
                active ? "border-primary bg-primary/12 text-foreground" : "text-muted-foreground hover:border-primary/30",
              )}
            >
              <span className="size-2 rounded-full" style={{ backgroundColor: r.color ? cssColor(r.color) : "currentColor" }} />
              {r.name}
            </motion.button>
          );
        })}
        {!offered.length && <span className="text-sm text-muted-foreground">{t("serversettings.economy.noRoles")}</span>}
      </div>
    </Field>
  );
}

function ItemTarget({ items, value, onChange, wrong }: { items: ProfileItem[]; value: string; onChange: (id: string) => void; wrong: boolean }) {
  const { t } = useI18n();
  return (
    <Field label={t("serversettings.economy.item")} wrong={wrong}>
      <div role="radiogroup" className="flex flex-wrap gap-1.5">
        {items.map((i) => (
          <motion.button
            key={i.id}
            type="button"
            role="radio"
            aria-checked={i.id === value}
            whileTap={{ scale: 0.94 }}
            onClick={() => onChange(i.id)}
            className={cn("rounded-full border px-3 py-1 text-xs font-bold transition-colors", i.id === value ? "border-primary bg-primary/12" : "text-muted-foreground hover:border-primary/30")}
          >
            {i.name}
          </motion.button>
        ))}
        {!items.length && <span className="text-sm text-muted-foreground">{t("serversettings.economy.noItems")}</span>}
      </div>
    </Field>
  );
}

