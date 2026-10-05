import { CheckIcon, CopyIcon, HashIcon, InfinityIcon, LinkIcon, PlusIcon, ServerIcon as ServerGlyph, TimerIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useEffect, useState } from "react";
import { ChannelType, Permission, type Invite, type User } from "@/gen/fuwa/v1/types_pb";
import { deleteInvite, listInvites, run } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useInstance } from "@/fuwa/hooks";
import { InviteDialog, publicBase } from "@/components/dialogs/InviteDialog";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/lib/motion";
import { Private } from "@/components/Private";
import { Button } from "@/components/ui/button";
import { displayName, formatLeft, formatStamp } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { expiresAt, inviteLink, works } from "@/lib/invites";
import { useNow } from "@/lib/notifications";
import { has, hasIn } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * The server's invite links that still work: who made each, where it leads,
 * how many it has let in and how long it has left. With Manage Server you see
 * and can revoke everyone's; otherwise just your own.
 */
export function Invites({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const access = useAccess(instanceKey, serverId);
  const channels = inst?.channels[serverId] ?? [];
  const [invites, setInvites] = useState<Invite[] | null>(null);
  const [inviters, setInviters] = useState<Record<string, User>>({});
  const [error, setError] = useState<string | null>(null);
  const [making, setMaking] = useState<string | null>(null);
  const now = useNow(1000);
  const manager = has(access, Permission.MANAGE_SERVER);

  const load = useCallback(
    () =>
      run(listInvites(instanceKey, serverId)).then(
        (r) => {
          setInvites(r.invites);
          setInviters((known) => ({ ...known, ...Object.fromEntries(r.inviters.map((u) => [u.id, u])) }));
        },
        (err: FuwaError) => setError(err.message),
      ),
    [instanceKey, serverId],
  );
  useEffect(() => void load(), [load]);

  // Where "Create invite" leads: the server if you may invite to it, else the first channel you may.
  const target = has(access, Permission.CREATE_INVITE)
    ? ""
    : (channels.find((c) => (c.type === ChannelType.TEXT || c.type === ChannelType.ANNOUNCEMENT) && hasIn(access, c.id, Permission.CREATE_INVITE))?.id ??
      null);

  async function revoke(invite: Invite) {
    setInvites((list) => list?.filter((i) => i.code !== invite.code) ?? null);
    await run(deleteInvite(instanceKey, serverId, invite.code)).catch((err: FuwaError) => {
      toast(err.message);
      void load();
    });
  }

  const live = invites?.filter((i) => works(i, now));
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="max-w-md text-sm text-muted-foreground">
          {manager ? t("serversettings.invites.everyone") : t("serversettings.invites.own")}
        </p>
        {target !== null && (
          <Button onClick={() => setMaking(target)} className="btn rounded-xl font-bold">
            <PlusIcon /> {t("serversettings.invites.create")}
          </Button>
        )}
      </div>
      {error ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{error}</p>
      ) : !live ? (
        <div className="flex flex-col gap-2">
          {[0, 1, 2].map((n) => (
            <div key={n} className="shimmer h-16 rounded-2xl" />
          ))}
        </div>
      ) : live.length === 0 ? (
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          className="flex flex-col items-center gap-2 rounded-3xl border border-dashed p-10 text-center"
        >
          <span className="float grid size-12 place-items-center rounded-full bg-primary/15 text-primary">
            <LinkIcon className="size-6" />
          </span>
          <p className="font-bold">{t("serversettings.invites.none")}</p>
          <p className="max-w-sm text-sm text-muted-foreground">{t("serversettings.invites.noneHint")}</p>
        </motion.div>
      ) : (
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false} mode="popLayout">
            {live.map((invite, n) => (
              <Row
                key={invite.code}
                index={n}
                invite={invite}
                inviter={inviters[invite.inviterId]}
                channelName={channels.find((c) => c.id === invite.channelId)?.name}
                link={inviteLink(publicBase(instanceKey), invite.code)}
                now={now}
                onRevoke={() => void revoke(invite)}
              />
            ))}
          </AnimatePresence>
        </ul>
      )}
      <InviteDialog
        open={making !== null}
        onOpenChange={(open) => {
          if (open) return;
          setMaking(null);
          void load();
        }}
        instanceKey={instanceKey}
        serverId={serverId}
        channelId={making ?? ""}
      />
    </div>
  );
}

