import {
  CheckIcon,
  CopyIcon,
  EllipsisIcon,
  EyeIcon,
  FingerprintIcon,
  KeyRoundIcon,
  Link2Icon,
  LoaderCircleIcon,
  PowerIcon,
  PowerOffIcon,
  SearchIcon,
  ShieldCheckIcon,
  ShieldIcon,
  ShieldOffIcon,
  UsersIcon, BuildingIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useRef, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { AccountFilter, type AccountSummary, type AccountTotals, type ListAccountsResponse } from "@/gen/fuwa/v1/admin_pb";
import { AccountKind } from "@/gen/fuwa/v1/types_pb";
import { AppBadge } from "@/components/AppBadge";
import { listAccounts, resetAccountPassword, run, updateAccount } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction, useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { Private } from "@/components/Private";
import { Count } from "@/components/motion";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { Segmented } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import { Textarea } from "@/components/ui/textarea";
import { type I18n, type Key, T, useI18n } from "@/i18n/react";
import { activeWhen } from "@/lib/devices";
import { displayName, formatDay, hueOf, toDate } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { usePrefs } from "@/lib/prefs";
import { copy, toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const PAGE = 50;
const REASON_MAX = 512;

/** "Today" and "Yesterday" in the middle of a sentence: "joined today". */
const midSentence = ({ t }: I18n, day: string) =>
  day === t("common.time.today") || day === t("common.time.yesterday") ? day.toLowerCase() : day;

type Pending = { account: AccountSummary; action: "turn-off" | "reset" } | null;

/**
 * Every account on the instance, for its admins: find one, make it an admin,
 * give it a new password, or turn it off so it can't sign in.
 */
export function Accounts({ instanceKey }: { instanceKey: string }) {
  const { t, number } = useI18n();
  const inst = useInstance(instanceKey);
  const myId = inst?.me?.id;
  const now = useNow(60_000);
  const developer = usePrefs((p) => p.developerMode);
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<AccountFilter>(AccountFilter.UNSPECIFIED);
  const [accounts, setAccounts] = useState<AccountSummary[] | null>(null);
  const [totals, setTotals] = useState<AccountTotals | null>(null);
  const [hasMore, setHasMore] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [pending, setPending] = useState<Pending>(null);
  const request = useRef(0);

  // Search as you type, once typing pauses.
  useEffect(() => {
    const timer = setTimeout(() => setSearch(query.trim()), 250);
    return () => clearTimeout(timer);
  }, [query]);

  function take(res: ListAccountsResponse, more: boolean) {
    setAccounts((current) => (more && current ? [...current, ...res.accounts] : res.accounts));
    setHasMore(res.hasMore);
    if (res.totals) setTotals(res.totals);
  }

  useEffect(() => {
    const n = ++request.current;
    setError(null);
    run(listAccounts(instanceKey, { query: search, filter, limit: PAGE })).then(
      (res) => n === request.current && take(res, false),
      (e: FuwaError) => n === request.current && setError(e.message),
    );
  }, [instanceKey, search, filter]);

  async function more() {
    const last = accounts?.at(-1)?.user?.id;
    if (!last) return;
    const n = request.current;
    setLoadingMore(true);
    try {
      const res = await run(listAccounts(instanceKey, { query: search, filter, beforeId: last, limit: PAGE }));
      if (n === request.current) take(res, true);
    } catch (e) {
      toast((e as FuwaError).message);
    } finally {
      setLoadingMore(false);
    }
  }

  /** Puts an account's new state on the page, and keeps the counts in step. */
  function replace(next: AccountSummary) {
    const id = next.user?.id;
    const prev = accounts?.find((a) => a.user?.id === id);
    if (prev && totals) {
      setTotals({
        ...totals,
        admins: totals.admins + BigInt(Number(next.admin) - Number(prev.admin)),
        disabled: totals.disabled + BigInt(Number(next.disabled) - Number(prev.disabled)),
      });
    }
    setAccounts((list) =>
      (list ?? [])
        .map((a) => (a.user?.id === id ? next : a))
        .filter((a) => (filter === AccountFilter.ADMINS ? a.admin : filter === AccountFilter.DISABLED ? a.disabled : true)),
    );
  }

  async function change(account: AccountSummary, patch: { admin?: boolean; disabled?: boolean }, done: string) {
    const id = account.user?.id ?? "";
    setBusy(id);
    try {
      replace(await run(updateAccount(instanceKey, id, patch)));
      toast(done);
    } catch (e) {
      toast((e as FuwaError).message);
    } finally {
      setBusy(null);
    }
  }

  /** A filter's name, with how many accounts it has once that's known. */
  const option = (bare: Key, counted: Key, n: bigint | undefined) => (n === undefined ? t(bare) : t(counted, { count: number(Number(n)) }));

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1 basis-56">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("instancesettings.accounts.search")}
            aria-label={t("instancesettings.accounts.searchLabel")}
            className="h-10 rounded-xl pl-9"
          />
        </div>
        <Segmented
          label={t("instancesettings.accounts.show")}
          value={filter}
          onChange={setFilter}
          options={[
            { value: AccountFilter.UNSPECIFIED, label: option("instancesettings.accounts.all", "instancesettings.accounts.allCount", totals?.all) },
            { value: AccountFilter.ADMINS, label: option("instancesettings.accounts.admins", "instancesettings.accounts.adminsCount", totals?.admins) },
            { value: AccountFilter.DISABLED, label: option("instancesettings.accounts.off", "instancesettings.accounts.offCount", totals?.disabled) },
          ]}
        />
      </div>

      {error ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>
      ) : !accounts ? (
        <div className="flex flex-col gap-1.5">
          {[0, 1, 2, 3].map((n) => (
            <div key={n} className="shimmer h-16 rounded-2xl" />
          ))}
        </div>
      ) : (
        <AccountList
          count={accounts.length}
          hasMore={hasMore}
          loadingMore={loadingMore}
          onMore={more}
          empty={t(emptyKey(search, filter))}
        >
          {accounts.map((a, n) => (
                <AccountRow
                  key={a.user?.id}
                  account={a}
                  index={n}
                  me={a.user?.id === myId}
                  now={now}
                  busy={busy === a.user?.id}
                  developer={developer}
                  onAdmin={(admin) =>
                    change(a, { admin }, t(admin ? "instancesettings.accounts.madeAdmin" : "instancesettings.accounts.unmadeAdmin", { name: displayName(a.user) }))
                  }
                  onTurnOn={() => change(a, { disabled: false }, t("instancesettings.accounts.turnedOn", { name: displayName(a.user) }))}
                  onTurnOff={() => setPending({ account: a, action: "turn-off" })}
                  onReset={() => setPending({ account: a, action: "reset" })}
                />
          ))}
        </AccountList>
      )}

      <Dialog open={!!pending} onOpenChange={(o) => !o && setPending(null)}>
        <DialogContent>
          {pending?.action === "turn-off" && (
            <TurnOff key={pending.account.user?.id} instanceKey={instanceKey} account={pending.account} onDone={(next) => {
                if (next) replace(next);
                setPending(null);
              }}
            />
          )}
          {pending?.action === "reset" && (
            <ResetPassword key={pending.account.user?.id} instanceKey={instanceKey} account={pending.account} onDone={() => setPending(null)} />
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

function AccountRow({
  account: a,
  index,
  me,
  now,
  busy,
  developer,
  onAdmin,
  onTurnOn,
  onTurnOff,
  onReset,
}: {
  account: AccountSummary;
  index: number;
  me: boolean;
  now: number;
  busy: boolean;
  developer: boolean;
  onAdmin: (admin: boolean) => void;
  onTurnOn: () => void;
  onTurnOff: () => void;
  onReset: () => void;
}) {
  const id = a.user?.id ?? "";
  const offDay = a.disabledAt ? formatDay(toDate(a.disabledAt)).toLowerCase() : "";
  return (
    <motion.li
      layout
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: Math.min(index, 14) * 0.02 } }}
      exit={{ opacity: 0, x: -24, transition: { duration: 0.2 } }}
      transition={SPRING}
      className={cn(
        "group flex flex-col gap-2 rounded-2xl border bg-background/40 p-2.5 pr-2 transition-colors hover:border-primary/30 hover:bg-muted/40",
        a.disabled && "border-destructive/30 bg-destructive/5 hover:border-destructive/40 hover:bg-destructive/10",
      )}
    >
      <div className="flex items-center gap-3">
        <UserAvatar user={a.user} className={cn("size-10 shrink-0 transition-[transform,filter] duration-300 group-hover:scale-105", a.disabled && "grayscale")} />
        <div className="min-w-0 flex-1">
          <p className="flex min-w-0 items-center gap-1.5">
            <span className={cn("name-tint truncate font-bold", a.disabled && "line-through decoration-destructive/60")} style={{ "--h": hueOf(id) } as CSSProperties}>
              {displayName(a.user)}
            </span>
            <AccountBadges account={a} me={me} />
          </p>
          <AccountFacts account={a} me={me} now={now} />
        </div>
        {(!me || developer) && (
          <AccountMenu account={a} me={me} busy={busy} developer={developer} onAdmin={onAdmin} onTurnOn={onTurnOn} onTurnOff={onTurnOff} onReset={onReset} />
        )}
      </div>
      <AnimatePresence initial={false} mode="popLayout">
        {a.disabled && (
          <motion.p
            {...SLIDE_IN}
            transition={SPRING}
            className="pl-[3.25rem] text-xs text-destructive/90"
          >
            <OffReason reason={a.disabledReason} day={offDay} />
          </motion.p>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** The account count, the rows, what shows with none, and "Show more". */
function AccountList({
  count,
  hasMore,
  loadingMore,
  onMore,
  empty,
  children,
}: {
  count: number;
  hasMore: boolean;
  loadingMore: boolean;
  onMore: () => void;
  empty: string;
  children: ReactNode;
}) {
  const { t } = useI18n();
  return (
    <>
      <p className="flex items-center gap-1.5 text-xs font-bold tracking-wide text-muted-foreground uppercase">
        <UsersIcon className="size-3.5" />{" "}
        <T k={hasMore ? "instancesettings.accounts.countMore" : "instancesettings.accounts.count"} values={{ count: <Count value={count} /> }} count={count} />
      </p>
      <ul className="flex flex-col gap-1.5">
        <AnimatePresence initial={false} mode="popLayout">
          {children}
        </AnimatePresence>
      </ul>
      <AnimatePresence>
        {count === 0 && (
          <motion.p initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0 }} className="py-8 text-center text-sm text-muted-foreground">
            {empty}
          </motion.p>
        )}
      </AnimatePresence>
      {hasMore && (
        <Button type="button" variant="outline" onClick={onMore} disabled={loadingMore} className="self-center rounded-xl">
          {loadingMore && <LoaderCircleIcon className="animate-spin" />} {t("instancesettings.accounts.showMore")}
        </Button>
      )}
    </>
  );
}

/** What the list says with no accounts in it: none match, none are off, or none at all. */
function emptyKey(search: string, filter: AccountFilter): Key {
  if (search) return "serversettings.shared.nobodyMatches";
  return filter === AccountFilter.DISABLED ? "instancesettings.accounts.noneOff" : "instancesettings.accounts.none";
}

/** Beside the name: you, admin, off, two-step sign-in, and how the account signs in. */
function AccountBadges({ account: a, me }: { account: AccountSummary; me: boolean }) {
  const { t } = useI18n();
  const kind = a.user?.kind;
  return (
    <>
      {me && <span className="shrink-0 rounded-full bg-muted px-1.5 py-px text-[0.65rem] font-bold text-muted-foreground uppercase">{t("serversettings.shared.you")}</span>}
      <AnimatePresence mode="popLayout" initial={false}>
        {a.admin && (
          <motion.span
            key="admin"
            initial={{ scale: 0, rotate: -40 }}
            animate={{ scale: 1, rotate: 0 }}
            exit={{ scale: 0, rotate: 40 }}
            transition={{ type: "spring", stiffness: 600, damping: 16 }}
            className="flex shrink-0 items-center gap-0.5 rounded-full bg-primary/15 px-1.5 py-px text-[0.65rem] font-bold text-primary uppercase"
            title={t("instancesettings.accounts.instanceAdmin")}
          >
            <ShieldIcon className="size-3" /> {t("instancesettings.accounts.admin")}
          </motion.span>
        )}
        {a.disabled && (
          <motion.span
            key="off"
            initial={{ scale: 0, opacity: 0 }}
            animate={{ scale: 1, opacity: 1 }}
            exit={{ scale: 0, opacity: 0 }}
            transition={{ type: "spring", stiffness: 600, damping: 18 }}
            className="flex shrink-0 items-center gap-0.5 rounded-full bg-destructive/15 px-1.5 py-px text-[0.65rem] font-bold text-destructive uppercase"
          >
            <PowerOffIcon className="size-3" /> {t("serversettings.shared.off")}
          </motion.span>
        )}
      </AnimatePresence>
      {a.twoFactor && (
        <span className="shrink-0 text-emerald-500" title={t("instancesettings.accounts.twoStepOn")}>
          <ShieldCheckIcon className="size-3.5" />
        </span>
      )}
      {kind === AccountKind.AGENT && <AppBadge agent />}
      {kind === AccountKind.LINKED && (
        <span className="shrink-0 text-muted-foreground" title={t("instancesettings.accounts.linked")}>
          <Link2Icon className="size-3.5" />
        </span>
      )}
      {kind === AccountKind.SSO && (
        <span className="shrink-0 text-muted-foreground" title={t("instancesettings.accounts.sso")}>
          <BuildingIcon className="size-3.5" />
        </span>
      )}
    </>
  );
}

/** Under the name: the username, when they joined and were last around, and what they have here. */
function AccountFacts({ account: a, me, now }: { account: AccountSummary; me: boolean; now: number }) {
  const lang = useI18n();
  const { t } = lang;
  const joined = midSentence(lang, formatDay(toDate(a.createdAt)));
  const seen = activeWhen(lang, toDate(a.lastSeenAt), now);
  const facts = [
    t("instancesettings.accounts.servers", { count: a.servers }),
    a.serversOwned ? t("instancesettings.accounts.owns", { count: a.serversOwned }) : null,
    t("instancesettings.accounts.devices", { count: a.sessions }),
  ].filter(Boolean);
  return (
    <>
      <p className="truncate text-xs text-muted-foreground">
        @{me ? <Private text={a.user?.username ?? ""} kind="name" className="align-top" /> : a.user?.username} ·{" "}
        {seen ? t("instancesettings.accounts.joinedSeen", { day: joined, when: seen }) : t("instancesettings.accounts.joinedNow", { day: joined })}
      </p>
      <p className="hidden truncate text-xs text-muted-foreground/80 sm:block">{facts.join(" · ")}</p>
    </>
  );
}

/** The row's "…" menu: admin actions on other accounts, and copying the id in developer mode. */
function AccountMenu({
  account: a,
  me,
  busy,
  developer,
  ...actions
}: {
  account: AccountSummary;
  me: boolean;
  busy: boolean;
  developer: boolean;
  onAdmin: (admin: boolean) => void;
  onTurnOn: () => void;
  onTurnOff: () => void;
  onReset: () => void;
}) {
  const { t } = useI18n();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={t("instancesettings.accounts.actionsFor", { name: displayName(a.user) })}
          disabled={busy}
          className="grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition hover:bg-muted hover:text-foreground data-[state=open]:bg-muted data-[state=open]:text-foreground"
        >
          {busy ? <LoaderCircleIcon className="size-4 animate-spin" /> : <EllipsisIcon className="size-4 transition-transform duration-300 group-hover:rotate-90" />}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-56">
        {!me && <AdminItems account={a} {...actions} />}
        {developer && !me && <DropdownMenuSeparator />}
        {developer && (
          <DropdownMenuItem onSelect={() => copy(t, a.user?.id ?? "", t("common.copy.accountId"))}>
            <FingerprintIcon /> {t("common.copyThing", { what: t("common.copy.accountId") })}
          </DropdownMenuItem>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** What an admin can do to someone else's account: admin or not, a new password, off or on. */
function AdminItems({
  account: a,
  onAdmin,
  onTurnOn,
  onTurnOff,
  onReset,
}: {
  account: AccountSummary;
  onAdmin: (admin: boolean) => void;
  onTurnOn: () => void;
  onTurnOff: () => void;
  onReset: () => void;
}) {
  const { t } = useI18n();
  const kind = a.user?.kind;
  return (
    <>
      {!a.admin && !a.disabled && kind !== AccountKind.AGENT && (
        <DropdownMenuItem onSelect={() => onAdmin(true)}>
          <ShieldIcon /> {t("instancesettings.accounts.makeAdmin")}
        </DropdownMenuItem>
      )}
      {a.admin && (
        <DropdownMenuItem onSelect={() => onAdmin(false)}>
          <ShieldOffIcon /> {t("instancesettings.accounts.removeAdmin")}
        </DropdownMenuItem>
      )}
      {kind === AccountKind.LOCAL && (
        <DropdownMenuItem onSelect={onReset}>
          <KeyRoundIcon /> {t("instancesettings.accounts.resetPassword")}
        </DropdownMenuItem>
      )}
      <DropdownMenuSeparator />
      {a.disabled ? (
        <DropdownMenuItem onSelect={onTurnOn}>
          <PowerIcon /> {t("instancesettings.accounts.turnBackOn")}
        </DropdownMenuItem>
      ) : (
        <DropdownMenuItem variant="destructive" onSelect={onTurnOff}>
          <PowerOffIcon /> {t("accountsettings.shared.turnOff")}
        </DropdownMenuItem>
      )}
    </>
  );
}

/** Why an account is off and since when, if the admin said. */
function OffReason({ reason, day }: { reason: string; day: string }) {
  const { t } = useI18n();
  if (!reason) return t("instancesettings.accounts.offNoReason", { date: day });
  return <T k="instancesettings.accounts.offReason" values={{ date: day, reason: <span className="italic">{reason}</span> }} />;
}

/** Turns an account off, with a reason only admins see. */
function TurnOff({ instanceKey, account, onDone }: { instanceKey: string; account: AccountSummary; onDone: (next?: AccountSummary) => void }) {
  const { t } = useI18n();
  const [reason, setReason] = useState("");
  const save = useAction(updateAccount);
  const power = useAnimationControls();
  const name = displayName(account.user);

  async function submit(e: FormEvent) {
    e.preventDefault();
    void power.start({ rotate: [0, 180], scale: [1, 0.8, 1], transition: { duration: 0.4 } });
    const next = await save.go(instanceKey, account.user?.id ?? "", { disabled: true, admin: account.admin ? false : undefined, reason: reason.trim() });
    if (!next) return;
    toast(t("instancesettings.accounts.turnedOff", { name }));
    onDone(next);
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <DialogHeader title={t("instancesettings.accounts.turnOffTitle", { name })} description={t("instancesettings.accounts.turnOffHint")} />
      <Who account={account} />
      {account.admin && <p className="rounded-xl bg-amber-500/10 px-3 py-2 text-sm text-amber-700 dark:text-amber-300">{t("instancesettings.accounts.turnOffAdmin")}</p>}
      <div className="flex flex-col gap-2">
        <Label htmlFor="turn-off-reason" className="flex items-center justify-between font-bold">
          {t("instancesettings.accounts.reason")} <span className="text-xs font-normal text-muted-foreground">{t("instancesettings.accounts.reasonHint")}</span>
        </Label>
        <Textarea id="turn-off-reason" rows={2} maxLength={REASON_MAX} value={reason} onChange={(e) => setReason(e.target.value)} className="rounded-xl" />
      </div>
      {save.error && <p className="text-sm text-destructive first-letter:uppercase">{save.error}</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" disabled={save.pending} onClick={() => onDone()} className="rounded-xl">
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="destructive" disabled={save.pending} className="rounded-xl font-bold">
          {save.pending ? (
            <LoaderCircleIcon className="animate-spin" />
          ) : (
            <motion.span animate={power} className="inline-flex">
              <PowerOffIcon />
            </motion.span>
          )}
          {t("accountsettings.shared.turnOff")}
        </Button>
      </div>
    </form>
  );
}

/** A new random password for someone locked out, shown once. */
function ResetPassword({ instanceKey, account, onDone }: { instanceKey: string; account: AccountSummary; onDone: () => void }) {
  const { t } = useI18n();
  const [twoFactor, setTwoFactor] = useState(false);
  const [password, setPassword] = useState<string | null>(null);
  const reset = useAction(resetAccountPassword);
  const name = displayName(account.user);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const next = await reset.go(instanceKey, account.user?.id ?? "", twoFactor);
    if (next) setPassword(next);
  }

  if (password) {
    return (
      <div className="flex flex-col gap-4">
        <DialogHeader title={t("instancesettings.accounts.newPasswordTitle", { name })} description={t("instancesettings.accounts.newPasswordHint")} />
        <NewPassword password={password} />
        {twoFactor && <p className="text-sm text-muted-foreground">{t("instancesettings.accounts.twoStepOff")}</p>}
        <Button type="button" onClick={onDone} className="btn self-end rounded-xl px-5 font-bold">
          {t("accountsettings.shared.done")}
        </Button>
      </div>
    );
  }

  return (
    <form onSubmit={submit} className="flex flex-col gap-4">
      <DialogHeader title={t("instancesettings.accounts.resetTitle", { name })} description={t("instancesettings.accounts.resetHint")} />
      <Who account={account} />
      {account.twoFactor && (
        <label className="flex cursor-pointer items-start gap-3 rounded-xl border p-3 transition-colors hover:border-primary/30">
          <Switch checked={twoFactor} onCheckedChange={setTwoFactor} className="mt-0.5" />
          <span className="flex flex-col gap-0.5">
            <span className="text-sm font-bold">{t("instancesettings.accounts.alsoTwoStep")}</span>
            <span className="text-xs text-muted-foreground">{t("instancesettings.accounts.alsoTwoStepHint")}</span>
          </span>
        </label>
      )}
      {reset.error && <p className="text-sm text-destructive first-letter:uppercase">{reset.error}</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" disabled={reset.pending} onClick={onDone} className="rounded-xl">
          {t("common.cancel")}
        </Button>
        <Button type="submit" disabled={reset.pending} className="btn group rounded-xl font-bold">
          {reset.pending ? <LoaderCircleIcon className="animate-spin" /> : <KeyRoundIcon className="transition-transform duration-300 group-hover:-rotate-45" />}
          {t("instancesettings.accounts.resetPassword")}
        </Button>
      </div>
    </form>
  );
}

/** The password, typed out letter by letter, behind a veil in streamer mode. */
function NewPassword({ password }: { password: string }) {
  const { t } = useI18n();
  const streaming = usePrefs((p) => p.streamer);
  const [revealed, setRevealed] = useState(false);
  const [copied, setCopied] = useState(false);
  const hidden = streaming && !revealed;
  return (
    <div className="flex items-center gap-2">
      <div className="relative min-w-0 flex-1">
        <code
          aria-label={t("instancesettings.accounts.newPassword")}
          className={cn(
            "block rounded-xl bg-muted px-3 py-3 text-center font-mono text-lg font-bold tracking-wider transition-[filter] duration-300",
            hidden && "blur-md select-none",
          )}
        >
          {[...password].map((ch, n) => (
            <motion.span
              key={n}
              initial={{ opacity: 0, y: 8, scale: 0.6 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              transition={{ type: "spring", stiffness: 600, damping: 20, delay: 0.1 + n * 0.025 }}
              className={cn("inline-block", ch === "-" && "text-muted-foreground")}
            >
              {ch}
            </motion.span>
          ))}
        </code>
        <AnimatePresence>
          {hidden && (
            <motion.button
              type="button"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              onClick={() => setRevealed(true)}
              className="absolute inset-0 flex items-center justify-center gap-2 rounded-xl bg-background/50 text-xs font-bold backdrop-blur-sm"
            >
              <EyeIcon className="size-4" /> {t("instancesettings.accounts.hiddenShow")}
            </motion.button>
          )}
        </AnimatePresence>
      </div>
      <Button
        type="button"
        variant="outline"
        size="icon"
        className="size-12 shrink-0 rounded-xl"
        aria-label={t("common.copyThing", { what: t("common.copy.password") })}
        onClick={() => {
          copy(t, password, t("common.copy.password"));
          setCopied(true);
          setTimeout(() => setCopied(false), 1600);
        }}
      >
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span key={String(copied)} initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0, rotate: 45 }} transition={SPRING}>
            {copied ? <CheckIcon className="size-4 text-emerald-500" /> : <CopyIcon className="size-4" />}
          </motion.span>
        </AnimatePresence>
      </Button>
    </div>
  );
}

function Who({ account }: { account: AccountSummary }) {
  return (
    <div className="flex items-center gap-3 rounded-2xl bg-muted/60 p-3">
      <UserAvatar user={account.user} className="size-10" />
      <span className="min-w-0 flex-1">
        <span className="block truncate font-bold">{displayName(account.user)}</span>
        <span className="block truncate text-xs text-muted-foreground">@{account.user?.username}</span>
      </span>
    </div>
  );
}
