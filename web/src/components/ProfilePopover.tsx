import * as Popover from "@radix-ui/react-popover";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import type { Member, User } from "@/gen/fuwa/v1/types_pb";
import { loadProfile, run } from "@/fuwa/actions";
import { useFuwa } from "@/fuwa/store";
import { ProfileCard } from "@/components/ProfileCard";

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

  useEffect(() => {
    if (!open || !user) return;
    setFailed(false);
    // Load it fresh each time it opens; what's kept shows meanwhile.
    run(loadProfile(instanceKey, user.id)).catch(() => setFailed(true));
  }, [open, instanceKey, user]);

  if (!user) return <>{children}</>;
  return (
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
              </motion.div>
            </Popover.Content>
          </Popover.Portal>
        )}
      </AnimatePresence>
    </Popover.Root>
  );
}
