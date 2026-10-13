import { CheckIcon, FlameIcon, GiftIcon, HistoryIcon, LoaderCircleIcon, SearchIcon, SendIcon, ShoppingBagIcon, StoreIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useId, useMemo, useState, type FormEvent } from "react";
import type { Balance, Currency, Member, ProfileItem, Role, ShopListing, User } from "@/gen/fuwa/v1/types_pb";
import { buy, claimDaily, closeShop, transfer, useEconomy, useShopView, type ShopTab } from "@/fuwa/economy";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useInstance, useMyMember } from "@/fuwa/hooks";
import { Amount, CurrencyIcon, LedgerList, ListingVisual, listingName } from "@/components/economy/shared";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { Private } from "@/components/Private";
import { Segmented } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { T, useI18n } from "@/i18n/react";
import { dailyOf, dailyRule, forSale, LISTING_CUSTOM, LISTING_ROLE, NOTE_MAX, num, owns, parseAmount, stockLeft } from "@/lib/economy";
import { displayName, formatLeft, isAgent, memberName, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * A server's shop (docs/economy.md), for any member: their balances with
 * the day's rewards to claim, what's for sale, their history, and sending
 * currencies the server lets people send. Opened from the wallet in the
 * channel sidebar (`openShop`), wherever that is.
 */
export function ShopHost({ open }: { open: boolean }) {
  const view = useShopView();
  const last = view.last;
  return (
    <Dialog open={open && view.open} onOpenChange={(o) => !o && closeShop()}>
      <DialogContent wide>{last && <ShopBody key={last.n} instanceKey={last.instanceKey} serverId={last.serverId} tab={last.tab} />}</DialogContent>
    </Dialog>
  );
}

function ShopBody({ instanceKey, serverId, tab: initial }: { instanceKey: string; serverId: string; tab: ShopTab }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const server = inst?.servers.find((s) => s.id === serverId);
  const { currencies, shop, wallet } = useEconomy(instanceKey, serverId, true);
  const [tab, setTab] = useState<ShopTab>(initial);
  const sendable = (currencies ?? []).filter((c) => c.transferable);
  const shown = tab === "send" && !sendable.length ? "shop" : tab;
  const balanceOf = (id: string) => wallet?.balances.find((b) => b.currencyId === id);
  // A new entry in your wallet reads your history again.
  const version = wallet?.balances.map((b) => `${b.currencyId}:${b.amount}`).join(",") ?? "";

  return (
    <div className="flex flex-col gap-4">
      <DialogHeader
        title={
          <span className="flex items-center gap-2.5">
            <motion.span
              initial={{ scale: 0.4, rotate: -25 }}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: "spring", stiffness: 520, damping: 14 }}
              className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary/15 text-primary"
            >
              <StoreIcon className="size-5" />
            </motion.span>
            {t("chattools.economy.shopTitle", { server: server?.name ?? "" })}
          </span>
        }
        description={t("chattools.economy.shopAbout")}
      />

      {!currencies || !wallet ? (
        <div className="grid gap-2 sm:grid-cols-2">
          {[0, 1].map((n) => (
            <div key={n} className="shimmer h-20 rounded-2xl" />
          ))}
        </div>
      ) : (
        <ul className="grid gap-2 sm:grid-cols-2">
          {currencies.map((c, n) => (
            <BalanceCard key={c.id} instanceKey={instanceKey} serverId={serverId} currency={c} balance={balanceOf(c.id)} index={n} />
          ))}
        </ul>
      )}

      <Segmented
        label={t("chattools.economy.shopTabs")}
        value={shown}
        onChange={setTab}
        className="self-start"
        options={[
          { value: "shop", label: t("chattools.economy.tab.shop"), icon: <ShoppingBagIcon className="size-4" /> },
          { value: "history", label: t("chattools.economy.tab.history"), icon: <HistoryIcon className="size-4" /> },
          ...(sendable.length ? [{ value: "send" as const, label: t("chattools.economy.tab.send"), icon: <SendIcon className="size-4" /> }] : []),
        ]}
      />

      <AnimatePresence mode="wait" initial={false}>
        <motion.div key={shown} initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={{ duration: 0.15 }}>
          {shown === "shop" && <Listings instanceKey={instanceKey} serverId={serverId} currencies={currencies ?? []} shop={shop} balanceOf={balanceOf} ownedItemIds={wallet?.ownedItemIds ?? []} />}
          {shown === "history" && (
            <LedgerList
              instanceKey={instanceKey}
              serverId={serverId}
              currencies={currencies ?? []}
              listings={shop}
              items={inst?.serverProfileItems[serverId]}
              version={version}
            />
          )}
          {shown === "send" && <SendForm instanceKey={instanceKey} serverId={serverId} currencies={sendable} balanceOf={balanceOf} />}
        </motion.div>
      </AnimatePresence>
    </div>
  );
}

