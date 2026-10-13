import { ArrowRightLeftIcon, CoinsIcon, HandCoinsIcon, ShoppingBagIcon, SparklesIcon, TrendingDownIcon, TrendingUpIcon, UsersIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useState } from "react";
import type { EconomyOverview, PersonTotal } from "@/gen/fuwa/v1/economy_pb";
import { Permission, type Currency } from "@/gen/fuwa/v1/types_pb";
import { economyOverview, useEconomy } from "@/fuwa/economy";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useInstance } from "@/fuwa/hooks";
import { Amount, CurrencyIcon, LedgerList, listingName } from "@/components/economy/shared";
import { UserAvatar } from "@/components/Icons";
import { CountUp } from "@/components/motion";
import { openMenuDialog } from "@/components/menus/dialogs";
import { Chips, Segmented } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n/react";
import { actionLabel, chartColumns, LEDGER_KINDS, ms, num, PERIODS, share } from "@/lib/economy";
import { memberName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { has } from "@/lib/permissions";
import { cn } from "@/lib/utils";

/**
 * Server settings > Finances (docs/economy.md), for Manage Economy: one
 * currency over a period, as tiles, a chart of the days and ranked lists,
 * then the whole ledger. "Give to a role" is here for Grant Currency.
 */
export function Finances({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const { currencies, shop } = useEconomy(instanceKey, serverId, true);
  const [picked, setPicked] = useState("");
  const [days, setDays] = useState<number>(30);
  const [kind, setKind] = useState(0);
  const [overview, setOverview] = useState<EconomyOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const currency = currencies?.find((c) => c.id === picked) ?? currencies?.[0];
  const currencyId = currency?.id ?? "";

  useEffect(() => {
    if (!currencyId) return;
    let live = true;
    setError(null);
    economyOverview(instanceKey, serverId, currencyId, days).then(
      (o) => live && setOverview(o ?? null),
      (err: FuwaError) => live && setError(err.message),
    );
    return () => {
      live = false;
    };
  }, [instanceKey, serverId, currencyId, days]);

  if (currencies && !currencies.length)
    return (
      <div className="flex flex-col items-center gap-1 rounded-3xl border border-dashed py-12 text-center">
        <CoinsIcon className="size-8 text-muted-foreground" />
        <p className="font-extrabold">{t("serversettings.finances.none")}</p>
        <p className="max-w-sm text-sm text-muted-foreground">{t("serversettings.finances.noneHint")}</p>
      </div>
    );

  const shown = overview?.currencyId === currencyId ? overview : null;
  const items = inst?.serverProfileItems[serverId];
  const roles = inst?.roles[serverId] ?? [];

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-center gap-2">
        {currencies && currencies.length > 1 && (
          <Segmented label={t("chattools.economy.currency")} value={currencyId} onChange={setPicked} options={currencies.map((c) => ({ value: c.id, label: c.name }))} />
        )}
        <Segmented
          label={t("serversettings.finances.period")}
          value={days}
          onChange={setDays}
          options={PERIODS.map((d) => ({ value: d, label: t("serversettings.finances.days", { count: d }) }))}
        />
        {has(access, Permission.GRANT_CURRENCY) && (
          <Button type="button" variant="outline" className="ml-auto rounded-xl font-bold" onClick={() => openMenuDialog({ kind: "give", target: { instanceKey, serverId } })}>
            <HandCoinsIcon /> {t("serversettings.finances.giveRole")}
          </Button>
        )}
      </div>

      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}

      {!shown || !currency ? (
        !error && (
          <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
            {Array.from({ length: 8 }, (_, n) => (
              <div key={n} className="shimmer h-20 rounded-2xl" />
            ))}
          </div>
        )
      ) : (
        <>
          <Tiles instanceKey={instanceKey} currency={currency} overview={shown} />
          <DaysChart overview={shown} />
          <div className="grid gap-4 md:grid-cols-2">
            <Ranked instanceKey={instanceKey} serverId={serverId} currency={currency} title={t("serversettings.finances.topHolders")} people={shown.topHolders} />
            <Ranked instanceKey={instanceKey} serverId={serverId} currency={currency} title={t("serversettings.finances.topEarners")} people={shown.topEarners} />
            <Ranked instanceKey={instanceKey} serverId={serverId} currency={currency} title={t("serversettings.finances.topSpenders")} people={shown.topSpenders} />
            <Bars
              title={t("serversettings.finances.byAction")}
              rows={shown.earnedByAction.map((a) => ({ key: String(a.action), label: t(actionLabel(a.action)), value: num(a.amount) }))}
              instanceKey={instanceKey}
              currency={currency}
            />
            <Bars
              title={t("serversettings.finances.topListings")}
              rows={shown.topListings.map((l) => ({
                key: l.listingId,
                label: listingName(
                  shop?.find((x) => x.id === l.listingId),
                  roles,
                  items,
                  t("chattools.economy.somethingFromShop"),
                ),
                value: num(l.amount),
                note: t("serversettings.finances.bought", { count: num(l.count) }),
              }))}
              instanceKey={instanceKey}
              currency={currency}
            />
          </div>
        </>
      )}

      {currencies && (
        <section className="flex flex-col gap-3">
          <h3 className="text-xs font-extrabold tracking-wide text-muted-foreground uppercase">{t("serversettings.finances.ledger")}</h3>
          <Chips
            label={t("serversettings.finances.kind")}
            value={kind}
            onChange={setKind}
            options={[{ value: 0, label: t("serversettings.finances.allKinds") }, ...LEDGER_KINDS.map((k) => ({ value: k.kind, label: t(k.label) }))]}
          />
          <LedgerList instanceKey={instanceKey} serverId={serverId} currencies={currencies} userId="*" kinds={kind ? [kind] : []} listings={shop} items={items} />
        </section>
      )}
    </div>
  );
}

