import { BanIcon, LockIcon } from "lucide-react";
import { m as motion } from "motion/react";
import type { ReactNode } from "react";
import type { ProfileItem, User } from "@/gen/fuwa/v1/types_pb";
import { UserAvatar } from "@/components/Icons";
import { DecorationImage } from "@/components/ProfileDecoration";
import { useI18n } from "@/i18n/react";
import { SPRING } from "@/lib/motion";
import { cn } from "@/lib/utils";

/**
 * Picks a decoration for your avatar: None (or `noneLabel`, like "Use my
 * profile's"), then each one offered, drawn around your own avatar. Items
 * for sale in the server's shop that you haven't bought (`locks`, with what
 * each costs) show a padlock and open the shop instead.
 */
export function DecorationPicker({
  value,
  onChange,
  user,
  items,
  noneLabel,
  pickedId = "decoration-picked",
  disabled,
  locks,
  onLocked,
}: {
  value: string;
  onChange: (decorationId: string) => void;
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
    <div role="radiogroup" aria-label={t("accountsettings.decorations.label")} className={cn("grid grid-cols-3 gap-2 sm:grid-cols-5", disabled && "pointer-events-none opacity-50")}>
      <Tile label={noneLabel ?? t("accountsettings.effects.none")} active={value === ""} onClick={() => onChange("")} user={user} pickedId={pickedId}>
        <span className="absolute -right-1 -bottom-1 z-[2] grid size-6 place-items-center rounded-full bg-card shadow">
          <BanIcon className="size-3.5 text-muted-foreground transition-transform duration-300 group-hover:rotate-90" />
        </span>
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
          >
            <DecorationImage item={item} />
          </Tile>
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
  children,
}: {
  label: string;
  title?: string;
  active: boolean;
  /** Over the tile: a padlock, for one not bought yet. */
  badge?: ReactNode;
  onClick: () => void;
  user: User;
  pickedId: string;
  children: ReactNode;
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
      whileHover={{ y: -3 }}
      whileTap={{ scale: 0.95 }}
      transition={{ type: "spring", stiffness: 500, damping: 26 }}
      className="group flex flex-col items-center gap-1.5 rounded-2xl p-1 text-xs font-bold outline-none"
    >
      <span className="relative grid aspect-square w-full place-items-center rounded-xl border bg-card shadow-sm transition-shadow group-hover:shadow-md group-focus-visible:ring-2 group-focus-visible:ring-ring">
        <span className={cn("relative size-[55%] transition-transform duration-300 ease-[cubic-bezier(0.3,1.6,0.5,1)] group-hover:scale-105", !!badge && "opacity-60")}>
          <UserAvatar user={user} className="size-full text-lg" />
          {children}
        </span>
        {badge}
        {active && <motion.span layoutId={pickedId} transition={SPRING} className="pointer-events-none absolute inset-0 z-20 rounded-xl ring-2 ring-primary ring-inset" />}
      </span>
      <span className={cn("max-w-full truncate transition-colors", active ? "text-foreground" : "text-muted-foreground group-hover:text-foreground")}>{label}</span>
    </motion.button>
  );
}

/** A padlock over an item you haven't bought, with what it costs. */
export function LockBadge({ children }: { children: ReactNode }) {
  return (
    <span className="pointer-events-none absolute inset-x-1 bottom-1 z-[3] flex items-center justify-center gap-1 rounded-lg bg-background/85 px-1 py-0.5 text-[0.65rem] font-bold shadow-sm backdrop-blur">
      <LockIcon className="size-3 shrink-0 text-muted-foreground" />
      {children}
    </span>
  );
}
