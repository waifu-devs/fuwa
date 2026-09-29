import * as Popover from "@radix-ui/react-popover";
import { DoorOpenIcon, GavelIcon, HourglassIcon, type LucideIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { loadProfile, run } from "@/fuwa/actions";
import { useFuwa } from "@/fuwa/store";
import { ModerateDialog, outranks, type ModAction } from "@/components/ModerateDialog";
import { ProfileCard } from "@/components/ProfileCard";
import { timedOutUntil } from "@/lib/format";
import { useNow } from "@/lib/notifications";

const MOD_ACTIONS: { action: ModAction; label: string; icon: LucideIcon; hover: string }[] = [
  { action: "timeout", label: "Time out", icon: HourglassIcon, hover: "group-hover:rotate-180" },
  { action: "kick", label: "Kick", icon: DoorOpenIcon, hover: "group-hover:translate-x-0.5" },
  { action: "ban", label: "Ban", icon: GavelIcon, hover: "group-hover:-rotate-45" },
];

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
  const profile = useFuwa((s) => (user ? s.instances[instanceKey]?.profiles[user.id] : undefined));
  const me = useFuwa((s) => s.instances[instanceKey]?.me?.id === user?.id);
  const [failed, setFailed] = useState(false);
  const mine = useFuwa((s) => {
    const i = s.instances[instanceKey];
    return member ? i?.members[member.serverId]?.find((m) => m.user?.id === i.me?.id) : undefined;
  });
  const canModerate = outranks(mine, member);
  const [moderating, setModerating] = useState<ModAction | null>(null);
  const now = useNow();

  useEffect(() => {
    if (!open || !user) return;
    setFailed(false);
    // Load it fresh each time it opens; what's kept shows meanwhile.
    run(loadProfile(instanceKey, user.id)).catch(() => setFailed(true));
  }, [open, instanceKey, user]);

  if (!user) return <>{children}</>;
  return (
    <>
    <Popover.Root open={open} onOpenChange={setOpen}>
      <Popover.Trigger asChild>{children}</Popover.Trigger>
      <AnimatePresence>
        {open && (
          <Popover.Portal forceMount>
            <Popover.Content forceMount side={side} align="start" sideOffset={10} collisionPadding={12} className="z-50 outline-none">
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
                  me={me}
                  loading={!profile && !failed}
                  className="w-[19rem] max-w-[calc(100vw-1.5rem)]"
                />
                {canModerate && (
                  <motion.div
                    initial={{ opacity: 0, y: -6 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={{ type: "spring", stiffness: 520, damping: 32, delay: 0.08 }}
                    className="mt-2 flex gap-1.5 rounded-2xl border bg-popover p-1.5 shadow-lg"
                  >
                    {MOD_ACTIONS.map(({ action, label, icon: Icon, hover }) => (
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
