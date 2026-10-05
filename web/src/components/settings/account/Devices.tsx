import { timestampDate } from "@bufbuild/protobuf/wkt";
import { LaptopIcon, LogOutIcon, ShieldCheckIcon, SmartphoneIcon, TabletIcon, TerminalIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import type { Session } from "@/gen/fuwa/v1/account_pb";
import { listSessions, revokeOtherSessions, revokeSession, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { Count, SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { T, useI18n } from "@/i18n/react";
import { MessageBackup } from "./MessageBackup";
import { activeAgo, describeDevice, deviceName, type DeviceKind } from "@/lib/devices";
import { useNow } from "@/lib/notifications";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const ICONS: Record<DeviceKind, typeof LaptopIcon> = { computer: LaptopIcon, phone: SmartphoneIcon, tablet: TabletIcon, tool: TerminalIcon };

const day = (d: Date) => d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });

/**
 * Every device signed in to your account on this instance, most recently
 * used first. Sign out one you don't recognise, or all but this one.
 */
export function Devices({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const [sessions, setSessions] = useState<Session[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [leaving, setLeaving] = useState<Set<string>>(new Set());
  const [confirm, setConfirm] = useState(false);
  const [signingOut, setSigningOut] = useState(false);
  const now = useNow(60_000);
  const where = inst?.node?.name ?? t("settings.nav.thisInstance");

  const load = useCallback(() => {
    run(listSessions(instanceKey)).then(
      (s) => {
        setSessions(s);
        setError(null);
      },
      (err: FuwaError) => setError(err.message),
    );
  }, [instanceKey]);
  useEffect(load, [load]);

  const current = sessions?.find((s) => s.current);
  const others = (sessions ?? []).filter((s) => !s.current);

  async function signOut(id: string) {
    setLeaving((l) => new Set(l).add(id));
    try {
      await run(revokeSession(instanceKey, id));
      setSessions((s) => s?.filter((x) => x.id !== id) ?? null);
      toast(t("accountsettings.devices.signedOutOne"));
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setLeaving((l) => {
        const next = new Set(l);
        next.delete(id);
        return next;
      });
    }
  }

  async function signOutOthers() {
    setSigningOut(true);
    try {
      const n = await run(revokeOtherSessions(instanceKey));
      setSessions((s) => s?.filter((x) => x.current) ?? null);
      toast(t("accountsettings.devices.signedOutCount", { count: n }));
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setSigningOut(false);
      setConfirm(false);
    }
  }

  if (error && !sessions) {
    return (
      <div className="flex flex-col items-start gap-3 rounded-2xl border border-destructive/40 bg-destructive/5 p-4">
        <p className="text-sm font-bold text-destructive first-letter:uppercase">{error}</p>
        <Button type="button" variant="outline" size="sm" className="rounded-xl" onClick={load}>
          {t("accountsettings.shared.tryAgain")}
        </Button>
      </div>
    );
  }

  return (
    <div className="flex flex-col gap-8">
      <section>
        <h3 className="mb-3 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">{t("accountsettings.devices.thisDevice")}</h3>
        {current ? (
          <DeviceRow session={current} now={now} here />
        ) : (
          <div className="shimmer h-[4.5rem] rounded-2xl" />
        )}
      </section>

      <section>
        <div className="mb-3 flex items-center justify-between gap-3">
          <h3 className="text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">
            {sessions ? (
              <T k="accountsettings.devices.othersCount" values={{ count: <Count value={others.length} /> }} count={others.length} />
            ) : (
              t("accountsettings.devices.others")
            )}
          </h3>
        </div>
        {!sessions ? (
          <div className="flex flex-col gap-2">
            <div className="shimmer h-[4.5rem] rounded-2xl" />
            <div className="shimmer h-[4.5rem] rounded-2xl opacity-60" />
          </div>
        ) : (
          <motion.ul layout className="flex flex-col gap-2">
            <AnimatePresence initial={false} mode="popLayout">
              {others.map((s, n) => (
                <motion.li
                  key={s.id}
                  layout
                  initial={{ opacity: 0, y: 12 }}
                  animate={{ opacity: 1, y: 0, transition: { ...SPRING, delay: n * 0.04 } }}
                  exit={{ opacity: 0, x: 60, scale: 0.95, transition: { duration: 0.25 } }}
                >
                  <DeviceRow session={s} now={now}>
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      disabled={leaving.has(s.id)}
                      aria-label={t("accountsettings.devices.signOutDevice", { device: deviceName(t, describeDevice(s.userAgent)) })}
                      onClick={() => void signOut(s.id)}
                      className="group shrink-0 rounded-xl text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
                    >
                      <LogOutIcon className="size-4 transition-transform group-hover:translate-x-0.5" />
                      <span className="hidden sm:inline">{leaving.has(s.id) ? t("accountsettings.shared.signingOut") : t("accountsettings.shared.signOut")}</span>
                    </Button>
                  </DeviceRow>
                </motion.li>
              ))}
              {others.length === 0 && (
                <motion.li
                  key="none"
                  layout
                  initial={{ opacity: 0, scale: 0.95 }}
                  animate={{ opacity: 1, scale: 1 }}
                  exit={{ opacity: 0 }}
                  transition={SPRING}
                  className="flex items-center gap-3 rounded-2xl border border-dashed p-4"
                >
                  <motion.span
                    initial={{ scale: 0, rotate: -30 }}
                    animate={{ scale: 1, rotate: 0 }}
                    transition={{ type: "spring", stiffness: 500, damping: 14, delay: 0.1 }}
                    className="grid size-10 shrink-0 place-items-center rounded-xl bg-emerald-500/15 text-emerald-500"
                  >
                    <ShieldCheckIcon className="size-5" />
                  </motion.span>
                  <span>
                    <span className="block text-sm font-bold">{t("accountsettings.devices.onlyThis")}</span>
                    <span className="block text-sm text-muted-foreground">{t("accountsettings.devices.onlyThisHint", { instance: where })}</span>
                  </span>
                </motion.li>
              )}
            </AnimatePresence>
          </motion.ul>
        )}
      </section>

      <AnimatePresence initial={false}>
        {others.length > 1 && (
          <motion.section
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            transition={SPRING}
            className="overflow-hidden"
          >
            <div className="flex flex-wrap items-center gap-3 rounded-2xl border p-4">
              <div className="min-w-0 flex-1">
                <p className="text-sm font-bold">{t("accountsettings.devices.signOutAll")}</p>
                <p className="text-sm text-muted-foreground">{t("accountsettings.devices.signOutAllHint")}</p>
              </div>
              <AnimatePresence mode="popLayout" initial={false}>
                {confirm ? (
                  <motion.div key="confirm" initial={{ opacity: 0, x: 12 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 12 }} transition={SPRING} className="flex gap-2">
                    <Button type="button" variant="ghost" size="sm" className="rounded-xl" onClick={() => setConfirm(false)} disabled={signingOut}>
                      {t("common.cancel")}
                    </Button>
                    <Button type="button" variant="destructive" size="sm" className="rounded-xl font-bold" onClick={() => void signOutOthers()} disabled={signingOut}>
                      {signingOut ? t("accountsettings.shared.signingOut") : t("accountsettings.devices.signOutCount", { count: others.length })}
                    </Button>
                  </motion.div>
                ) : (
                  <motion.div key="ask" initial={{ opacity: 0, x: -12 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -12 }} transition={SPRING}>
                    <Button type="button" variant="outline" size="sm" className="rounded-xl border-destructive/40 text-destructive hover:bg-destructive/10" onClick={() => setConfirm(true)}>
                      <LogOutIcon className="size-4" /> {t("accountsettings.devices.signOutAllButton")}
                    </Button>
                  </motion.div>
                )}
              </AnimatePresence>
            </div>
          </motion.section>
        )}
      </AnimatePresence>

      <MessageBackup instanceKey={instanceKey} />
    </div>
  );
}

function DeviceRow({ session, now, here = false, children }: { session: Session; now: number; here?: boolean; children?: ReactNode }) {
  const lang = useI18n();
  const { t } = lang;
  const device = describeDevice(session.userAgent);
  const Icon = ICONS[device.kind];
  const created = session.createdAt ? timestampDate(session.createdAt) : null;
  const active = session.lastActiveAt ? timestampDate(session.lastActiveAt) : created;
  return (
    <div className={cn("flex items-center gap-3 rounded-2xl border p-3 pr-2 transition-colors", here ? "border-primary/40 bg-primary/5" : "bg-card hover:border-primary/30")}>
      <span className={cn("relative grid size-11 shrink-0 place-items-center rounded-xl", here ? "bg-primary text-primary-foreground" : "bg-muted text-muted-foreground")}>
        <Icon className="size-5" />
        {here && (
          <span className="absolute -right-0.5 -bottom-0.5 size-3 rounded-full bg-emerald-500 ring-2 ring-card">
            <span className="absolute inset-0 animate-ping rounded-full bg-emerald-500 opacity-60" />
          </span>
        )}
      </span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-bold">{deviceName(t, device)}</p>
        <p className="truncate text-xs text-muted-foreground">
          {here ? t("accountsettings.devices.usingNow") : active ? activeAgo(lang, active, now) : t("accountsettings.devices.notUsed")}
          {created && ` · ${t("accountsettings.devices.signedIn", { date: day(created) })}`}
        </p>
      </div>
      {children}
    </div>
  );
}