function Row({
  invite,
  inviter,
  channelName,
  link,
  now,
  index,
  onRevoke,
}: {
  invite: Invite;
  inviter: User | undefined;
  channelName: string | undefined;
  link: string;
  now: number;
  index: number;
  onRevoke: () => void;
}) {
  const { t } = useI18n();
  return (
    <motion.li
      layout
      initial={{ opacity: 0, x: -12 }}
      animate={{ opacity: 1, x: 0, transition: { ...SPRING, delay: Math.min(index, 10) * 0.03 } }}
      exit={{ opacity: 0, x: 24, scale: 0.96, transition: { duration: 0.2 } }}
      className="group flex flex-wrap items-center gap-x-4 gap-y-2 rounded-2xl border bg-background/40 p-3 transition-colors hover:border-primary/30"
      data-invite={invite.code}
    >
      <span className="flex min-w-0 flex-1 basis-48 items-center gap-2.5">
        <UserAvatar user={inviter} className="size-9" />
        <span className="min-w-0">
          <span className="block truncate text-sm font-bold">{displayName(inviter)}</span>
          <span className="block truncate font-mono text-xs text-muted-foreground">
            <Private text={invite.code} kind="secret" />
          </span>
        </span>
      </span>
      <span className="flex basis-32 items-center gap-1 text-sm text-muted-foreground" title={channelName ? t("serversettings.invites.opens", { channel: channelName }) : t("serversettings.invites.opensServer")}>
        {invite.channelId ? <HashIcon className="size-3.5 shrink-0" /> : <ServerGlyph className="size-3.5 shrink-0" />}
        <span className="truncate">{invite.channelId ? (channelName ?? t("serversettings.invites.deletedChannel")) : t("serversettings.invites.server")}</span>
      </span>
      <InviteUses invite={invite} />
      <InviteExpiry invite={invite} now={now} />
      <span className="ml-auto flex items-center gap-1">
        <CopyLinkButton link={link} />
        <button
          type="button"
          onClick={onRevoke}
          aria-label={t("serversettings.invites.revoke")}
          title={t("serversettings.invites.revoke")}
          className="grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:rotate-90 hover:bg-destructive/10 hover:text-destructive active:scale-90"
        >
          <XIcon className="size-4" />
        </button>
      </span>
    </motion.li>
  );
}

/** How many times it was used, out of how many, with a bar. */
function InviteUses({ invite }: { invite: Invite }) {
  const { t } = useI18n();
  const share = invite.maxUses ? invite.uses / invite.maxUses : 0;
  return (
    <span className="flex basis-24 flex-col gap-1" title={
        invite.maxUses
          ? t("serversettings.invites.usesOf", { uses: invite.uses, count: invite.maxUses })
          : t("serversettings.invites.usesNoLimit", { count: invite.uses })
      }>
      <span className="flex items-center gap-1 text-sm tabular-nums">
        <b>{invite.uses}</b>
        <span className="text-muted-foreground">/</span>
        {invite.maxUses ? <span className="text-muted-foreground">{invite.maxUses}</span> : <InfinityIcon className="size-3.5 text-muted-foreground" />}
      </span>
      <span className="h-1 overflow-hidden rounded-full bg-muted">
        <motion.span
          className="block h-full rounded-full bg-primary"
          initial={{ x: "-100%" }}
          animate={{ x: invite.maxUses ? `${share * 100 - 100}%` : "0%", opacity: invite.maxUses ? 1 : 0.25 }}
          transition={{ duration: 0.8, ease: [0.22, 1, 0.36, 1] }}
        />
      </span>
    </span>
  );
}

/** How long until it expires, amber in its last hour. */
function InviteExpiry({ invite, now }: { invite: Invite; now: number }) {
  const lang = useI18n();
  const { t } = lang;
  const until = expiresAt(invite);
  const left = until ? until.getTime() - now : null;
  return (
    <span
      className={cn("flex basis-24 items-center gap-1 text-sm tabular-nums", left !== null && left < 3_600_000 ? "text-amber-500" : "text-muted-foreground")}
      title={until ? t("serversettings.invites.expires", { time: formatStamp(until) }) : t("serversettings.invites.neverExpires")}
    >
      {left !== null ? <TimerIcon className="size-3.5" /> : <InfinityIcon className="size-3.5" />}
      {left !== null ? formatLeft(lang, left) : t("serversettings.shared.never")}
    </span>
  );
}

/** Copies the invite's link, ticking for a moment once it's copied. */
function CopyLinkButton({ link }: { link: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  function copyLink() {
    void navigator.clipboard?.writeText(link).then(
      () => {
        setCopied(true);
        setTimeout(() => setCopied(false), 1400);
      },
      () => toast(t("serversettings.invites.copyFailed")),
    );
  }
  return (
    <button
      type="button"
      onClick={copyLink}
      aria-label={t("serversettings.invites.copy")}
      title={t("serversettings.invites.copy")}
      className={cn(
        "grid size-8 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground active:scale-90",
        copied && "text-emerald-500 hover:text-emerald-500",
      )}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span key={String(copied)} initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={SPRING}>
          {copied ? <CheckIcon className="size-4" strokeWidth={3} /> : <CopyIcon className="size-4" />}
        </motion.span>
      </AnimatePresence>
    </button>
  );
}