/** One currency: what you have, and its daily reward to claim when it has one. */
function BalanceCard({ instanceKey, serverId, currency, balance, index }: { instanceKey: string; serverId: string; currency: Currency; balance: Balance | undefined; index: number }) {
  const lang = useI18n();
  const { t } = lang;
  const now = useNow(30_000);
  const rule = dailyRule(currency);
  const daily = rule ? dailyOf(balance, rule, now) : null;
  const [busy, setBusy] = useState(false);
  const [won, setWon] = useState<{ amount: number; n: number } | null>(null);

  async function claim() {
    setBusy(true);
    try {
      const res = await claimDaily(instanceKey, serverId, currency.id);
      setWon({ amount: num(res.entry?.amount), n: Date.now() });
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <motion.li
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: index * 0.04 } }}
      className="relative flex items-center gap-3 rounded-2xl border bg-background/50 p-3"
      title={currency.description || undefined}
    >
      <span className="grid size-11 shrink-0 place-items-center rounded-2xl bg-primary/10 text-2xl">
        <CurrencyIcon instanceKey={instanceKey} currency={currency} className="size-7 text-2xl" />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-xs font-bold text-muted-foreground uppercase">{currency.name}</span>
        <span className="block text-xl font-extrabold tabular-nums">
          <Count value={num(balance?.amount)} />
        </span>
        {daily && daily.streak > 0 && rule!.streakMaxDays > 0 && (
          <span className="flex items-center gap-1 text-xs font-bold text-orange-500">
            <FlameIcon className="size-3.5" />
            <T k="chattools.economy.streak" values={{ count: <Count value={daily.streak} /> }} count={daily.streak} />
          </span>
        )}
      </span>
      {daily && (
        <div className="relative flex shrink-0 flex-col items-end gap-0.5">
          <Button type="button" size="sm" className="group rounded-xl font-bold" disabled={!daily.ready || busy} onClick={() => void claim()}>
            {busy ? <LoaderCircleIcon className="animate-spin" /> : daily.ready ? <GiftIcon className="transition-transform group-hover:-rotate-12" /> : <CheckIcon />}
            {daily.ready ? t("chattools.economy.claim", { amount: daily.reward }) : t("chattools.economy.claimed")}
          </Button>
          {!daily.ready && <span className="text-[0.7rem] text-muted-foreground">{t("chattools.economy.nextClaim", { time: formatLeft(lang, daily.nextAt - now) })}</span>}
          <AnimatePresence>
            {won && (
              <motion.span
                key={won.n}
                initial={{ opacity: 0, y: 0, scale: 0.6 }}
                animate={{ opacity: [0, 1, 1, 0], y: -36, scale: 1.1 }}
                transition={{ duration: 1.4, ease: "easeOut" }}
                onAnimationComplete={() => setWon(null)}
                className="pointer-events-none absolute -top-2 right-2 font-extrabold text-emerald-500"
              >
                +{won.amount.toLocaleString()}
              </motion.span>
            )}
          </AnimatePresence>
        </div>
      )}
    </motion.li>
  );
}

