import { LoaderCircleIcon, PackageIcon, ShieldIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import type { Currency, LedgerEntry, ProfileItem, Role, ShopListing, User } from "@/gen/fuwa/v1/types_pb";
import { listLedger } from "@/fuwa/economy";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { EmojiImage } from "@/components/EmojiImage";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { DecorationImage } from "@/components/ProfileDecoration";
import { ProfileEffect } from "@/components/ProfileEffect";
import { MiniCard } from "@/components/settings/account/EffectPicker";
import { Chips } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { T, useI18n } from "@/i18n/react";
import { FALLBACK_ICON, ledgerLine, LISTING_PROFILE_ITEM, LISTING_ROLE, num, withPage } from "@/lib/economy";
import { displayName, formatStamp, toDate } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { cssColor } from "@/lib/permissions";
import { ITEM_DECORATION, itemEffect } from "@/lib/profile-items";
import { cn } from "@/lib/utils";

/*
 * What the economy's screens share (docs/economy.md): a currency's icon and
 * amounts, what a listing sells, and the ledger as a list that loads its
 * pages as it goes.
 */

/**
 * A currency's icon: one of the server's emoji while it's there, else its
 * standard emoji, else a coin.
 */
export function CurrencyIcon({ instanceKey, currency, className }: { instanceKey: string; currency: Pick<Currency, "serverId" | "iconEmoji" | "iconEmojiId"> | undefined; className?: string }) {
  const inst = useInstance(instanceKey);
  const custom = currency?.iconEmojiId ? inst?.emojis[currency.serverId]?.find((e) => e.id === currency.iconEmojiId) : undefined;
  if (custom) return <EmojiImage emoji={custom} title={false} className={cn("size-4 shrink-0", className)} />;
  return (
    <span aria-hidden className={cn("inline-grid size-4 shrink-0 place-items-center leading-none", className)}>
      {currency?.iconEmoji || FALLBACK_ICON}
    </span>
  );
}

/** An amount with its currency's icon; it rolls when it changes. `signed` shows + for what came in. */
export function Amount({
  instanceKey,
  currency,
  value,
  signed = false,
  className,
  iconClassName,
}: {
  instanceKey: string;
  currency: Currency | undefined;
  value: number;
  signed?: boolean;
  className?: string;
  iconClassName?: string;
}) {
  return (
    <span className={cn("inline-flex items-center gap-1 font-bold tabular-nums", className)} title={currency?.name}>
      <CurrencyIcon instanceKey={instanceKey} currency={currency} className={iconClassName} />
      {signed && value > 0 && "+"}
      {signed && value < 0 && "−"}
      <Count value={signed ? Math.abs(value) : value} />
    </span>
  );
}

/** What a listing is called: its own name, or its role's or item's. */
export function listingName(listing: Pick<ShopListing, "name" | "kind" | "targetId"> | undefined, roles: readonly Role[], items: readonly ProfileItem[] | undefined, fallback: string): string {
  if (!listing) return fallback;
  if (listing.name) return listing.name;
  if (listing.kind === LISTING_ROLE) return roles.find((r) => r.id === listing.targetId)?.name ?? fallback;
  if (listing.kind === LISTING_PROFILE_ITEM) return items?.find((i) => i.id === listing.targetId)?.name ?? fallback;
  return fallback;
}

/** A picture of what a listing sells: the role's color, the item worn by you, or a parcel. */
export function ListingVisual({ listing, roles, items, me, className }: { listing: ShopListing; roles: readonly Role[]; items: readonly ProfileItem[] | undefined; me: User | undefined | null; className?: string }) {
  if (listing.kind === LISTING_ROLE) {
    const role = roles.find((r) => r.id === listing.targetId);
    const color = role?.color !== undefined ? cssColor(role.color) : undefined;
    return (
      <span className={cn("grid place-items-center rounded-2xl bg-muted", className)} style={color ? { backgroundColor: `${color}26`, color } : undefined}>
        <ShieldIcon className="size-1/2" />
      </span>
    );
  }
  if (listing.kind === LISTING_PROFILE_ITEM) {
    const item = items?.find((i) => i.id === listing.targetId);
    if (item?.kind === ITEM_DECORATION && me)
      return (
        <span className={cn("grid place-items-center rounded-2xl bg-muted", className)}>
          <span className="relative size-[60%]">
            <UserAvatar user={me} className="size-full" />
            <DecorationImage item={item} />
          </span>
        </span>
      );
    const spec = item ? itemEffect(item) : null;
    if (spec && me) return <EffectVisual spec={spec} me={me} className={className} />;
  }
  return (
    <span className={cn("grid place-items-center rounded-2xl bg-primary/10 text-primary", className)}>
      <PackageIcon className="size-1/2" />
    </span>
  );
}

/** An effect playing on a tiny card while pointed at. */
function EffectVisual({ spec, me, className }: { spec: NonNullable<ReturnType<typeof itemEffect>>; me: User; className?: string }) {
  const [hover, setHover] = useState(false);
  return (
    <span className={cn("grid place-items-center overflow-hidden rounded-2xl bg-muted p-1", className)} onPointerEnter={() => setHover(true)} onPointerLeave={() => setHover(false)}>
      <MiniCard userId={me.id} accent={-1} className="h-full w-auto">
        <ProfileEffect effect={spec} seed={me.id} play={hover} measure={false} replayOnHover={false} />
      </MiniCard>
    </span>
  );
}

/** A role's name in its color. */
export function RoleChip({ role, fallback }: { role: Role | undefined; fallback: string }) {
  return (
    <b className="inline-flex items-center gap-1" style={role?.color !== undefined ? { color: cssColor(role.color) } : undefined}>
      <span className="size-2 rounded-full bg-current" />
      {role?.name ?? fallback}
    </b>
  );
}

/**
 * The ledger, newest first, a page at a time: yours (no `userId`), someone's,
 * or everyone's ("*", with whose balance on each line). Filters by currency
 * (chips, when there's more than one) and, for everyone's, by kind.
 */
export function LedgerList({
  instanceKey,
  serverId,
  currencies,
  userId = "",
  kinds = [],
  listings,
  items,
  version = 0,
}: {
  instanceKey: string;
  serverId: string;
  currencies: readonly Currency[];
  userId?: string;
  kinds?: number[];
  /** The shop, to name what was bought. */
  listings: readonly ShopListing[] | null;
  items: readonly ProfileItem[] | undefined;
  /** Changes to read the first page again (a new entry came). */
  version?: string | number;
}) {
  const { t } = useI18n();
  const [currencyId, setCurrencyId] = useState("");
  const [entries, setEntries] = useState<LedgerEntry[] | null>(null);
  const [more, setMore] = useState(false);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The people the pages name, as the instance sent them with each page. */
  const [people, setPeople] = useState<Record<string, User>>({});
  const kindsKey = kinds.join(",");

  useEffect(() => {
    let live = true;
    setEntries(null);
    setError(null);
    setPeople({});
    listLedger(instanceKey, serverId, { currencyId, userId, kinds: kindsKey ? kindsKey.split(",").map(Number) : [] }).then(
      (res) => {
        if (!live) return;
        setEntries(res.entries);
        setPeople(byId({}, res.users));
        setMore(res.hasMore);
      },
      (err: FuwaError) => live && setError(err.message),
    );
    return () => {
      live = false;
    };
  }, [instanceKey, serverId, currencyId, userId, kindsKey, version]);

  async function next() {
    const last = entries?.at(-1);
    if (!last || loading) return;
    setLoading(true);
    try {
      const res = await listLedger(instanceKey, serverId, { currencyId, userId, kinds: kindsKey ? kindsKey.split(",").map(Number) : [], beforeId: last.id });
      setEntries((list) => withPage(list ?? [], res.entries));
      setPeople((known) => byId(known, res.users));
      setMore(res.hasMore);
    } catch (err) {
      setError((err as FuwaError).message);
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="flex flex-col gap-3">
      {currencies.length > 1 && (
        <Chips
          label={t("chattools.economy.currency")}
          value={currencyId}
          onChange={setCurrencyId}
          options={[{ value: "", label: t("chattools.economy.allCurrencies") }, ...currencies.map((c) => ({ value: c.id, label: c.name }))]}
        />
      )}
      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
      {!entries && !error && (
        <div className="flex flex-col gap-1.5">
          {[0, 1, 2, 3].map((n) => (
            <div key={n} className="shimmer h-12 rounded-xl" />
          ))}
        </div>
      )}
      {entries && !entries.length && (
        <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col items-center gap-1 py-8 text-center">
          <motion.span animate={{ y: [0, -5, 0], rotate: [0, -8, 8, 0] }} transition={{ duration: 2.6, repeat: Infinity, repeatDelay: 1 }} className="text-3xl">
            {FALLBACK_ICON}
          </motion.span>
          <p className="font-bold">{t("chattools.economy.noHistory")}</p>
          <p className="text-sm text-muted-foreground">{t("chattools.economy.noHistoryHint")}</p>
        </motion.div>
      )}
      {entries && entries.length > 0 && (
        <ol className="flex flex-col gap-1">
          <AnimatePresence initial={false}>
            {entries.map((entry, n) => (
              <LedgerRow
                key={entry.id}
                instanceKey={instanceKey}
                serverId={serverId}
                entry={entry}
                index={n}
                currencies={currencies}
                listings={listings}
                items={items}
                people={people}
                whose={userId === "*"}
              />
            ))}
          </AnimatePresence>
        </ol>
      )}
      {more && (
        <Button type="button" variant="ghost" className="self-center rounded-xl" disabled={loading} onClick={() => void next()}>
          {loading && <LoaderCircleIcon className="animate-spin" />}
          {t("chattools.economy.older")}
        </Button>
      )}
    </div>
  );
}

/** `known` with `users` added by id. */
function byId(known: Record<string, User>, users: readonly User[]): Record<string, User> {
  if (!users.length) return known;
  const next = { ...known };
  for (const user of users) next[user.id] = user;
  return next;
}

/** One change to a balance: what happened, how much, and what was left. */
function LedgerRow({
  instanceKey,
  serverId,
  entry,
  index,
  currencies,
  listings,
  items,
  people,
  whose,
}: {
  instanceKey: string;
  serverId: string;
  entry: LedgerEntry;
  index: number;
  currencies: readonly Currency[];
  listings: readonly ShopListing[] | null;
  items: readonly ProfileItem[] | undefined;
  people: Record<string, User>;
  whose: boolean;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const roles = useMemo(() => inst?.roles[serverId] ?? [], [inst?.roles, serverId]);
  // The ledger names people the app may not know (gone, or not loaded yet):
  // the page's own users first, then what the app has.
  const person = (id: string) => people[id] ?? inst?.users[id] ?? inst?.members[serverId]?.find((m) => m.user?.id === id)?.user;
  const currency = currencies.find((c) => c.id === entry.currencyId);
  const amount = num(entry.amount);
  const listing = listings?.find((l) => l.id === entry.listingId);
  const who = <b>{displayName(person(entry.counterpartId || entry.actorId))}</b>;
  const role = <RoleChip role={roles.find((r) => r.id === entry.roleId)} fallback={t("serversettings.audit.aRole")} />;
  const bought = <b>{listingName(listing, roles, items, t("chattools.economy.somethingFromShop"))}</b>;
  const owner = whose ? person(entry.userId) : undefined;
  const at = toDate(entry.createdAt);
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, x: -10 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 12) * 0.02 } }}
      exit={{ opacity: 0 }}
      className="flex items-center gap-3 rounded-xl px-2 py-1.5 transition-colors hover:bg-muted/50"
    >
      {whose ? (
        <UserAvatar user={owner} className="size-8 shrink-0" />
      ) : (
        <span className={cn("grid size-8 shrink-0 place-items-center rounded-xl", amount < 0 ? "bg-destructive/10" : "bg-emerald-500/10")}>
          <CurrencyIcon instanceKey={instanceKey} currency={currency} />
        </span>
      )}
      <span className="min-w-0 flex-1">
        <span className="block text-sm break-words">
          {whose && <b className="mr-1">{displayName(owner)}</b>}
          <T k={ledgerLine(entry)} values={{ who, role, listing: bought }} />
        </span>
        <span className="block truncate text-xs text-muted-foreground" title={formatStamp(at)}>
          {formatStamp(at)}
          {entry.note && <> · {entry.note}</>}
        </span>
      </span>
      <span className="flex shrink-0 flex-col items-end">
        <span className={cn("text-sm font-extrabold tabular-nums", amount < 0 ? "text-destructive" : "text-emerald-600 dark:text-emerald-400")}>
          {amount < 0 ? "−" : "+"}
          {Math.abs(amount).toLocaleString()}
        </span>
        <span className="text-[0.7rem] text-muted-foreground tabular-nums">
          <Amount instanceKey={instanceKey} currency={currency} value={num(entry.balanceAfter)} className="font-normal" iconClassName="size-3" />
        </span>
      </span>
    </motion.li>
  );
}
