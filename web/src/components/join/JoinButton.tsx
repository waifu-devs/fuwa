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
import { SPRING } from "@/components/motion";
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

  async function go() {
    const s = await join.go(instanceKey, server.id, inviteCode);
    if (!s) return;
    setJoined(true);
    setTimeout(() => onOpen(s), 650);
  }

  const kind = joined
    ? "joined"
    : member
      ? "open"
      : server.linkedOnly && local
        ? "linked-only"
        : applied?.status === ApplicationStatus.PENDING
          ? "waiting"
          : server.ssoRequired && !signedInForServer(instanceKey, server.id)
            ? "sso"
          : server.applications
            ? "apply"
            : "join";

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
    button = (
      <Button size={size} variant="outline" disabled className={cn("w-full rounded-xl font-bold", tall)}>
        <LockIcon /> {t("join.button.linkedOnly")}
      </Button>
    );
    note =
      inst?.me?.kind === AccountKind.SSO
        ? t("join.button.linkedOnlySso")
        : t("join.button.linkedOnlyLocal");
  } else if (kind === "sso") {
    const name = server.ssoName || t("connect.provider.yourOrganization");
    button = (
      <ProviderButton
        name={name}
        label={server.applications ? t("join.button.signInToApply") : t("join.button.joinWith", { name })}
        icon={<BuildingIcon className="size-5 transition-transform duration-500 group-hover:-translate-y-0.5 group-hover:scale-110" />}
        onGo={() =>
          sso.go(instanceKey, server.id, {
            join: !server.applications,
            inviteCode,
            next: server.applications ? window.location.pathname : null,
          })
        }
        error={null}
        testId="sso-join"
      />
    );
    note = server.ssoHost ? (
      <T k="join.button.ssoNoteAt" values={{ server: server.name, name, host: <b className="text-foreground">{server.ssoHost}</b> }} />
    ) : (
      t("join.button.ssoNote", { server: server.name, name })
    );
  } else if (kind === "waiting") {
    button = (
      <span className={cn("flex w-full items-center justify-center gap-2 rounded-xl bg-amber-500/15 px-4 text-sm font-bold text-amber-600 dark:text-amber-400", tall)}>
        <HourglassIcon className="size-4 animate-[flip_3s_ease-in-out_infinite]" /> {t("join.button.waiting")}
      </span>
    );
    note = (
      <T
        k="join.button.waitingNote"
        values={{
          look: (
            <button type="button" onClick={() => setLooking(true)} className="font-bold text-foreground underline-offset-2 hover:underline">
              {t("join.seeWhereItStands")}
            </button>
          ),
          withdraw: (
            <button
              type="button"
              disabled={withdraw.pending}
              onClick={async () => {
                if ((await withdraw.go(instanceKey, server.id)) !== undefined) toast(t("join.withdrawn", { server: server.name }));
              }}
              className="font-bold text-foreground underline-offset-2 hover:underline"
            >
              {t("join.button.takeItBack")}
            </button>
          ),
        }}
      />
    );
  } else if (kind === "apply") {
    const again = applied?.status === ApplicationStatus.REJECTED;
    button = (
      <Button size={size} onClick={() => setApplying(true)} className={cn("btn w-full rounded-xl font-bold", tall)}>
        <ClipboardPenIcon /> {again ? t("join.applyAgain") : t("join.apply")}
      </Button>
    );
    note = again
      ? applied!.reason
        ? t("join.button.lastTurnedDownBecause", { reason: applied!.reason })
        : t("join.button.lastTurnedDown")
      : null;
  } else {
    button = (
      <Button
        size={size}
        onClick={() => void go()}
        disabled={join.pending || joined}
        className={cn("w-full rounded-xl font-bold", tall, joined ? "bg-emerald-500 text-white hover:bg-emerald-500" : "btn")}
      >
        <AnimatePresence mode="wait" initial={false}>
          <motion.span
            key={joined ? "done" : join.pending ? "busy" : "join"}
            initial={{ opacity: 0, y: 8, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -8, scale: 0.9 }}
            transition={SPRING}
            className="flex items-center gap-2"
          >
            {joined ? <CheckIcon strokeWidth={3} /> : join.pending ? <LoaderCircleIcon className="animate-spin" /> : null}
            {joined ? t("join.button.joined") : size === "lg" ? t("join.button.joinServer", { server: server.name }) : t("join.button.join")}
          </motion.span>
        </AnimatePresence>
      </Button>
    );
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
