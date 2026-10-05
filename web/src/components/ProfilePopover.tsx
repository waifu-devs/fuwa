import * as Popover from "@radix-ui/react-popover";
import { useNavigate } from "@tanstack/react-router";
import { DoorOpenIcon, GavelIcon, HourglassIcon, LoaderCircleIcon, LockKeyholeIcon, MessageCircleIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { loadProfile, run } from "@/fuwa/actions";
import { openConversation } from "@/fuwa/dms";
import { useFuwa } from "@/fuwa/store";
import { MemberRoles } from "@/components/MemberRoles";
import { ModerateDialog, useModeration, type ModAction } from "@/components/ModerateDialog";
import { ProfileCard } from "@/components/ProfileCard";
import { FriendActions } from "@/components/friends/FriendActions";
import { isAgent, timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { toast } from "@/lib/ui";
import { useMediaQuery } from "@/lib/use-media-query";

const MOD_ACTIONS: { action: ModAction; label: string; icon: LucideIcon; hover: string }[] = [
  { action: "timeout", label: "Time out", icon: HourglassIcon, hover: "group-hover:rotate-180" },
  { action: "kick", label: "Kick", icon: DoorOpenIcon, hover: "group-hover:translate-x-0.5" },
  { action: "ban", label: "Ban", icon: GavelIcon, hover: "group-hover:-rotate-45" },
];

/** How long a profile may take before the card shows a placeholder for it. */
const SLOW_MS = 500;

/**
 * Opens someone's profile card from whatever you click on them: a name or
 * avatar in chat, a row in the member list. The card shows what's already
 * known at once and fills in the bio and banner as they load.
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
  // A phone has no room beside a name for the card, so it opens under it and slides to fit.
  const roomy = useMediaQuery("(min-width: 768px)");
  const profile = useFuwa((s) => (user ? s.instances[instanceKey]?.profiles[user.id] : undefined));
  const me = useFuwa((s) => s.instances[instanceKey]?.me?.id === user?.id);
  const [failed, setFailed] = useState(false);
  // The bio's placeholder waits a moment: a quick load would flash it and pull it away again.
  const [slow, setSlow] = useState(false);
  const allowed = useModeration(instanceKey, member?.serverId ?? "", member);
  const canModerate = allowed.timeout || allowed.kick || allowed.ban;
  const owner = useFuwa((s) => !!member && s.instances[instanceKey]?.servers.find((x) => x.id === member.serverId)?.ownerId === user?.id);
  const [moderating, setModerating] = useState<ModAction | null>(null);
  const now = useNow();
  // Anyone signed in can write to someone else privately, where the instance and this browser can. Agents have no DMs.
  const canMessage = useFuwa((s) => {
    const status = s.instances[instanceKey]?.dms.status;
    return !me && !isAgent(user) && (status === "ready" || status === "starting");
  });
  // Friends are people's, on instances that have them.
  const canFriend = useFuwa((s) => !me && !isAgent(user) && s.instances[instanceKey]?.friends.status === "ready");
  const [opening, setOpening] = useState(false);
  const navigate = useNavigate();

  async function message() {
    if (!user || opening) return;
    setOpening(true);
    try {
      const conversation = await run(openConversation(instanceKey, user.id));
      setOpen(false);
      void navigate({ to: "/$instance/dm/$conversation", params: { instance: instanceKey, conversation } });
    } catch (err) {
      toast((err as Error).message);
    } finally {
      setOpening(false);
    }
  }

  useEffect(() => {
    if (!open || !user) return;
    setFailed(false);
    setSlow(false);
    const timer = setTimeout(() => setSlow(true), SLOW_MS);
    // Load it fresh each time it opens; what's kept shows meanwhile.
    run(loadProfile(instanceKey, user.id))
      .catch(() => setFailed(true))
      .finally(() => clearTimeout(timer));
    return () => clearTimeout(timer);
  }, [open, instanceKey, user]);

  if (!user) return <>{children}</>;
  return (
    <>
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <AnimatePresence>
        {open && (
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
                  loading={!profile && !failed && slow}
                  className="w-[19rem] max-w-[calc(100vw-1.5rem)]"
                />
                {canMessage && (
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
                      Message
                      <LockKeyholeIcon
                        aria-label="End-to-end encrypted"
                        className="size-3.5 opacity-80 transition-transform duration-300 group-hover:translate-x-0.5"
                      />
                    </motion.button>
                  </motion.div>
                )}
                {canFriend && user && (
                  <motion.div
                    initial={{ opacity: 0, y: -6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.065 }}
                    className="mt-2 rounded-2xl border bg-popover p-1.5 shadow-lg"
                  >
                    <FriendActions instanceKey={instanceKey} user={user} />
                  </motion.div>
                )}
                {canModerate && (
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
                        onClick={() => {
                          setOpen(false);
                          setModerating(action);
                        }}
                        className={
                          "group flex flex-1 items-center justify-center gap-1.5 rounded-xl px-2 py-1.5 text-xs font-bold text-muted-foreground transition hover:bg-destructive/10 hover:text-destructive active:scale-95"
                        }
                      >
                        <Icon className={`size-3.5 transition-transform duration-300 ${hover}`} />
                        {action === "timeout" && timedOutUntil(member, now) ? "Timed out" : label}
                      </button>
                    ))}
                  </motion.div>
                )}
              </motion.div>
            </Popover.Content>
          </Popover.Portal>
        )}
      </AnimatePresence>
    </Popover.Root>
    {member && canModerate && (
      <ModerateDialog instanceKey={instanceKey} serverId={member.serverId} member={moderating ? member : null} action={moderating} onClose={() => setModerating(null)} />
    )}
    </>
  );
}
