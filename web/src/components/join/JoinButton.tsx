import { ArrowRightIcon, BuildingIcon, CheckIcon, ClipboardPenIcon, HourglassIcon, LoaderCircleIcon, LockIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { AccountKind, ApplicationStatus, type Server } from "@/gen/fuwa/v1/types_pb";
import { joinServer, startServerSso, withdrawApplication } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { ProviderButton } from "@/components/Connect";
import { ApplicationDialog } from "@/components/join/ApplicationStatus";
import { ApplyDialog } from "@/components/join/ApplyDialog";
import { signedInForServer } from "@/lib/sso";
import { SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import { T, useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * The one way into a server, as Browse cards and invite pages show it: Join,
 * Apply to join, or where your application stands. Servers for waifu.dev
 * accounts only say so to accounts made on this instance. Servers that
 * require single sign-on send you through their provider first, which joins
 * on the way back (or comes back here to apply). Once you're in, it opens
 * the server.
 */
export function JoinButton({
  instanceKey,
  server,
  inviteCode = "",
  auto = false,
  onOpen,
  openLabel,
  size = "default",
}: {
  instanceKey: string;
  server: Server;
  inviteCode?: string;
  /** Joins straight away when it shows, for someone who just signed in to join. Never applies by itself. */
  auto?: boolean;
  /** Goes to the server, once you're in. */
  onOpen: (server: Server) => void;
  openLabel?: string;
  size?: "default" | "lg";
}) {
  const inst = useInstance(instanceKey);
  const { t } = useI18n();
  const member = !!inst?.servers.some((s) => s.id === server.id);
  const applied = inst?.applied[server.id];
  const local = inst?.me && inst.me.kind !== AccountKind.LINKED;
  const join = useAction(joinServer);
  const sso = useAction(startServerSso);
  const withdraw = useAction(withdrawApplication);
  const [joined, setJoined] = useState(false);
  const [applying, setApplying] = useState(false);
  const [looking, setLooking] = useState(false);
  const tall = size === "lg" ? "h-11" : "h-10";

  // Nothing more happens once it's gone (an auto join can finish after the page moves on).
  const alive = useRef(true);
  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  async function go() {
    const s = await join.go(instanceKey, server.id, inviteCode);
    if (!s || !alive.current) return;
    setJoined(true);
    setTimeout(() => onOpen(s), 650);
  }

  const kind = kindOf({
    joined,
    member,
    linkedOnly: !!server.linkedOnly && !!local,
    waiting: applied?.status === ApplicationStatus.PENDING,
    sso: server.ssoRequired && !signedInForServer(instanceKey, server.id),
    applications: server.applications,
  });

  const tried = useRef(false);
  useEffect(() => {
    if (auto && kind === "join" && !tried.current) {
      tried.current = true;
      void go();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, kind]);

  let button: ReactNode;
  let note: ReactNode = null;
  if (kind === "open") {
    button = (
      <Button size={size} variant="outline" onClick={() => onOpen(server)} className={cn("group w-full rounded-xl font-bold", tall)}>
        {openLabel ?? t("join.button.open")} <ArrowRightIcon className="transition-transform group-hover:translate-x-1" />
      </Button>
    );
  } else if (kind === "linked-only") {
    button = <LinkedOnlyButton size={size} tall={tall} />;
    note = <LinkedOnlyNote sso={inst?.me?.kind === AccountKind.SSO} />;
  } else if (kind === "sso") {
    button = <SsoButton server={server} onGo={(next) => sso.go(instanceKey, server.id, { join: !server.applications, inviteCode, next })} />;
    note = <SsoNote server={server} />;
  } else if (kind === "waiting") {
    button = <WaitingBadge tall={tall} />;
    note = (
      <WaitingNote
        withdrawing={withdraw.pending}
        onLook={() => setLooking(true)}
        onWithdraw={async () => {
          if ((await withdraw.go(instanceKey, server.id)) !== undefined) toast(t("join.withdrawn", { server: server.name }));
        }}
      />
    );
  } else if (kind === "apply") {
    const again = applied?.status === ApplicationStatus.REJECTED;
    button = <ApplyButton size={size} tall={tall} again={again} onApply={() => setApplying(true)} />;
    if (again) note = <TurnedDown reason={applied!.reason} />;
  } else {
    button = <JoinNow size={size} tall={tall} name={server.name} pending={join.pending} joined={joined} onJoin={() => void go()} />;
  }

  const error = join.error ?? withdraw.error ?? sso.error;
  return (
    <div className="flex flex-col gap-2">
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.div
          key={kind === "joined" ? "join" : kind}
          initial={{ opacity: 0, scale: 0.94 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.94 }}
          transition={SPRING}
        >
          {button}
        </motion.div>
      </AnimatePresence>
      <JoinLine kind={kind} note={note} error={error} />
      <ApplyDialog open={applying} onOpenChange={setApplying} instanceKey={instanceKey} server={server} inviteCode={inviteCode} />
      <ApplicationDialog
        open={looking}
        onOpenChange={setLooking}
        instanceKey={instanceKey}
        server={server}
        onOpen={() => {
          setLooking(false);
          onOpen(server);
        }}
        onApplyAgain={() => {
          setLooking(false);
          setApplying(true);
        }}
      />
    </div>
  );
}

type Kind = "joined" | "open" | "linked-only" | "waiting" | "sso" | "apply" | "join";

/** Which way in the button offers, first match wins. */
function kindOf(at: { joined: boolean; member: boolean; linkedOnly: boolean; waiting: boolean; sso: boolean; applications: boolean }): Kind {
  if (at.joined) return "joined";
  if (at.member) return "open";
  if (at.linkedOnly) return "linked-only";
  if (at.waiting) return "waiting";
  if (at.sso) return "sso";
  return at.applications ? "apply" : "join";
}

/** Signing in through the server's provider, to join on the way back (or to come back and apply). */
function SsoButton({ server, onGo }: { server: Server; onGo: (next: string | null) => Promise<unknown> }) {
  const { t } = useI18n();
  const name = server.ssoName || t("connect.provider.yourOrganization");
  return (
    <ProviderButton
      name={name}
      label={server.applications ? t("join.button.signInToApply") : t("join.button.joinWith", { name })}
      icon={<BuildingIcon className="size-5 transition-transform duration-500 group-hover:-translate-y-0.5 group-hover:scale-110" />}
      onGo={() => onGo(server.applications ? window.location.pathname : null)}
      error={null}
      testId="sso-join"
    />
  );
}

/** Where the provider is, so people know who sees them sign in. */
function SsoNote({ server }: { server: Server }) {
  const { t } = useI18n();
  const name = server.ssoName || t("connect.provider.yourOrganization");
  if (server.ssoHost) return <T k="join.button.ssoNoteAt" values={{ server: server.name, name, host: <b className="text-foreground">{server.ssoHost}</b> }} />;
  return t("join.button.ssoNote", { server: server.name, name });
}

/** An application waiting to be read. */
function WaitingBadge({ tall }: { tall: string }) {
  const { t } = useI18n();
  return (
    <span
      className={cn("flex w-full items-center justify-center gap-2 rounded-xl bg-amber-500/15 px-4 text-sm font-bold text-amber-600 dark:text-amber-400", tall)}
    >
      <HourglassIcon className="size-4 animate-[flip_3s_ease-in-out_infinite]" /> {t("join.button.waiting")}
    </span>
  );
}

/** Seeing where an application stands, or taking it back. */
function WaitingNote({ withdrawing, onLook, onWithdraw }: { withdrawing: boolean; onLook: () => void; onWithdraw: () => void }) {
  const { t } = useI18n();
  return (
    <T
      k="join.button.waitingNote"
      values={{
        look: (
          <button type="button" onClick={onLook} className="font-bold text-foreground underline-offset-2 hover:underline">
            {t("join.seeWhereItStands")}
          </button>
        ),
        withdraw: (
          <button type="button" disabled={withdrawing} onClick={onWithdraw} className="font-bold text-foreground underline-offset-2 hover:underline">
            {t("join.button.takeItBack")}
          </button>
        ),
      }}
    />
  );
}

/** The last application was turned down, and why when they said. */
function TurnedDown({ reason }: { reason: string }) {
  const { t } = useI18n();
  return reason ? t("join.button.lastTurnedDownBecause", { reason }) : t("join.button.lastTurnedDown");
}

/** Join, a spinner while it goes, then a tick. */
function JoinNow({
  size,
  tall,
  name,
  pending,
  joined,
  onJoin,
}: {
  size: "default" | "lg";
  tall: string;
  name: string;
  pending: boolean;
  joined: boolean;
  onJoin: () => void;
}) {
  const { t } = useI18n();
  return (
    <Button
      size={size}
      onClick={onJoin}
      disabled={pending || joined}
      className={cn("w-full rounded-xl font-bold", tall, joined ? "bg-emerald-500 text-white hover:bg-emerald-500" : "btn")}
    >
      <AnimatePresence mode="wait" initial={false}>
        <motion.span
          key={joined ? "done" : pending ? "busy" : "join"}
          initial={{ opacity: 0, y: 8, scale: 0.9 }}
          animate={{ opacity: 1, y: 0, scale: 1 }}
          exit={{ opacity: 0, y: -8, scale: 0.9 }}
          transition={SPRING}
          className="flex items-center gap-2"
        >
          {joined ? <CheckIcon strokeWidth={3} /> : pending ? <LoaderCircleIcon className="animate-spin" /> : null}
          {joined ? t("join.button.joined") : size === "lg" ? t("join.button.joinServer", { server: name }) : t("join.button.join")}
        </motion.span>
      </AnimatePresence>
    </Button>
  );
}

/** Why someone can't join a server for waifu.dev accounts, by the kind of account they have. */
function LinkedOnlyButton({ size, tall }: { size: "default" | "lg"; tall: string }) {
  const { t } = useI18n();
  return (
    <Button size={size} variant="outline" disabled className={cn("w-full rounded-xl font-bold", tall)}>
      <LockIcon /> {t("join.button.linkedOnly")}
    </Button>
  );
}

function LinkedOnlyNote({ sso }: { sso: boolean }) {
  const { t } = useI18n();
  return sso ? t("join.button.linkedOnlySso") : t("join.button.linkedOnlyLocal");
}

/** Apply to join, or apply again after being turned down. */
function ApplyButton({ size, tall, again, onApply }: { size: "default" | "lg"; tall: string; again: boolean; onApply: () => void }) {
  const { t } = useI18n();
  return (
    <Button size={size} onClick={onApply} className={cn("btn w-full rounded-xl font-bold", tall)}>
      <ClipboardPenIcon /> {again ? t("join.applyAgain") : t("join.apply")}
    </Button>
  );
}

/** The line under the button: what went wrong (with a shake), or a note about the way in. */
function JoinLine({ kind, note, error }: { kind: Kind; note: ReactNode; error: string | null }) {
  return (
    <AnimatePresence initial={false}>
      {(note || error) && (
        <motion.p
          key={error ? "error" : kind}
          initial={{ opacity: 0, height: 0 }}
          animate={{ opacity: 1, height: "auto", x: error ? [0, -6, 6, -3, 3, 0] : 0 }}
          exit={{ opacity: 0, height: 0 }}
          className={cn("text-center text-xs break-words", error ? "text-destructive first-letter:uppercase" : "text-muted-foreground")}
        >
          {error ?? note}
        </motion.p>
      )}
    </AnimatePresence>
  );
}