/** The period's totals, each counting up. */
function Tiles({ instanceKey, currency, overview: o }: { instanceKey: string; currency: Currency; overview: EconomyOverview }) {
  const { t } = useI18n();
  const tiles: { label: string; value: number; icon: LucideIcon; tint: string; plain?: boolean }[] = [
    { label: t("serversettings.finances.circulating"), value: num(o.circulating), icon: CoinsIcon, tint: "text-amber-500 bg-amber-500/10" },
    { label: t("serversettings.finances.holders"), value: o.holders, icon: UsersIcon, tint: "text-sky-500 bg-sky-500/10", plain: true },
    { label: t("serversettings.finances.earned"), value: num(o.earned), icon: TrendingUpIcon, tint: "text-emerald-500 bg-emerald-500/10" },
    { label: t("serversettings.finances.granted"), value: num(o.granted), icon: SparklesIcon, tint: "text-violet-500 bg-violet-500/10" },
    { label: t("serversettings.finances.taken"), value: num(o.taken), icon: TrendingDownIcon, tint: "text-destructive bg-destructive/10" },
    { label: t("serversettings.finances.spent"), value: num(o.spent), icon: ShoppingBagIcon, tint: "text-pink-500 bg-pink-500/10" },
    { label: t("serversettings.finances.transferred"), value: num(o.transferred), icon: ArrowRightLeftIcon, tint: "text-sky-500 bg-sky-500/10" },
    { label: t("serversettings.finances.purchases"), value: num(o.purchases), icon: ShoppingBagIcon, tint: "text-primary bg-primary/10", plain: true },
  ];
  return (
    <ul className="grid grid-cols-2 gap-2 sm:grid-cols-4">
      {tiles.map((tile, n) => (
        <motion.li
          key={tile.label}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: n * 0.03 } }}
          className="flex flex-col gap-1 rounded-2xl border bg-background/50 p-3"
        >
          <span className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground">
            <span className={cn("grid size-6 place-items-center rounded-lg", tile.tint)}>
              <tile.icon className="size-3.5" />
            </span>
            {tile.label}
          </span>
          <span className="flex items-center gap-1 text-xl font-extrabold">
            {!tile.plain && <CurrencyIcon instanceKey={instanceKey} currency={currency} className="size-5" />}
            <CountUp value={tile.value} />
          </span>
        </motion.li>
      ))}
    </ul>
  );
}

