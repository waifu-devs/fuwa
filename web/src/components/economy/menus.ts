import { HandCoinsIcon, StoreIcon } from "lucide-react";
import { Permission } from "@/gen/fuwa/v1/types_pb";
import { economyHere, economyNow, openShop } from "@/fuwa/economy";
import { accessNow } from "@/fuwa/hooks";
import { openMenuDialog } from "@/components/menus/dialogs";
import { i18n } from "@/i18n/i18n";
import { extendMenu, items } from "@/lib/context-menu";
import { has } from "@/lib/permissions";

/*
 * The economy's items in the app's right-click menus (docs/context-menus.md):
 * "Give currency" on people, for whoever has Grant Currency, and "Shop" on
 * servers with currencies. Only on instances that have the economy.
 */

const hasCurrencies = (instanceKey: string, serverId: string) => !!economyNow(instanceKey, serverId).currencies?.length;

extendMenu("member", {
  section: "moderate",
  at: "end",
  build: ({ instanceKey, serverId, user }) => {
    if (!serverId || !economyHere(instanceKey) || !hasCurrencies(instanceKey, serverId)) return [];
    if (!has(accessNow(instanceKey, serverId), Permission.GRANT_CURRENCY)) return [];
    return items({
      id: "give-currency",
      label: i18n().t("chattools.economy.giveItem"),
      icon: HandCoinsIcon,
      onSelect: () => openMenuDialog({ kind: "give", target: { instanceKey, serverId, user } }),
    });
  },
});

extendMenu("server", {
  section: "primary",
  at: "end",
  build: ({ instanceKey, server }) => {
    if (!economyHere(instanceKey) || !hasCurrencies(instanceKey, server.id)) return [];
    return items({ id: "shop", label: i18n().t("chattools.economy.shopItem"), icon: StoreIcon, onSelect: () => openShop(instanceKey, server.id) });
  },
});
