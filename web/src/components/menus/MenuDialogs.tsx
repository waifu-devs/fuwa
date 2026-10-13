import { LoaderCircleIcon, TriangleAlertIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { useState, useSyncExternalStore } from "react";
import { InviteDialog } from "@/components/dialogs/InviteDialog";
import { lazyComponent } from "@/components/lazy";
import { ModerateDialog } from "@/components/ModerateDialog";
import "@/components/economy/menus";
import { GiveDialog } from "@/components/economy/GiveDialog";
import "@/components/modview/menus";
import { ReportDialog } from "@/components/modview/ReportDialog";
import { closeMenuDialog as close, subscribeMenuDialogs as subscribe, menuDialogs, type Confirm } from "@/components/menus/dialogs";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { useShopView } from "@/fuwa/economy";
import { useModView } from "@/fuwa/moderation";
import { useI18n } from "@/i18n/react";

/*
 * Dialogs a right-click menu opens from places that don't have them already:
 * the server rail has no invite or settings dialog of its own, and a member
 * row has no moderation dialog. They're the same dialogs the buttons open.
 * Reporting and Mod View (docs/mod-view.md) open from here too, wherever
 * they're asked for, and so do the shop and giving currency
 * (docs/economy.md).
 */

const ModView = lazyComponent(
  () => import("@/components/modview/ModView").then((m) => m.ModViewHost),
  (p) => p.open,
);

const Shop = lazyComponent(
  () => import("@/components/economy/Shop").then((m) => m.ShopHost),
  (p) => p.open,
);

const ServerSettingsDialog = lazyComponent(
  () => import("@/components/dialogs/ServerSettingsDialog").then((m) => m.ServerSettingsDialog),
  (p) => p.open,
);

export function MenuDialogs() {
  const { open, last } = useSyncExternalStore(subscribe, menuDialogs);
  const moderate = last.moderate?.kind === "moderate" ? last.moderate : null;
  const invite = last.invite?.kind === "invite" ? last.invite : null;
  const settings = last["server-settings"]?.kind === "server-settings" ? last["server-settings"] : null;
  const confirm = last.confirm?.kind === "confirm" ? last.confirm.confirm : null;
  const modView = useModView();
  const shop = useShopView();
  return (
    <>
      <ReportDialog target={open?.kind === "report" ? open.target : null} onClose={close} />
      <ModView open={modView.open} />
      <Shop open={shop.open} />
      <GiveDialog target={open?.kind === "give" ? open.target : null} onClose={close} />
      {moderate && (
        <ModerateDialog
          instanceKey={moderate.instanceKey}
          serverId={moderate.serverId}
          member={open?.kind === "moderate" ? moderate.member : null}
          action={open?.kind === "moderate" ? moderate.action : null}
          onClose={close}
        />
      )}
      {invite && (
        <InviteDialog
          open={open?.kind === "invite"}
          onOpenChange={(o) => !o && close()}
          instanceKey={invite.instanceKey}
          serverId={invite.serverId}
          channelId={invite.channelId}
        />
      )}
      {settings && (
        <ServerSettingsDialog
          open={open?.kind === "server-settings"}
          onOpenChange={(o) => !o && close()}
          instanceKey={settings.instanceKey}
          server={settings.server}
          tab={settings.tab}
          target={settings.target}
        />
      )}
      <Dialog open={open?.kind === "confirm"} onOpenChange={(o) => !o && close()}>
        <DialogContent>{confirm && <ConfirmBody key={confirm.title} confirm={confirm} />}</DialogContent>
      </Dialog>
    </>
  );
}

function ConfirmBody({ confirm }: { confirm: Confirm }) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function go() {
    setBusy(true);
    setError(null);
    try {
      await confirm.run();
      close();
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <DialogHeader
        title={
          <span className="flex items-center gap-2.5">
            <motion.span
              initial={{ scale: 0.4, rotate: -25 }}
              animate={{ scale: 1, rotate: 0 }}
              transition={{ type: "spring", stiffness: 520, damping: 14 }}
              className="grid size-9 shrink-0 place-items-center rounded-xl bg-destructive/15 text-destructive"
            >
              <TriangleAlertIcon className="size-5" />
            </motion.span>
            {confirm.title}
          </span>
        }
        description={confirm.body}
      />
      {error && <p className="-mt-2 mb-3 text-sm text-destructive first-letter:uppercase">{error}</p>}
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" onClick={close} className="rounded-xl">
          {t("common.cancel")}
        </Button>
        <Button type="button" variant="destructive" disabled={busy} onClick={() => void go()} className="rounded-xl font-bold" autoFocus>
          {busy && <LoaderCircleIcon className="animate-spin" />} {confirm.action}
        </Button>
      </div>
    </>
  );
}
