import * as Popover from "@radix-ui/react-popover";
import { useNavigate } from "@tanstack/react-router";
import { DoorOpenIcon, GavelIcon, HourglassIcon, LoaderCircleIcon, LockKeyholeIcon, MessageCircleIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, m as motion, useIsPresent } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { loadProfile, lookUpAgentHere, run } from "@/fuwa/actions";
import { openConversation } from "@/fuwa/dms";
import { useFuwa } from "@/fuwa/store";
import { byId } from "@/fuwa/hooks";
import { MemberRoles } from "@/components/MemberRoles";
import { ModerateDialog, useModeration, type ModAction } from "@/components/ModerateDialog";
import { ProfileCard } from "@/components/ProfileCard";
import { ProfileGive } from "@/components/economy/ProfileGive";
import { ProfileReport } from "@/components/modview/ProfileReport";
import { FriendActions } from "@/components/friends/FriendActions";
import { AddAgentMenu, useManagedServers } from "@/components/agents/AddAgent";
import { instanceHas } from "@/lib/compat";
import { isAgent, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { toast } from "@/lib/ui";
import type { Key } from "@/i18n/i18n";
import { useI18n } from "@/i18n/react";
import { useMediaQuery } from "@/lib/use-media-query";

const MOD_ACTIONS: { action: ModAction; label: Key; icon: LucideIcon; hover: string }[] = [
  { action: "timeout", label: "workspace.popover.timeout", icon: HourglassIcon, hover: "group-hover:rotate-180" },
  { action: "kick", label: "workspace.popover.kick", icon: DoorOpenIcon, hover: "group-hover:translate-x-0.5" },
  { action: "ban", label: "workspace.popover.ban", icon: GavelIcon, hover: "group-hover:-rotate-45" },
];

/** How long a profile may take before the card shows a placeholder for it. */
const SLOW_MS = 500;

/**
 * Loads someone's profile fresh each time the card opens; what's kept shows
 * meanwhile. True while it's slow to come, so the card can hold a place for it.
 */
function useProfileLoad(open: boolean, instanceKey: string, user: User | undefined) {
  const [failed, setFailed] = useState(false);
  // The bio's placeholder waits a moment: a quick load would flash it and pull it away again.
  const [slow, setSlow] = useState(false);

  useEffect(() => {
    if (!open || !user) return;
    setFailed(false);
    setSlow(false);
    const timer = setTimeout(() => setSlow(true), SLOW_MS);
    run(loadProfile(instanceKey, user.id))
      .catch(() => setFailed(true))
      .finally(() => clearTimeout(timer));
    return () => clearTimeout(timer);
  }, [open, instanceKey, user]);

  return !failed && slow;
}

/**
 * Opens someone's profile card from whatever you click on them: a name or
 * avatar in chat, a row in the member list. The card shows what's already
 * known at once and fills in the bio and banner as they load.
 *
 * One is mounted per author in chat and per member row, so while closed it
 * holds only its open state and the trigger: everything the card reads from
 * the store (the profile, what you may do to them, the clock) lives in
 * `ProfileContent`, mounted only while the card is open or closing.
 */
export function ProfilePopover({
  instanceKey,
  user,
  member,
  side = "right",
  children,
}: {
  instanceKey: string;
  user: User | undefined;
  member?: Member;
  side?: "right" | "left" | "top" | "bottom";
  children: ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const [moderating, setModerating] = useState<ModAction | null>(null);
  // The dialog mounts the first time it's wanted and stays, so its closing animation plays.
  const [moderated, setModerated] = useState(false);
  if (!user) return <>{children}</>;
  return (
    <>
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <AnimatePresence>
        {open && (
          <ProfileContent
            key="card"
            instanceKey={instanceKey}
            user={user}
            member={member}
            side={side}
            onClose={() => setOpen(false)}
            onModerate={(action) => {
              setOpen(false);
              setModerated(true);
              setModerating(action);
            }}
          />
        )}
      </AnimatePresence>
    </Popover.Root>
    {member && moderated && (
      <ModerateDialog instanceKey={instanceKey} serverId={member.serverId} member={moderating ? member : null} action={moderating} onClose={() => setModerating(null)} />
    )}
    </>
  );
}

/** The open card: mounted by `ProfilePopover` only while it's open (and while it animates away). */
function ProfileContent({
  instanceKey,
  user,
  member,
  side,
  onClose,
  onModerate,
}: {
  instanceKey: string;
  user: User;
  member?: Member;
  side: "right" | "left" | "top" | "bottom";
  onClose: () => void;
  onModerate: (action: ModAction) => void;
}) {
  // False once it's closing, so a load isn't started again on the way out.
  const present = useIsPresent();
  // A phone has no room beside a name for the card, so it opens under it and slides to fit.
  const roomy = useMediaQuery("(min-width: 768px)");
  const profile = useFuwa((s) => s.instances[instanceKey]?.profiles[user.id]);
  const me = useFuwa((s) => s.instances[instanceKey]?.me?.id === user.id);
  const loading = useProfileLoad(present, instanceKey, user) && !profile;
  const allowed = useModeration(instanceKey, member?.serverId ?? "", member);
  const canModerate = allowed.timeout || allowed.kick || allowed.ban;
  const owner = useFuwa((s) => !!member && byId(s.instances[instanceKey]?.servers, member.serverId)?.ownerId === user.id);
  const now = useNow();
  // Anyone signed in can write to someone else privately, where the instance and this browser can. Agents have no DMs.
  const canMessage = useFuwa((s) => {
    const status = s.instances[instanceKey]?.dms.status;
    return !me && !isAgent(user) && (status === "ready" || status === "starting");
  });
  // Friends are people's, on instances that have them.
  const canFriend = useFuwa((s) => !me && !isAgent(user) && s.instances[instanceKey]?.friends.status === "ready");
  return (
    <Popover.Portal forceMount>
      <Popover.Content forceMount side={roomy ? side : "bottom"} align="start" sideOffset={10} collisionPadding={12} className="scroll-thin z-50 max-h-(--radix-popover-content-available-height) overflow-y-auto outline-none">
        <motion.div
          initial={{ opacity: 0, scale: 0.92, y: 6 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.95, y: 4 }}
          transition={{ type: "spring", stiffness: 520, damping: 32 }}
          style={{ transformOrigin: "var(--radix-popover-content-transform-origin)" }}
        >
          <ProfileCard
            user={user}
            profile={profile}
            member={member}
            owner={owner}
            roles={member && <MemberRoles instanceKey={instanceKey} member={member} />}
            me={me}
            instanceKey={instanceKey}
            loading={loading}
            className="w-[19rem] max-w-[calc(100vw-1.5rem)]"
          />
          {canMessage && <MessageButton instanceKey={instanceKey} user={user} onOpened={onClose} />}
          {isAgent(user) && <AgentAdd instanceKey={instanceKey} user={user} />}
          {canFriend && (
            <motion.div
              initial={{ opacity: 0, y: -6 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.065 }}
              className="mt-2 rounded-2xl border bg-popover p-1.5 shadow-lg"
            >
              <FriendActions instanceKey={instanceKey} user={user} />
            </motion.div>
          )}
          {canModerate && <ModActions allowed={allowed} timedOut={!!timedOutUntil(member, now)} onPick={onModerate} />}
          {member && <ProfileGive instanceKey={instanceKey} user={user} member={member} onClose={onClose} />}
          {member && <ProfileReport instanceKey={instanceKey} user={user} member={member} me={me} onClose={onClose} />}
        </motion.div>
      </Popover.Content>
    </Popover.Portal>
  );
}

/** "Message": opens a private conversation with them. */
function MessageButton({ instanceKey, user, onOpened }: { instanceKey: string; user: User; onOpened: () => void }) {
  const { t } = useI18n();
  const [opening, setOpening] = useState(false);
  const navigate = useNavigate();

  async function message() {
    if (opening) return;
    setOpening(true);
    try {
      const conversation = await run(openConversation(instanceKey, user.id));
      onOpened();
      void navigate({ to: "/$instance/dm/$conversation", params: { instance: instanceKey, conversation } });
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setOpening(false);
    }
  }

  return (
    <motion.div
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.05 }}
      className="mt-2 rounded-2xl border bg-popover p-1.5 shadow-lg"
    >
      <motion.button
        type="button"
        onClick={() => void message()}
        disabled={opening}
        whileTap={{ scale: 0.96 }}
        className="group relative flex w-full items-center justify-center gap-2 overflow-hidden rounded-xl bg-primary px-3 py-2 text-sm font-bold text-primary-foreground transition hover:brightness-110 disabled:opacity-80"
      >
        <span aria-hidden className="shine pointer-events-none absolute inset-0" />
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span
            key={opening ? "opening" : "idle"}
            initial={{ scale: 0.4, opacity: 0, rotate: -30 }}
            animate={{ scale: 1, opacity: 1, rotate: 0 }}
            exit={{ scale: 0.4, opacity: 0 }}
            transition={{ type: "spring", stiffness: 600, damping: 20 }}
            className="grid place-items-center"
          >
            {opening ? (
              <LoaderCircleIcon className="size-4 animate-spin" />
            ) : (
              <MessageCircleIcon className="size-4 transition-transform duration-300 group-hover:-rotate-12 group-hover:scale-110" />
            )}
          </motion.span>
        </AnimatePresence>
        {t("workspace.popover.message")}
        <LockKeyholeIcon
          aria-label={t("workspace.popover.encrypted")}
          className="size-3.5 opacity-80 transition-transform duration-300 group-hover:translate-x-0.5"
        />
      </motion.button>
    </motion.div>
  );
}

