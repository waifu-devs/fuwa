import { BadgeCheckIcon, CheckIcon, CopyIcon, LaptopIcon, ShieldAlertIcon, SmartphoneIcon, TabletIcon, TerminalIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import type { Conversation, Device } from "@/gen/fuwa/v1/dm_pb";
import { verifyDm } from "@/fuwa/dms";
import { engine } from "@/fuwa/sync";
import { useFuwa, type DmMember } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/components/motion";
import { Padlock } from "@/components/dm/Padlock";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { describeDevice, deviceName, type DeviceKind } from "@/lib/devices";
import { displayName } from "@/lib/format";
import { T, useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const NO_MEMBERS: DmMember[] = [];

const KIND_ICON: Record<DeviceKind, typeof LaptopIcon> = {
  computer: LaptopIcon,
  phone: SmartphoneIcon,
  tablet: TabletIcon,
  tool: TerminalIcon,
};

/** A device id as four groups of four, like a key fingerprint. */
const fingerprint = (id: string) => id.slice(0, 16).match(/.{4}/g)?.join(" ") ?? id;

/**
 * How a conversation is kept private, and the means to check it: the safety
 * number both people should see the same, and every device that can read it.
 */
export function EncryptionDialog({
  open,
  onOpenChange,
  instanceKey,
  conversation,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  conversation: Conversation;
}) {
  const { t } = useI18n();
  const id = conversation.id;
  const me = useFuwa((s) => s.instances[instanceKey]?.me);
  const myDevice = useFuwa((s) => s.instances[instanceKey]?.dms.deviceId ?? "");
  const safety = useFuwa((s) => s.instances[instanceKey]?.dms.safety[id] ?? "");
  const verified = useFuwa((s) => s.instances[instanceKey]?.dms.verified[id] ?? "");
  const members = useFuwa((s) => s.instances[instanceKey]?.dms.members[id] ?? NO_MEMBERS);
  const partner = conversation.users.find((u) => u.id !== me?.id);
  const [devices, setDevices] = useState<Map<string, Device>>(new Map());
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    engine(instanceKey)
      .api.dms.listDevices({ userIds: conversation.users.map((u) => u.id) })
      .then(({ devices }) => !cancelled && setDevices(new Map(devices.map((d) => [d.id, d]))))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [open, instanceKey, conversation.users]);

  const isVerified = !!safety && verified === safety;
  const changed = !!verified && !!safety && verified !== safety;
  const groups = safety.match(/.{5}/g) ?? [];

  async function toggle() {
    setBusy(true);
    try {
      await verifyDm(instanceKey, id, isVerified ? "" : safety);
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <div className="mb-4 flex justify-center">
          <Padlock delay={0.1} />
        </div>
        <DialogHeader
          title={t("dms-calls.dm.encrypted")}
          description={<T k="dms-calls.dm.encryption.description" values={{ name: displayName(partner) }} />}
        />

        <section className="rounded-2xl border bg-muted/40 p-4">
          <div className="flex items-center justify-between gap-2">
            <p className="text-sm font-extrabold">{t("dms-calls.dm.encryption.safety")}</p>
            <AnimatePresence mode="popLayout" initial={false}>
              {isVerified ? (
                <motion.span
                  key="verified"
                  initial={{ scale: 0.5, opacity: 0 }}
                  animate={{ scale: 1, opacity: 1 }}
                  exit={{ scale: 0.5, opacity: 0 }}
                  transition={{ type: "spring", stiffness: 600, damping: 16 }}
                  className="flex items-center gap-1 rounded-full bg-emerald-500/15 px-2 py-0.5 text-xs font-bold text-emerald-700 dark:text-emerald-300"
                >
                  <BadgeCheckIcon className="size-3.5" /> {t("dms-calls.dm.trust.verified")}
                </motion.span>
              ) : changed ? (
                <motion.span
                  key="changed"
                  initial={{ scale: 0.5, opacity: 0 }}
                  animate={{ scale: 1, opacity: 1 }}
                  exit={{ scale: 0.5, opacity: 0 }}
                  className="flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 text-xs font-bold text-amber-700 dark:text-amber-300"
                >
                  <ShieldAlertIcon className="size-3.5" /> {t("dms-calls.dm.encryption.changed")}
                </motion.span>
              ) : null}
            </AnimatePresence>
          </div>
          {groups.length ? (
            <motion.div
              key={safety}
              className="mt-3 grid grid-cols-4 gap-x-3 gap-y-2 font-mono text-[0.95rem] font-bold tracking-wider tabular-nums sm:text-base"
              initial="hidden"
              animate="shown"
              variants={{ shown: { transition: { staggerChildren: 0.035, delayChildren: 0.15 } } }}
            >
              {groups.map((group, n) => (
                <motion.span
                  key={n}
                  variants={{ hidden: { opacity: 0, rotateX: -90, y: 6 }, shown: { opacity: 1, rotateX: 0, y: 0 } }}
                  transition={SPRING}
                  className="text-center"
                >
                  {group}
                </motion.span>
              ))}
            </motion.div>
          ) : (
            <p className="mt-2 text-sm text-muted-foreground">{t("dms-calls.dm.encryption.noNumber")}</p>
          )}
          <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
            {changed
              ? t("dms-calls.dm.encryption.changedText", { name: displayName(partner) })
              : t("dms-calls.dm.encryption.compare", { name: displayName(partner) })}
          </p>
          {groups.length > 0 && (
            <div className="mt-3 flex flex-wrap gap-2">
              <Button size="sm" variant={isVerified ? "outline" : "default"} disabled={busy} onClick={() => void toggle()} className="btn rounded-xl font-bold">
                {isVerified ? t("dms-calls.dm.encryption.clear") : t("dms-calls.dm.encryption.mark")}
              </Button>
              <Button
                size="sm"
                variant="ghost"
                className="rounded-xl font-bold"
                onClick={() => {
                  void navigator.clipboard?.writeText(groups.join(" "));
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1200);
                }}
              >
                <AnimatePresence mode="wait" initial={false}>
                  <motion.span
                    key={copied ? "copied" : "copy"}
                    initial={{ scale: 0.3, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    exit={{ scale: 0.3, opacity: 0 }}
                    transition={{ type: "spring", stiffness: 700, damping: 22 }}
                    className="grid place-items-center"
                  >
                    {copied ? <CheckIcon className="text-primary" /> : <CopyIcon />}
                  </motion.span>
                </AnimatePresence>
                {copied ? t("dms-calls.dm.encryption.copied") : t("dms-calls.dm.encryption.copy")}
              </Button>
            </div>
          )}
        </section>

        <section className="mt-4">
          <p className="mb-2 text-sm font-extrabold">{t("dms-calls.dm.encryption.devices")}</p>
          <div className="flex flex-col gap-3">
            {conversation.users.map((user) => {
              const own = members.filter((m) => m.userId === user.id);
              return (
                <div key={user.id}>
                  <p className="mb-1 flex items-center gap-2 text-xs font-bold text-muted-foreground">
                    <UserAvatar user={user} className="size-5 text-[0.6rem]" />
                    {user.id === me?.id ? t("dms-calls.dm.encryption.you") : displayName(user)}
                  </p>
                  {own.length === 0 && <p className="pl-7 text-xs text-muted-foreground">{t("dms-calls.dm.encryption.noDevices")}</p>}
                  <ul className="flex flex-col gap-1">
                    {own.map((m, n) => {
                      const device = devices.get(m.deviceId);
                      const described = describeDevice(device?.label ?? "");
                      const Icon = KIND_ICON[described.kind];
                      const mine = m.deviceId === myDevice;
                      return (
                        <motion.li
                          key={m.deviceId}
                          initial={{ opacity: 0, x: -8 }}
                          animate={{ opacity: 1, x: 0 }}
                          transition={{ ...SPRING, delay: 0.2 + n * 0.05 }}
                          className="group flex items-center gap-3 rounded-xl px-2 py-1.5 transition hover:bg-muted/60"
                        >
                          <span
                            className={cn(
                              "grid size-8 shrink-0 place-items-center rounded-lg transition-transform duration-300 group-hover:-rotate-6",
                              mine ? "bg-primary/15 text-primary" : "bg-muted text-muted-foreground",
                            )}
                          >
                            <Icon className="size-4" />
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="flex items-center gap-1.5 text-sm font-bold">
                              <span className="truncate">{device ? deviceName(t, described) : t("dms-calls.dm.encryption.signedOut")}</span>
                              {mine && <span className="shrink-0 rounded-full bg-primary/15 px-1.5 text-[0.65rem] text-primary">{t("dms-calls.dm.encryption.thisDevice")}</span>}
                            </span>
                            <span className="block font-mono text-[0.7rem] text-muted-foreground" title={m.deviceId}>
                              {fingerprint(m.deviceId)}
                            </span>
                          </span>
                        </motion.li>
                      );
                    })}
                  </ul>
                </div>
              );
            })}
          </div>
          <p className="mt-3 text-xs leading-relaxed text-muted-foreground">
            {t("dms-calls.dm.encryption.footer")}
          </p>
        </section>
      </DialogContent>
    </Dialog>
  );
}