/** What's for sale, in the shop's order. */
function Listings({
  instanceKey,
  serverId,
  currencies,
  shop,
  balanceOf,
  ownedItemIds,
}: {
  instanceKey: string;
  serverId: string;
  currencies: Currency[];
  shop: ShopListing[] | null;
  balanceOf: (currencyId: string) => Balance | undefined;
  ownedItemIds: string[];
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const member = useMyMember(instanceKey, serverId);
  const access = useAccess(instanceKey, serverId);
  const roles = useMemo(() => inst?.roles[serverId] ?? [], [inst?.roles, serverId]);
  const items = inst?.serverProfileItems[serverId];
  const now = useNow();
  // As the server says: timed out, or not agreed to the rules yet, you look but don't buy.
  const held = access.pending ? t("chattools.economy.agreeFirst") : timedOutUntil(member, now) ? t("chattools.economy.timedOut") : null;
  if (!shop)
    return (
      <div className="grid gap-2 sm:grid-cols-2">
        {[0, 1, 2, 3].map((n) => (
          <div key={n} className="shimmer h-28 rounded-2xl" />
        ))}
      </div>
    );
  const listed = forSale(shop, currencies);
  if (!listed.length)
    return (
      <div className="flex flex-col items-center gap-1 py-10 text-center">
        <motion.span animate={{ y: [0, -6, 0], rotate: [0, 6, -6, 0] }} transition={{ duration: 2.6, repeat: Infinity, repeatDelay: 0.8 }} className="text-4xl">
          🛍️
        </motion.span>
        <p className="font-bold">{t("chattools.economy.emptyShop")}</p>
        <p className="text-sm text-muted-foreground">{t("chattools.economy.emptyShopHint")}</p>
      </div>
    );
  return (
    <ul className="grid gap-2 sm:grid-cols-2">
      <AnimatePresence initial={false}>
        {listed.map((l, n) => (
          <ListingCard
            key={l.id}
            instanceKey={instanceKey}
            serverId={serverId}
            listing={l}
            index={n}
            currency={currencies.find((c) => c.id === l.currencyId)}
            balance={num(balanceOf(l.currencyId)?.amount)}
            owned={owns(l, member?.roleIds ?? [], ownedItemIds)}
            held={held}
            roles={roles}
            items={items}
            me={inst?.me}
          />
        ))}
      </AnimatePresence>
    </ul>
  );
}

function ListingCard({
  instanceKey,
  serverId,
  listing,
  index,
  currency,
  balance,
  owned,
  held,
  roles,
  items,
  me,
}: {
  instanceKey: string;
  serverId: string;
  listing: ShopListing;
  index: number;
  currency: Currency | undefined;
  balance: number;
  owned: boolean;
  held: string | null;
  roles: Role[];
  items: ProfileItem[] | undefined;
  me: User | null | undefined;
}) {
  const { t } = useI18n();
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [bought, setBought] = useState(false);
  const price = num(listing.price);
  const left = stockLeft(listing);
  const soldOut = left === 0;
  const short = balance < price;
  const name = listingName(listing, roles, items, t("chattools.economy.somethingFromShop"));
  const role = listing.kind === LISTING_ROLE ? roles.find((r) => r.id === listing.targetId) : undefined;
  const why = owned ? null : soldOut ? t("chattools.economy.soldOut") : held ?? (short ? t("chattools.economy.notEnough", { currency: currency?.name ?? "" }) : null);

  async function go() {
    setBusy(true);
    setError(null);
    try {
      await buy(instanceKey, serverId, listing);
      setAsking(false);
      setBought(true);
      setTimeout(() => setBought(false), 1800);
      toast(t("chattools.economy.bought", { name }));
    } catch (err) {
      setError((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, y: 10, scale: 0.97 }}
      animate={{ opacity: 1, y: 0, scale: 1, transition: { ...SPRING, delay: Math.min(index, 10) * 0.03 } }}
      exit={{ opacity: 0, scale: 0.9 }}
      whileHover={{ y: -2 }}
      className={cn("flex flex-col gap-2 rounded-2xl border bg-background/50 p-3 transition-colors", asking ? "border-primary/50" : "hover:border-primary/30")}
    >
      <div className="flex items-start gap-3">
        <ListingVisual listing={listing} roles={roles} items={items} me={me} className="size-14 shrink-0" />
        <div className="min-w-0 flex-1">
          <p className="truncate font-extrabold" style={role?.color !== undefined && role.color ? { color: `#${role.color.toString(16).padStart(6, "0")}` } : undefined}>
            {name}
          </p>
          <p className="text-[0.7rem] font-bold tracking-wide text-muted-foreground uppercase">
            {listing.kind === LISTING_ROLE ? t("chattools.economy.kindRole") : listing.kind === LISTING_CUSTOM ? t("chattools.economy.kindCustom") : t("chattools.economy.kindItem")}
          </p>
          {listing.description && <p className="mt-0.5 line-clamp-2 text-xs text-muted-foreground">{listing.description}</p>}
        </div>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Amount instanceKey={instanceKey} currency={currency} value={price} className="rounded-full bg-muted px-2.5 py-0.5 text-sm" />
        {left !== null && (
          <span className={cn("text-xs font-bold", soldOut ? "text-destructive" : "text-muted-foreground")}>
            {soldOut ? t("chattools.economy.soldOut") : <T k="chattools.economy.left" values={{ count: <Count value={left} /> }} count={left} />}
          </span>
        )}
        <span className="ml-auto">
          {owned ? (
            <span className="flex items-center gap-1 rounded-full bg-emerald-500/15 px-2.5 py-1 text-xs font-bold text-emerald-600 dark:text-emerald-400">
              <CheckIcon className="size-3.5" strokeWidth={3} /> {t("chattools.economy.owned")}
            </span>
          ) : !asking ? (
            <Button type="button" size="sm" className="rounded-xl font-bold" disabled={!!why} title={why ?? undefined} onClick={() => setAsking(true)}>
              <AnimatePresence mode="popLayout" initial={false}>
                <motion.span key={bought ? "done" : "buy"} initial={{ y: 8, opacity: 0 }} animate={{ y: 0, opacity: 1 }} exit={{ y: -8, opacity: 0 }} transition={SPRING} className="flex items-center gap-1.5">
                  {bought ? <CheckIcon /> : <ShoppingBagIcon />}
                  {bought ? t("chattools.economy.boughtShort") : t("chattools.economy.buy")}
                </motion.span>
              </AnimatePresence>
            </Button>
          ) : null}
        </span>
      </div>
      {why && !owned && <p className="text-xs text-muted-foreground">{why}</p>}
      <AnimatePresence mode="popLayout" initial={false}>
        {asking && (
          <motion.div key="ask" {...SLIDE_IN} transition={SPRING} className="flex flex-wrap items-center gap-2 rounded-xl bg-primary/10 p-2 text-sm">
            <span className="min-w-0 flex-1">
              <T k="chattools.economy.buyAsk" values={{ name: <b>{name}</b>, price: <Amount instanceKey={instanceKey} currency={currency} value={price} /> }} />
            </span>
            <Button type="button" size="sm" variant="ghost" className="rounded-xl" disabled={busy} onClick={() => setAsking(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="button" size="sm" className="rounded-xl font-bold" disabled={busy} onClick={() => void go()} autoFocus>
              {busy ? <LoaderCircleIcon className="animate-spin" /> : <CheckIcon />}
              {t("chattools.economy.buyConfirm")}
            </Button>
          </motion.div>
        )}
        {error && (
          <motion.p key="error" {...SLIDE_IN} transition={SPRING} role="alert" className="text-xs text-destructive first-letter:uppercase">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** Sending a currency to another member, with a note they see. */
function SendForm({ instanceKey, serverId, currencies, balanceOf }: { instanceKey: string; serverId: string; currencies: Currency[]; balanceOf: (currencyId: string) => Balance | undefined }) {
  const { t } = useI18n();
  const id = useId();
  const [currencyId, setCurrencyId] = useState(currencies[0]?.id ?? "");
  const [to, setTo] = useState<Member | null>(null);
  const [amount, setAmount] = useState("");
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const currency = currencies.find((c) => c.id === currencyId) ?? currencies[0];
  const have = num(balanceOf(currency?.id ?? "")?.amount);
  const value = parseAmount(amount, 1, Math.max(1, have));
  const ready = !!currency && !!to?.user && value !== null && have > 0;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!ready || busy) return;
    setBusy(true);
    setError(null);
    try {
      await transfer(instanceKey, serverId, currency.id, to.user!.id, value, note.trim());
      toast(t("chattools.economy.sent", { amount: value, currency: currency.name, name: memberName(to) }));
      setAmount("");
      setNote("");
    } catch (err) {
      setError((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-4">
      {currencies.length > 1 && (
        <Segmented label={t("chattools.economy.currency")} value={currency?.id ?? ""} onChange={setCurrencyId} options={currencies.map((c) => ({ value: c.id, label: c.name }))} className="self-start" />
      )}
      <div className="flex flex-col gap-2">
        <Label className="font-bold">{t("chattools.economy.sendTo")}</Label>
        <MemberPicker instanceKey={instanceKey} serverId={serverId} value={to} onChange={setTo} />
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor={`${id}-amount`} className="flex items-center justify-between font-bold">
          {t("chattools.economy.amount")}
          <span className="text-xs font-normal text-muted-foreground">
            <T k="chattools.economy.youHave" values={{ amount: <Amount instanceKey={instanceKey} currency={currency} value={have} /> }} />
          </span>
        </Label>
        <div className="flex gap-2">
          <Input id={`${id}-amount`} inputMode="numeric" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0" className="h-10 flex-1 rounded-xl font-bold tabular-nums" />
          <Button type="button" variant="outline" className="h-10 rounded-xl" disabled={have <= 0} onClick={() => setAmount(String(have))}>
            {t("chattools.economy.all")}
          </Button>
        </div>
        {amount && value === null && <p className="text-xs text-destructive">{have <= 0 ? t("chattools.economy.nothingToSend") : t("chattools.economy.badAmount", { max: have })}</p>}
      </div>
      <div className="flex flex-col gap-2">
        <Label htmlFor={`${id}-note`} className="flex items-center justify-between font-bold">
          {t("chattools.economy.note")}
          <span className="text-xs font-normal text-muted-foreground tabular-nums">
            <Count value={NOTE_MAX - note.length} />
          </span>
        </Label>
        <Textarea id={`${id}-note`} rows={2} maxLength={NOTE_MAX} value={note} onChange={(e) => setNote(e.target.value)} placeholder={t("chattools.economy.notePlaceholder")} className="rounded-xl" />
      </div>
      <AnimatePresence mode="popLayout" initial={false}>
        {error && (
          <motion.p key="error" {...SLIDE_IN} transition={SPRING} role="alert" className="text-sm text-destructive first-letter:uppercase">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <motion.div layout="position" transition={SPRING} className="flex justify-end">
        <Button type="submit" disabled={!ready || busy} className="group rounded-xl font-bold">
          {busy ? <LoaderCircleIcon className="animate-spin" /> : <SendIcon className="transition-transform group-hover:translate-x-0.5 group-hover:-rotate-12" />}
          {t("chattools.economy.send")}
        </Button>
      </motion.div>
    </form>
  );
}

/** At most this many people listed under the search. */
const PICKS = 6;

/** Picks another member of the server by name. */
export function MemberPicker({
  instanceKey,
  serverId,
  value,
  onChange,
  withSelf = false,
}: {
  instanceKey: string;
  serverId: string;
  value: Member | null;
  onChange: (member: Member | null) => void;
  withSelf?: boolean;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const [query, setQuery] = useState("");
  const me = inst?.me?.id;
  const q = query.trim().toLowerCase();
  const found = useMemo(
    () =>
      (inst?.members[serverId] ?? [])
        .filter((m) => m.user && (withSelf || m.user.id !== me))
        .filter((m) => !q || [m.nickname, m.user!.displayName, m.user!.username].some((s) => s?.toLowerCase().includes(q)))
        .slice(0, PICKS),
    [inst?.members, serverId, me, q, withSelf],
  );
  if (value?.user)
    return (
      <motion.div initial={{ opacity: 0, scale: 0.96 }} animate={{ opacity: 1, scale: 1 }} transition={SPRING} className="flex items-center gap-2 rounded-xl border bg-muted/40 p-2">
        <UserAvatar user={value.user} className="size-8" />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-bold">{memberName(value)}</span>
          <span className="block truncate text-xs text-muted-foreground">
            @<Private text={value.user.username} kind="name" />
          </span>
        </span>
        <Button type="button" variant="ghost" size="icon" aria-label={t("chattools.economy.pickSomeoneElse")} className="size-8 rounded-full" onClick={() => onChange(null)}>
          <XIcon className="size-4" />
        </Button>
      </motion.div>
    );
  return (
    <div className="flex flex-col gap-1">
      <div className="relative">
        <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
        <Input value={query} onChange={(e) => setQuery(e.target.value)} placeholder={t("chattools.economy.findMember")} aria-label={t("chattools.economy.findMember")} className="h-10 rounded-xl pl-9" />
      </div>
      <ul className="flex flex-col">
        {found.map((m) => (
          <li key={m.user!.id}>
            <button type="button" onClick={() => onChange(m)} className="flex w-full items-center gap-2 rounded-xl px-2 py-1.5 text-left text-sm transition-colors hover:bg-muted active:scale-[0.99]">
              <UserAvatar user={m.user} className="size-6" />
              <span className="truncate font-bold">{memberName(m)}</span>
              {isAgent(m.user) && <span className="rounded bg-primary/15 px-1 text-[0.6rem] font-extrabold text-primary">{t("chattools.economy.agent")}</span>}
              <span className="truncate text-xs text-muted-foreground">{displayName(m.user) !== memberName(m) ? displayName(m.user) : ""}</span>
            </button>
          </li>
        ))}
        {!found.length && <li className="px-2 py-1.5 text-sm text-muted-foreground">{t("chattools.economy.nobody")}</li>}
      </ul>
    </div>
  );
}
