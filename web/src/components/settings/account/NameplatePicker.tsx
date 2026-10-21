import { BanIcon } from "lucide-react";
import { m as motion } from "motion/react";
import type { ReactNode } from "react";
import type { ProfileItem, User } from "@/gen/fuwa/v1/types_pb";
import { UserAvatar } from "@/components/Icons";
import { NameplateImage } from "@/components/ProfileDecoration";
import { LockBadge } from "@/components/settings/account/DecorationPicker";
import { useI18n } from "@/i18n/react";
import { displayName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

/**
 * Picks a nameplate: None (or `noneLabel`), then each one offered, drawn as
 * your own row in the member list. Like the decoration picker, items for
 * sale you haven't bought (`locks`) show a padlock and open the shop.
 */
export function NameplatePicker({
  value,
  onChange,
  user,
  items,
  noneLabel,
  pickedId = "nameplate-picked",
  disabled,
  locks,
  onLocked,
}: {
  value: string;
  onChange: (nameplateId: string) => void;
  user: User;
  items: ProfileItem[];
  noneLabel?: string;
  /** The ring's layout id, apart for each picker on screen. */
  pickedId?: string;
  disabled?: boolean;
  locks?: ReadonlyMap<string, ReactNode>;
  onLocked?: (itemId: string) => void;
}) {
  const { t } = useI18n();
  return (
    <div role="radiogroup" aria-label={t("accountsettings.nameplates.label")} className={cn("grid gap-2 sm:grid-cols-2", disabled && "pointer-events-none opacity-50")}>
      <Tile label={noneLabel ?? t("accountsettings.effects.none")} active={value === ""} onClick={() => onChange("")} user={user} pickedId={pickedId}>
        <BanIcon className="ml-auto size-3.5 shrink-0 text-muted-foreground transition-transform duration-300 group-hover:rotate-90" />
      </Tile>
      {items.map((item) => {
        const lock = value !== item.id ? locks?.get(item.id) : undefined;
        return (
          <Tile
            key={item.id}
            label={item.name}
            title={lock ? t("accountsettings.serverProfiles.forSale", { name: item.name }) : item.description || undefined}
            active={value === item.id}
            badge={lock && <LockBadge>{lock}</LockBadge>}
            onClick={() => (lock ? onLocked?.(item.id) : onChange(item.id))}
            user={user}
            pickedId={pickedId}
            plate={<NameplateImage item={item} />}
          />
        );
      })}
    </div>
  );
}

function Tile({
  label,
  title,
  active,
  badge,
  onClick,
  user,
  pickedId,
  plate,
  children,
}: {
  label: string;
  title?: string;
  active: boolean;
  badge?: ReactNode;
  onClick: () => void;
  user: User;
  pickedId: string;
  /** The nameplate behind the row. */
  plate?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <motion.button
      type="button"
      role="radio"
      aria-checked={active}
      aria-disabled={badge ? true : undefined}
      aria-label={label}
      title={title ?? label}
      onClick={onClick}
      whileHover={{ y: -2 }}
      whileTap={{ scale: 0.97 }}
      transition={{ type: "spring", stiffness: 500, damping: 26 }}
      className="group flex flex-col gap-1 rounded-2xl p-1 text-left text-xs font-bold outline-none"
    >
      <span className="relative isolate flex h-12 w-full items-center gap-2.5 overflow-hidden rounded-xl border bg-card px-2.5 shadow-sm transition-shadow group-hover:shadow-md group-focus-visible:ring-2 group-focus-visible:ring-ring">
        {plate}
        <UserAvatar user={user} className={cn("size-8 shrink-0 text-xs", !!badge && "opacity-60")} />
        <span className={cn("min-w-0 truncate text-sm", !!badge && "opacity-60")}>{displayName(user)}</span>
        {children}
        {badge}
        {active && <motion.span layoutId={pickedId} transition={SPRING} className="pointer-events-none absolute inset-0 z-20 rounded-xl ring-2 ring-primary ring-inset" />}
      </span>
      <span className={cn("max-w-full truncate px-1 transition-colors", active ? "text-foreground" : "text-muted-foreground group-hover:text-foreground")}>{label}</span>
    </motion.button>
  );
}
