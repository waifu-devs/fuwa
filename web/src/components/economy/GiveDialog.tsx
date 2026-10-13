import { HandCoinsIcon, LoaderCircleIcon, MinusIcon, PlusIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useId, useState, type FormEvent } from "react";
import type { User } from "@/gen/fuwa/v1/types_pb";
import { grant, useEconomy, walletOf } from "@/fuwa/economy";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { Amount } from "@/components/economy/shared";
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
import { AMOUNT_MAX, NOTE_MAX, num, parseAmount } from "@/lib/economy";
import { displayName } from "@/lib/format";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { cssColor, sortRoles } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** Who's being given currency: one person, or everyone with a role (picked in the dialog). */
export type GiveTarget = { instanceKey: string; serverId: string; user?: User };

/**
 * Giving currency, or taking it away, with Grant Currency (docs/economy.md):
 * to one member, from their profile card or menu, or to everyone with a
 * role, from the finance page. The reason goes to them and the audit log.
 */
export function GiveDialog({ target, onClose }: { target: GiveTarget | null; onClose: () => void }) {
  // Keep what was shown while the dialog closes.
  const [shown, setShown] = useState<GiveTarget | null>(null);
  if (target && target !== shown) setShown(target);
  return (
    <Dialog open={!!target} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>{shown && <Body key={`${shown.serverId}:${shown.user?.id ?? "role"}`} target={shown} onDone={onClose} />}</DialogContent>
    </Dialog>
  );
}

