import { CheckIcon, PlusIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useState } from "react";
import { Permission, type Member, type Role } from "@/gen/fuwa/v1/types_pb";
import { giveRole, run, takeRole } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAccess, useRoles } from "@/fuwa/hooks";
import { RoleDot } from "@/components/chat/mentions";
import { SPRING } from "@/components/motion";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { memberName } from "@/lib/format";
import { above, has, rolesOf } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/**
 * Someone's roles as chips, highest first. With Manage Roles, the roles
 * ranked below yours get a remove button and a + adds more; chips pop in and
 * out as they change.
 */
export function MemberRoles({ instanceKey, member, compact = false }: { instanceKey: string; member: Member; compact?: boolean }) {
  const roles = useRoles(instanceKey, member.serverId);
  const access = useAccess(instanceKey, member.serverId);
  const held = rolesOf(roles, member);
  const manage = has(access, Permission.MANAGE_ROLES);
  const assignable = roles.filter((r) => r.id !== member.serverId && above(access, r.position));
  const [busy, setBusy] = useState<string | null>(null);
  if (!held.length && !(manage && assignable.length)) return null;

  const toggle = async (role: Role, give: boolean) => {
    setBusy(role.id);
    const userId = member.user?.id ?? "";
    try {
      await run((give ? giveRole : takeRole)(instanceKey, member.serverId, userId, role.id));
    } catch (err) {
      toast((err as FuwaError).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className={cn(!compact && "mt-3")}>
      {!compact && <p className="mb-1.5 text-[0.7rem] font-extrabold tracking-wide text-muted-foreground uppercase">Roles</p>}
      <motion.ul layout className="flex flex-wrap gap-1">
        <AnimatePresence initial={false} mode="popLayout">
          {held.map((role) => {
            const removable = manage && above(access, role.position);
            return (
              <motion.li
                key={role.id}
                layout
                initial={{ opacity: 0, scale: 0.6 }}
                animate={{ opacity: busy === role.id ? 0.5 : 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.6 }}
                transition={SPRING}
                className="group/role flex h-6 max-w-full items-center gap-1.5 rounded-full border bg-background/70 pr-2 pl-1.5 text-xs font-bold"
              >
                {removable ? (
                  <button
                    type="button"
                    aria-label={`Take ${role.name} from ${memberName(member)}`}
                    disabled={!!busy}
                    onClick={() => void toggle(role, false)}
                    className="relative grid size-3 shrink-0 place-items-center"
                  >
                    <RoleDot role={role} className="transition-transform duration-200 group-hover/role:scale-0" />
                    <XIcon className="absolute size-3 scale-0 text-destructive transition-transform duration-200 group-hover/role:scale-100" strokeWidth={3} />
                  </button>
                ) : (
                  <RoleDot role={role} />
                )}
                <span className="truncate">{role.name}</span>
              </motion.li>
            );
          })}
          {manage && assignable.length > 0 && (
            <motion.li key="add" layout transition={SPRING}>
              <DropdownMenu>
                <DropdownMenuTrigger asChild>
                  <button
                    type="button"
                    aria-label={`Change ${memberName(member)}'s roles`}
                    className="grid size-6 place-items-center rounded-full border border-dashed text-muted-foreground transition hover:rotate-90 hover:border-primary/50 hover:text-primary data-[state=open]:rotate-45 data-[state=open]:text-primary"
                  >
                    <PlusIcon className="size-3.5" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="max-h-72 w-56 overflow-y-auto">
                  <DropdownMenuLabel className="text-xs text-muted-foreground">Roles you can hand out</DropdownMenuLabel>
                  {assignable.map((role) => {
                    const on = member.roleIds.includes(role.id);
                    return (
                      <DropdownMenuItem
                        key={role.id}
                        disabled={!!busy}
                        onSelect={(e) => {
                          e.preventDefault();
                          void toggle(role, !on);
                        }}
                      >
                        <RoleDot role={role} />
                        <span className="flex-1 truncate">{role.name}</span>
                        <AnimatePresence initial={false}>
                          {on && (
                            <motion.span initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={SPRING}>
                              <CheckIcon className="size-4 text-primary" strokeWidth={3} />
                            </motion.span>
                          )}
                        </AnimatePresence>
                      </DropdownMenuItem>
                    );
                  })}
                </DropdownMenuContent>
              </DropdownMenu>
            </motion.li>
          )}
        </AnimatePresence>
      </motion.ul>
    </div>
  );
}