/**
 * "Add to server" on an agent's card, for someone who manages a server here
 * and may add it (it's public, or theirs): the instance says which.
 */
function AgentAdd({ instanceKey, user }: { instanceKey: string; user: User }) {
  const here = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "agent-discovery"));
  const managed = useManagedServers(instanceKey).length > 0;
  const [addable, setAddable] = useState(false);

  useEffect(() => {
    if (!here || !managed) return;
    let cancelled = false;
    run(lookUpAgentHere(instanceKey, { agentId: user.id })).then(
      () => !cancelled && setAddable(true),
      () => !cancelled && setAddable(false),
    );
    return () => {
      cancelled = true;
    };
  }, [here, managed, instanceKey, user.id]);

  if (!here || !managed || !addable) return null;
  return (
    <motion.div
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.05 }}
      className="mt-2 rounded-2xl border bg-popover p-1.5 shadow-lg"
    >
      <AddAgentMenu instanceKey={instanceKey} agent={user} className="h-9 w-full" />
    </motion.div>
  );
}

/** Time out, kick and ban, as far as you're allowed. */
function ModActions({
  allowed,
  timedOut,
  onPick,
}: {
  allowed: Record<ModAction, boolean>;
  timedOut: boolean;
  onPick: (action: ModAction) => void;
}) {
  const { t } = useI18n();
  return (
    <motion.div
      initial={{ opacity: 0, y: -6 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.08 }}
      className="mt-2 flex gap-1.5 rounded-2xl border bg-popover p-1.5 shadow-lg"
    >
      {MOD_ACTIONS.filter(({ action }) => allowed[action]).map(({ action, label, icon: Icon, hover }) => (
        <button
          key={action}
          type="button"
          onClick={() => onPick(action)}
          className={
            "group flex flex-1 items-center justify-center gap-1.5 rounded-xl px-2 py-1.5 text-xs font-bold text-muted-foreground transition hover:bg-destructive/10 hover:text-destructive active:scale-95"
          }
        >
          <Icon className={`size-3.5 transition-transform duration-300 ${hover}`} />
          {action === "timeout" && timedOut ? t("workspace.popover.timedOut") : t(label)}
        </button>
      ))}
    </motion.div>
  );
}
