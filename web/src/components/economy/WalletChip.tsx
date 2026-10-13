import { ChevronRightIcon, GiftIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEconomy, openShop } from "@/fuwa/economy";
import { Amount } from "@/components/economy/shared";
import { useI18n } from "@/i18n/react";
import { dailyOf, dailyRule, num } from "@/lib/economy";
import { useNow } from "@/lib/notifications";
import { SPRING } from "@/lib/motion";

/** At most this many currencies side by side; the shop shows the rest. */
const SHOWN = 3;

/**
 * Your balances in a server, above your name in the channel sidebar: each
 * currency's icon and count, rolling as it changes, with a gift when a
 * daily reward is waiting. Opens the shop. Only where the server has
 * currencies.
 */
export function WalletChip({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const { currencies, wallet } = useEconomy(instanceKey, serverId);
  const now = useNow(60_000);
  const shown = currencies && wallet ? currencies.slice(0, SHOWN) : [];
  const claimable = !!currencies?.some((c) => {
    const rule = dailyRule(c);
    return rule && dailyOf(wallet?.balances.find((b) => b.currencyId === c.id), rule, now).ready;
  });
  return (
    <AnimatePresence initial={false}>
      {shown.length > 0 && (
        <motion.div
          key="wallet"
          initial={{ opacity: 0, height: 0 }}
          animate={{ opacity: 1, height: "auto" }}
          exit={{ opacity: 0, height: 0 }}
          transition={SPRING}
          className="shrink-0 overflow-hidden border-t bg-[color-mix(in_srgb,var(--background)_60%,transparent)]"
        >
          <button
            type="button"
            onClick={() => openShop(instanceKey, serverId)}
            aria-label={t("chattools.economy.openShop")}
            title={t("chattools.economy.openShop")}
            className="group flex w-full items-center gap-3 px-3 py-1.5 text-left text-sm transition-colors hover:bg-muted/60 active:scale-[0.99]"
          >
            <span className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-0.5">
              {shown.map((c) => (
                <Amount
                  key={c.id}
                  instanceKey={instanceKey}
                  currency={c}
                  value={num(wallet?.balances.find((b) => b.currencyId === c.id)?.amount)}
                  iconClassName="transition-transform duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:-rotate-12 group-hover:scale-125"
                />
              ))}
            </span>
            <AnimatePresence initial={false}>
              {claimable && (
                <motion.span
                  key="daily"
                  initial={{ scale: 0, rotate: -30 }}
                  animate={{ scale: 1, rotate: [0, -12, 12, -6, 0] }}
                  exit={{ scale: 0 }}
                  transition={{ ...SPRING, rotate: { duration: 0.8, repeat: Infinity, repeatDelay: 3 } }}
                  title={t("chattools.economy.dailyWaiting")}
                  className="grid size-6 place-items-center rounded-full bg-primary/15 text-primary"
                >
                  <GiftIcon className="size-3.5" />
                </motion.span>
              )}
            </AnimatePresence>
            <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5" />
          </button>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