/** The chart's series: what came in by earning and grants, and what was spent. */
const SERIES = [
  { key: "earned", label: "serversettings.finances.earned", color: "var(--color-emerald-500)" },
  { key: "granted", label: "serversettings.finances.granted", color: "var(--color-violet-500)" },
  { key: "spent", label: "serversettings.finances.spent", color: "var(--color-pink-500)" },
] as const;

const W = 600;
const H = 160;

/** Each day of the period as three lines on one scale, with the day under the pointer told in full. */
function DaysChart({ overview }: { overview: EconomyOverview }) {
  const lang = useI18n();
  const { t } = lang;
  const [at, setAt] = useState<number | null>(null);
  const { max, columns } = useMemo(() => chartColumns(overview.days), [overview.days]);
  const n = columns.length;
  const x = (i: number) => (n <= 1 ? W / 2 : (i / (n - 1)) * W);
  const y = (f: number) => H - f * (H - 8);
  const day = at !== null ? overview.days[at] : undefined;
  return (
    <section className="flex flex-col gap-2 rounded-2xl border bg-background/50 p-4">
      <div className="flex flex-wrap items-center gap-3">
        <h3 className="flex-1 text-xs font-extrabold tracking-wide text-muted-foreground uppercase">{t("serversettings.finances.chart")}</h3>
        {SERIES.map((s) => (
          <span key={s.key} className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <span className="h-0.5 w-3 rounded-full" style={{ backgroundColor: s.color }} />
            {t(s.label)}
          </span>
        ))}
      </div>
      <div className="relative">
        <svg viewBox={`0 0 ${W} ${H}`} preserveAspectRatio="none" className="h-40 w-full overflow-visible" role="img" aria-label={t("serversettings.finances.chart")} onPointerLeave={() => setAt(null)}>
          {[0.5, 1].map((f) => (
            <line key={f} x1={0} x2={W} y1={y(f)} y2={y(f)} className="stroke-border" strokeDasharray="3 4" vectorEffect="non-scaling-stroke" />
          ))}
          <line x1={0} x2={W} y1={H} y2={H} className="stroke-border" vectorEffect="non-scaling-stroke" />
          {SERIES.map((s, k) => (
            <motion.polyline
              key={`${s.key}-${n}`}
              points={columns.map((c, i) => `${x(i)},${y(c[s.key])}`).join(" ")}
              fill="none"
              stroke={s.color}
              strokeWidth={2}
              strokeLinejoin="round"
              strokeLinecap="round"
              vectorEffect="non-scaling-stroke"
              initial={{ pathLength: 0, opacity: 0 }}
              animate={{ pathLength: 1, opacity: 1 }}
              transition={{ duration: 0.9, delay: k * 0.1, ease: [0.22, 1, 0.36, 1] }}
            />
          ))}
          {at !== null && <line x1={x(at)} x2={x(at)} y1={0} y2={H} className="stroke-muted-foreground/50" vectorEffect="non-scaling-stroke" />}
          {columns.map((_, i) => (
            <rect key={i} x={x(i) - W / Math.max(1, n) / 2} y={0} width={W / Math.max(1, n)} height={H} fill="transparent" onPointerEnter={() => setAt(i)} />
          ))}
        </svg>
        <span className="pointer-events-none absolute top-0 left-1 text-[0.65rem] text-muted-foreground tabular-nums">{lang.number(max)}</span>
        <AnimatePresence>
          {day && at !== null && (
            <motion.div
              key="tip"
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.12 }}
              className="pointer-events-none absolute top-2 z-10 rounded-xl border bg-popover px-3 py-2 text-xs shadow-lg"
              style={{ left: `${(x(at) / W) * 100}%`, transform: `translateX(${at > n / 2 ? "-105%" : "5%"})` }}
            >
              <p className="mb-1 font-bold">{lang.date(ms(day.day), { dateStyle: "medium", timeZone: "UTC" })}</p>
              {SERIES.map((s) => (
                <p key={s.key} className="flex items-center gap-2">
                  <span className="h-0.5 w-3 rounded-full" style={{ backgroundColor: s.color }} />
                  <span className="flex-1 text-muted-foreground">{t(s.label)}</span>
                  <b className="tabular-nums">{lang.number(num(day[s.key]))}</b>
                </p>
              ))}
            </motion.div>
          )}
        </AnimatePresence>
      </div>
      <div className="flex justify-between text-[0.65rem] text-muted-foreground">
        {overview.days.length > 0 && <span>{lang.date(ms(overview.days[0]!.day), { month: "short", day: "numeric", timeZone: "UTC" })}</span>}
        {overview.days.length > 1 && <span>{lang.date(ms(overview.days.at(-1)!.day), { month: "short", day: "numeric", timeZone: "UTC" })}</span>}
      </div>
    </section>
  );
}