function Body({ target, onDone }: { target: GiveTarget; onDone: () => void }) {
  const { t } = useI18n();
  const id = useId();
  const { instanceKey, serverId, user } = target;
  const inst = useInstance(instanceKey);
  const { currencies } = useEconomy(instanceKey, serverId);
  const roles = sortRoles(inst?.roles[serverId] ?? []);
  const [currencyId, setCurrencyId] = useState("");
  const [roleId, setRoleId] = useState(serverId);
  const [way, setWay] = useState<"give" | "take">("give");
  const [amount, setAmount] = useState("");
  const [reason, setReason] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [has, setHas] = useState<Record<string, number> | null>(null);
  const currency = currencies?.find((c) => c.id === currencyId) ?? currencies?.[0];
  const value = parseAmount(amount, 1, AMOUNT_MAX);

  // What they have now, so a take says how far it can go.
  useEffect(() => {
    if (!user) return;
    let live = true;
    walletOf(instanceKey, serverId, user.id).then(
      (w) => live && setHas(Object.fromEntries(w.balances.map((b) => [b.currencyId, num(b.amount)]))),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [instanceKey, serverId, user]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!currency || value === null || busy) return;
    setBusy(true);
    setError(null);
    try {
      const signed = way === "take" ? -value : value;
      const res = await grant(instanceKey, serverId, user ? { userId: user.id } : { roleId }, currency.id, signed, reason.trim());
      const moved = Math.abs(num(res.total));
      toast(
        user
          ? t(way === "take" ? "chattools.economy.tookFrom" : "chattools.economy.gaveTo", { amount: moved, currency: currency.name, name: displayName(user) })
          : t(way === "take" ? "chattools.economy.tookFromPeople" : "chattools.economy.gaveToPeople", { amount: moved, currency: currency.name, count: res.people }),
      );
      onDone();
    } catch (err) {
      setError((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-4">
      <DialogHeader
        title={
          <span className="flex items-center gap-2.5">
            <motion.span
              initial={{ scale: 0.4, rotate: -25 }}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: "spring", stiffness: 520, damping: 14 }}
              className="grid size-9 shrink-0 place-items-center rounded-xl bg-amber-500/15 text-amber-500"
            >
              <HandCoinsIcon className="size-5" />
            </motion.span>
            {user ? t("chattools.economy.giveTitle", { name: displayName(user) }) : t("chattools.economy.giveRoleTitle")}
          </span>
        }
        description={t("chattools.economy.giveAbout")}
      />
      {user ? (
        <div className="flex items-center gap-3 rounded-2xl bg-muted/60 p-3">
          <UserAvatar user={user} className="size-9" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-sm font-bold">{displayName(user)}</span>
            <span className="block truncate text-xs text-muted-foreground">
              @<Private text={user.username} kind="name" />
            </span>
          </span>
          {currency && has && <Amount instanceKey={instanceKey} currency={currency} value={has[currency.id] ?? 0} />}
        </div>
      ) : (
        <div className="flex flex-col gap-2">
          <Label className="font-bold">{t("chattools.economy.toRole")}</Label>
          <div role="radiogroup" aria-label={t("chattools.economy.toRole")} className="flex max-h-40 flex-wrap gap-1.5 overflow-y-auto">
            {roles.map((r) => {
              const active = r.id === roleId;
              return (
                <motion.button
                  key={r.id}
                  type="button"
                  role="radio"
                  aria-checked={active}
                  whileTap={{ scale: 0.94 }}
                  onClick={() => setRoleId(r.id)}
                  className={cn("relative flex items-center gap-1.5 rounded-full border px-3 py-1 text-xs font-bold transition-colors", active ? "border-primary/60" : "text-muted-foreground hover:border-primary/30")}
                >
                  {active && <motion.span layoutId={`${id}-role`} transition={SPRING} className="absolute inset-0 rounded-full bg-primary/12" />}
                  <span className="relative size-2 rounded-full" style={{ backgroundColor: r.color ? cssColor(r.color) : "currentColor" }} />
                  <span className="relative">{r.id === serverId ? "@everyone" : r.name}</span>
                </motion.button>
              );
            })}
          </div>
          <p className="text-xs text-muted-foreground">{t("chattools.economy.toRoleHint")}</p>
        </div>
      )}

      {!currencies?.length ? (
        <p className="text-sm text-muted-foreground">{t("chattools.economy.noCurrencies")}</p>
      ) : (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <Segmented
              label={t("chattools.economy.giveOrTake")}
              value={way}
              onChange={setWay}
              options={[
                { value: "give", label: t("chattools.economy.give"), icon: <PlusIcon className="size-4" /> },
                { value: "take", label: t("chattools.economy.take"), icon: <MinusIcon className="size-4" /> },
              ]}
            />
            {currencies.length > 1 && (
              <Segmented label={t("chattools.economy.currency")} value={currency?.id ?? ""} onChange={setCurrencyId} options={currencies.map((c) => ({ value: c.id, label: c.name }))} />
            )}
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor={`${id}-amount`} className="font-bold">
              {user ? t("chattools.economy.amount") : t("chattools.economy.amountEach")}
            </Label>
            <Input id={`${id}-amount`} inputMode="numeric" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0" className="h-10 rounded-xl font-bold tabular-nums" />
            {amount && value === null && <p className="text-xs text-destructive">{t("chattools.economy.badAmount", { max: AMOUNT_MAX })}</p>}
            {way === "take" && <p className="text-xs text-muted-foreground">{t("chattools.economy.takeHint")}</p>}
          </div>
          <div className="flex flex-col gap-2">
            <Label htmlFor={`${id}-reason`} className="flex items-center justify-between font-bold">
              {t("chattools.economy.reason")}
              <span className="text-xs font-normal text-muted-foreground tabular-nums">
                <Count value={NOTE_MAX - reason.length} />
              </span>
            </Label>
            <Textarea id={`${id}-reason`} rows={2} maxLength={NOTE_MAX} value={reason} onChange={(e) => setReason(e.target.value)} placeholder={t("chattools.economy.reasonPlaceholder")} className="rounded-xl" />
            <p className="text-xs text-muted-foreground">{t("chattools.economy.reasonShown")}</p>
          </div>
        </>
      )}

      <AnimatePresence mode="popLayout" initial={false}>
        {error && (
          <motion.p key="error" {...SLIDE_IN} transition={SPRING} role="alert" className="text-sm text-destructive first-letter:uppercase">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <motion.div layout="position" transition={SPRING} className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={onDone} className="rounded-xl">
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant={way === "take" ? "destructive" : "default"} disabled={!currency || value === null || busy} className="group rounded-xl font-bold">
          {busy ? <LoaderCircleIcon className="animate-spin" /> : <HandCoinsIcon className="transition-transform group-hover:-rotate-12" />}
          {value !== null && currency ? (
            <T k={way === "take" ? "chattools.economy.takeAmount" : "chattools.economy.giveAmount"} values={{ amount: <Amount instanceKey={instanceKey} currency={currency} value={value} /> }} />
          ) : way === "take" ? (
            t("chattools.economy.take")
          ) : (
            t("chattools.economy.give")
          )}
        </Button>
      </motion.div>
    </form>
  );
}
