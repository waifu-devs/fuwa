import { CheckIcon, ChevronDownIcon, CopyIcon, HashIcon, LoaderCircleIcon, RefreshCwIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useRef, useState } from "react";
import type { Invite } from "@/gen/fuwa/v1/types_pb";
import { createInvite, listInvites, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { getInstance, useInstance } from "@/fuwa/hooks";
import { ServerIcon } from "@/components/Icons";
import { SPRING } from "@/lib/motion";
import { usePrivateField } from "@/components/Private";
import { Chips } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { useI18n } from "@/i18n/react";
import { DEFAULT_INVITE, EXPIRE_AFTER, MAX_USES, expiresAt, inviteLink, timeLeft, works } from "@/lib/invites";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** An instance's own address, as links to it should read. */
export const publicBase = (key: string) => {
  const inst = getInstance(key);
  return inst?.node?.publicUrl || inst?.url || "";
};

/**
 * Invite people to a server, or into one of its channels, like Discord: a
 * link is ready to copy the moment it opens, and "Edit invite link" makes one
 * that lasts longer or lets in fewer people.
 */
export function InviteDialog({
  open,
  onOpenChange,
  instanceKey,
  serverId,
  channelId = "",
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
  serverId: string;
  /** The channel it opens for newcomers, or none for the server. */
  channelId?: string;
}) {
  const lang = useI18n();
  const { t } = lang;
  const inst = useInstance(instanceKey);
  const server = inst?.servers.find((s) => s.id === serverId);
  const channel = channelId ? inst?.channels[serverId]?.find((c) => c.id === channelId) : undefined;
  const [invite, setInvite] = useState<Invite | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [options, setOptions] = useState(DEFAULT_INVITE);
  const [making, setMaking] = useState(false);
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<ReturnType<typeof setTimeout>>(undefined);
  const privateField = usePrivateField();

  // Opening reuses a link of yours that's still good for a while, as Discord does, so
  // inviting twice doesn't leave two links lying around; otherwise it makes one.
  useEffect(() => {
    if (!open) return;
    setInvite(null);
    setError(null);
    setEditing(false);
    setOptions(DEFAULT_INVITE);
    setCopied(false);
    let cancelled = false;
    const me = getInstance(instanceKey)?.me?.id;
    run(listInvites(instanceKey, serverId))
      .then(({ invites }) => {
        const now = Date.now();
        const mine = invites.find(
          (i) =>
            i.inviterId === me &&
            i.channelId === channelId &&
            i.maxUses === 0 &&
            works(i, now) &&
            (expiresAt(i)?.getTime() ?? Infinity) - now > 86_400_000,
        );
        return mine ?? run(createInvite(instanceKey, serverId, { channelId, ...DEFAULT_INVITE }));
      })
      .then(
        (made) => !cancelled && setInvite(made),
        (err: FuwaError) => !cancelled && setError(err.message),
      );
    return () => {
      cancelled = true;
    };
  }, [open, instanceKey, serverId, channelId]);

  useEffect(() => () => clearTimeout(copiedTimer.current), []);

  const link = invite ? inviteLink(publicBase(instanceKey), invite.code) : "";

  function copyLink() {
    if (!link) return;
    void navigator.clipboard?.writeText(link).then(
      () => {
        setCopied(true);
        clearTimeout(copiedTimer.current);
        copiedTimer.current = setTimeout(() => setCopied(false), 1600);
      },
      () => toast(t("workspace.invite.copyFailed")),
    );
  }

  async function generate() {
    setMaking(true);
    setError(null);
    try {
      setInvite(await run(createInvite(instanceKey, serverId, { channelId, ...options })));
      setEditing(false);
      setCopied(false);
    } catch (err) {
      setError((err as FuwaError).message);
    } finally {
      setMaking(false);
    }
  }

  const until = invite && expiresAt(invite);
  const limit = invite?.maxUses ?? 0;
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader
          title={
            <span className="flex items-center gap-3">
              {server && (
                <motion.span initial={{ scale: 0.6, rotate: -12 }} animate={{ scale: 1, rotate: 0 }} transition={{ type: "spring", stiffness: 500, damping: 14 }}>
                  <ServerIcon server={server} active className="size-10 text-sm" />
                </motion.span>
              )}
              <span className="min-w-0">
                <span className="block truncate">{t("workspace.invite.title", { server: server?.name ?? t("workspace.invite.thisServer") })}</span>
                {channel && (
                  <span className="flex items-center gap-1 text-sm font-bold text-muted-foreground">
                    <HashIcon className="size-3.5" />
                    {channel.name}
                  </span>
                )}
              </span>
            </span>
          }
          description={t("workspace.invite.about")}
        />
        <div className="flex flex-col gap-2">
          <span className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("workspace.invite.link")}</span>
          <div
            className={cn(
              "flex items-center gap-2 rounded-xl border bg-background/60 p-1.5 pl-3 transition-colors duration-300",
              copied && "border-emerald-500/60 bg-emerald-500/5",
            )}
          >
            <div className="relative min-w-0 flex-1 overflow-hidden">
              <AnimatePresence mode="popLayout" initial={false}>
                {invite ? (
                  <motion.span
                    key={invite.code}
                    initial={{ opacity: 0, y: 12, filter: "blur(4px)" }}
                    animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
                    exit={{ opacity: 0, y: -12, filter: "blur(4px)" }}
                    transition={SPRING}
                    className={cn("block truncate font-mono text-sm select-all", privateField)}
                    data-testid="invite-link"
                  >
                    {link}
                  </motion.span>
                ) : (
                  <motion.span key="loading" exit={{ opacity: 0 }} className="shimmer block h-5 w-4/5 rounded-md" />
                )}
              </AnimatePresence>
            </div>
            <motion.div whileTap={{ scale: 0.92 }}>
              <Button
                type="button"
                onClick={copyLink}
                disabled={!invite}
                className={cn("btn h-9 w-24 overflow-hidden rounded-lg font-bold transition-colors", copied && "bg-emerald-500 text-white hover:bg-emerald-500")}
              >
                <AnimatePresence mode="popLayout" initial={false}>
                  <motion.span
                    key={copied ? "copied" : "copy"}
                    initial={{ y: 16, opacity: 0, scale: 0.8 }}
                    animate={{ y: 0, opacity: 1, scale: 1 }}
                    exit={{ y: -16, opacity: 0, scale: 0.8 }}
                    transition={{ type: "spring", stiffness: 600, damping: 22 }}
                    className="flex items-center gap-1.5"
                  >
                    {copied ? <CheckIcon className="size-4" strokeWidth={3} /> : <CopyIcon className="size-4" />}
                    {copied ? t("workspace.invite.copied") : t("workspace.invite.copy")}
                  </motion.span>
                </AnimatePresence>
              </Button>
            </motion.div>
          </div>
          <AnimatePresence initial={false}>
            {error && (
              <motion.p
                initial={{ opacity: 0, height: 0 }}
                animate={{ opacity: 1, height: "auto" }}
                exit={{ opacity: 0, height: 0 }}
                className="text-sm text-destructive first-letter:uppercase"
              >
                {error}
              </motion.p>
            )}
          </AnimatePresence>
          <p className="text-xs text-muted-foreground">
            {invite ? (
              <>
                {until
                  ? limit > 0
                    ? t("workspace.invite.expiresInUses", { time: timeLeft(lang, until.getTime() - Date.now()), count: limit })
                    : t("workspace.invite.expiresIn", { time: timeLeft(lang, until.getTime() - Date.now()) })
                  : limit > 0
                    ? t("workspace.invite.neverUses", { count: limit })
                    : t("workspace.invite.never")}{" "}
              </>
            ) : null}
            <button
              type="button"
              onClick={() => setEditing((e) => !e)}
              aria-expanded={editing}
              className="inline-flex items-center gap-0.5 font-bold text-primary hover:underline"
            >
              {t("workspace.invite.edit")}
              <ChevronDownIcon className={cn("size-3.5 transition-transform duration-300", editing && "rotate-180")} />
            </button>
          </p>
        </div>
        <AnimatePresence initial={false}>
          {editing && (
            <motion.div
              initial={{ height: 0, opacity: 0 }}
              animate={{ height: "auto", opacity: 1 }}
              exit={{ height: 0, opacity: 0 }}
              transition={{ duration: 0.3, ease: [0.22, 1, 0.36, 1] }}
              className="overflow-hidden"
            >
              <div className="mt-4 flex flex-col gap-4 rounded-2xl border bg-muted/30 p-4">
                <div className="flex flex-col gap-2">
                  <span className="text-sm font-bold">{t("workspace.invite.expireAfter")}</span>
                  <Chips
                    label={t("workspace.invite.expireAfter")}
                    value={options.maxAgeSeconds}
                    options={EXPIRE_AFTER.map((o) => ({ value: o.value, label: t(o.label) }))}
                    onChange={(maxAgeSeconds) => setOptions((o) => ({ ...o, maxAgeSeconds }))}
                  />
                </div>
                <div className="flex flex-col gap-2">
                  <span className="text-sm font-bold">{t("workspace.invite.howMany")}</span>
                  <Chips
                    label={t("workspace.invite.howMany")}
                    value={options.maxUses}
                    options={MAX_USES.map((n) => ({ value: n, label: n ? t("workspace.invite.uses.count", { count: n }) : t("workspace.invite.uses.none") }))} onChange={(maxUses) => setOptions((o) => ({ ...o, maxUses }))} />
                </div>
                <Button type="button" onClick={() => void generate()} disabled={making} className="btn self-end rounded-xl font-bold">
                  {making ? <LoaderCircleIcon className="animate-spin" /> : <RefreshCwIcon className="transition-transform duration-500 group-hover:rotate-180" />}
                  {t("workspace.invite.generate")}
                </Button>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </DialogContent>
    </Dialog>
  );
}