/** Members ranked by an amount, with a bar for each one's share of the first. */
function Ranked({ instanceKey, serverId, currency, title, people }: { instanceKey: string; serverId: string; currency: Currency; title: string; people: PersonTotal[] }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const top = num(people[0]?.amount);
  return (
    <section className="flex flex-col gap-2 rounded-2xl border bg-background/50 p-4">
      <h3 className="text-xs font-extrabold tracking-wide text-muted-foreground uppercase">{title}</h3>
      {!people.length && <p className="text-sm text-muted-foreground">{t("serversettings.finances.nobody")}</p>}
      <ol className="flex flex-col gap-1.5">
        {people.map((p, n) => {
          const member = inst?.members[serverId]?.find((m) => m.user?.id === p.userId);
          const user = member?.user ?? inst?.users[p.userId];
          return (
            <motion.li key={p.userId} initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: n * 0.03 } }} className="relative flex items-center gap-2 overflow-hidden rounded-xl px-2 py-1">
              <motion.span
                className="absolute inset-y-0 left-0 rounded-xl bg-primary/8"
                initial={{ width: 0 }}
                animate={{ width: `${share(num(p.amount), top) * 100}%` }}
                transition={{ duration: 0.8, delay: n * 0.03, ease: [0.22, 1, 0.36, 1] }}
              />
              <span className="relative w-4 text-xs font-bold text-muted-foreground tabular-nums">{n + 1}</span>
              <UserAvatar user={user} className="relative size-6" />
              <span className="relative min-w-0 flex-1 truncate text-sm font-bold">{member ? memberName(member) : user ? (user.displayName || user.username) : t("common.someone")}</span>
              <Amount instanceKey={instanceKey} currency={currency} value={num(p.amount)} className="relative text-sm" iconClassName="size-3.5" />
            </motion.li>
          );
        })}
      </ol>
    </section>
  );
}

/** Things ranked by an amount, with bars. */
function Bars({ title, rows, instanceKey, currency }: { title: string; rows: { key: string; label: string; value: number; note?: string }[]; instanceKey: string; currency: Currency }) {
  const { t } = useI18n();
  const top = Math.max(0, ...rows.map((r) => r.value));
  return (
    <section className="flex flex-col gap-2 rounded-2xl border bg-background/50 p-4">
      <h3 className="text-xs font-extrabold tracking-wide text-muted-foreground uppercase">{title}</h3>
      {!rows.length && <p className="text-sm text-muted-foreground">{t("serversettings.finances.nothingYet")}</p>}
      <ol className="flex flex-col gap-2">
        {rows.map((r, n) => (
          <li key={r.key} className="flex flex-col gap-1">
            <span className="flex items-center gap-2 text-sm">
              <span className="min-w-0 flex-1 truncate font-bold">{r.label}</span>
              {r.note && <span className="text-xs text-muted-foreground">{r.note}</span>}
              <Amount instanceKey={instanceKey} currency={currency} value={r.value} iconClassName="size-3.5" />
            </span>
            <span className="h-1.5 overflow-hidden rounded-full bg-muted">
              <motion.span
                className="block h-full rounded-full bg-primary/70"
                initial={{ width: 0 }}
                animate={{ width: `${share(r.value, top) * 100}%` }}
                transition={{ duration: 0.8, delay: n * 0.04, ease: [0.22, 1, 0.36, 1] }}
              />
            </span>
          </li>
        ))}
      </ol>
    </section>
  );
}
