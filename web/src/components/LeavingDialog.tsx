import { ExternalLinkIcon, ShieldAlertIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { useState, useSyncExternalStore } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { useI18n } from "@/i18n/react";
import { hostOf, trusted } from "@/lib/links";
import { getPrefs, setPrefs } from "@/lib/prefs";

/*
 * Before a masked link opens (its words aren't where it goes, lib/links.ts):
 * the real address, its site picked out, and a way to trust that site so
 * its links open straight away from then on, on this device.
 */

let asking: string | null = null;
const listeners = new Set<() => void>();
const set = (href: string | null) => {
  asking = href;
  for (const l of listeners) l();
};

/** Asks before opening `href` in a new tab. */
export const askToLeave = (href: string) => set(href);

/**
 * For a click on a masked link: asks first, unless its site is trusted or
 * it stays on this site. Pressed with a modifier (a new tab or window by
 * choice) it asks all the same.
 */
export function guardMasked(event: { preventDefault: () => void }, href: string) {
  let url: URL;
  try {
    url = new URL(href, location.href);
  } catch {
    return;
  }
  if (url.origin === location.origin || trusted(getPrefs().trustedDomains, hostOf(url.href))) return;
  event.preventDefault();
  askToLeave(url.href);
}

const open = (href: string) => window.open(href, "_blank", "noopener,noreferrer");

/** The one dialog, mounted once beside the app's other hosts. */
export function LeavingDialogHost() {
  const href = useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => asking,
  );
  // The last address stays while the dialog plays its exit.
  const [shown, setShown] = useState(href);
  if (href && href !== shown) setShown(href);
  return <LeavingDialog href={shown ?? ""} open={!!href} onClose={() => set(null)} />;
}

function LeavingDialog({ href, open: isOpen, onClose }: { href: string; open: boolean; onClose: () => void }) {
  const { t } = useI18n();
  const [trust, setTrust] = useState(false);
  const host = hostOf(href);
  const at = href.indexOf(host);
  const close = () => {
    setTrust(false);
    onClose();
  };
  const go = () => {
    if (trust && host) setPrefs((p) => ({ trustedDomains: [...p.trustedDomains, host] }));
    open(href);
    close();
  };
  return (
    <Dialog
      open={isOpen}
      onOpenChange={(next) => {
        if (!next) close();
      }}
    >
      <DialogContent>
        <DialogHeader title={t("chat.leaving.title")} description={t("chat.leaving.about")} />
        <motion.div
          initial={{ opacity: 0, y: 6 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ type: "spring", stiffness: 460, damping: 30, delay: 0.05 }}
          className="flex items-start gap-3 rounded-2xl border bg-muted/40 p-3"
        >
          <ShieldAlertIcon className="mt-0.5 size-5 shrink-0 text-primary" />
          <p className="min-w-0 font-mono text-sm break-all select-all">
            {at >= 0 && host ? (
              <>
                <span className="text-muted-foreground">{href.slice(0, at)}</span>
                <span className="font-bold text-foreground">{host}</span>
                <span className="text-muted-foreground">{href.slice(at + host.length)}</span>
              </>
            ) : (
              href
            )}
          </p>
        </motion.div>
        {host && (
          <label className="mt-3 flex cursor-pointer items-center justify-between gap-4 rounded-2xl border p-3">
            <span>
              <span className="block text-sm font-bold">{t("chat.leaving.trust", { site: host })}</span>
              <span className="block text-xs text-muted-foreground">{t("chat.leaving.trustHint")}</span>
            </span>
            <Switch checked={trust} onCheckedChange={setTrust} />
          </label>
        )}
        <div className="mt-5 flex justify-end gap-2">
          <Button variant="ghost" onClick={close} autoFocus>
            {t("chat.leaving.back")}
          </Button>
          <Button onClick={go}>
            <ExternalLinkIcon />
            {t("chat.leaving.visit")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
