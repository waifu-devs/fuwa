import { BanIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { useState, type CSSProperties, type ReactNode } from "react";
import { hue } from "@/components/icons-utils";
import { ProfileEffect } from "@/components/ProfileEffect";
import { type I18n, useI18n } from "@/i18n/react";
import { BUILTIN_EFFECTS, type ProfileEffectSpec } from "@/lib/effects/profile";
import { effectKeys } from "@/lib/effects/text";
import { colorCss, hueOf } from "@/lib/format";
import { cn } from "@/lib/utils";

/** A built-in effect's name and line in the app's language; any other effect keeps its own. */
function effectText(t: I18n["t"], effect: ProfileEffectSpec): { name: string; description: string } {
  const keys = effectKeys(effect.id);
  return keys ? { name: t(keys.name), description: t(keys.about) } : effect;
}

/** An effect's one line, as the picker's tiles say it. */
export function EffectAbout({ effect }: { effect: ProfileEffectSpec }) {
  const { t } = useI18n();
  return <>{effectText(t, effect).description}</>;
}

/** An effect someone offers (the instance or a server), by its item's id, ready to play. */
export type OfferedEffect = { id: string; spec: ProfileEffectSpec };

/**
 * Picks a profile effect: None (or `noneLabel`, like "Use my profile's"),
 * then a tile for each effect fuwa ships with and each one `offered` by the
 * instance or the server. Tiles sit still until you point at one, focus it
 * or pick it, so a grid of them never plays nine effects at once.
 */
export function EffectPicker({
  value,
  onChange,
  userId,
  accent,
  offered = [],
  noneLabel,
  pickedId = "effect-picked",
  disabled,
}: {
  value: string;
  onChange: (effect: string) => void;
  userId: string;
  /** The profile color, 0xRRGGBB, or -1 for fuwa's pick. */
  accent: number;
  offered?: OfferedEffect[];
  /** What the empty choice is called. */
  noneLabel?: string;
  /** The ring's layout id, apart for each picker on screen. */
  pickedId?: string;
  disabled?: boolean;
}) {
  const { t } = useI18n();
  const color = accent < 0 ? `hsl(${hueOf(userId)} 85% 72%)` : colorCss(accent);
  return (
    <div role="radiogroup" aria-label={t("settings.nav.profileEffect")} className={cn("grid grid-cols-3 gap-2 sm:grid-cols-5", disabled && "pointer-events-none opacity-50")}>
      <Tile label={noneLabel ?? t("accountsettings.effects.none")} active={value === ""} onClick={() => onChange("")} userId={userId} accent={accent} pickedId={pickedId}>
        <span className="absolute inset-x-0 top-[28%] bottom-0 grid place-items-center bg-card/70">
          <BanIcon className="size-6 text-muted-foreground transition-transform duration-300 group-hover:rotate-90" />
        </span>
      </Tile>
      {BUILTIN_EFFECTS.map((effect) => (
        <Tile
          key={effect.id}
          label={effectText(t, effect).name}
          title={effectText(t, effect).description}
          active={value === effect.id}
          onClick={() => onChange(effect.id)}
          userId={userId}
          accent={accent}
          pickedId={pickedId}
        >
          {(lively) => <ProfileEffect effect={effect.id} seed={userId} color={color} play={lively} measure={false} replayOnHover={false} />}
        </Tile>
      ))}
      {offered.map(({ id, spec }) => (
        <Tile key={id} label={spec.name} title={spec.description || undefined} active={value === id} onClick={() => onChange(id)} userId={userId} accent={accent} pickedId={pickedId}>
          {(lively) => <ProfileEffect effect={spec} seed={userId} color={color} play={lively} measure={false} replayOnHover={false} />}
        </Tile>
      ))}
    </div>
  );
}

/**
 * A tiny profile card: banner, avatar and two lines, so an effect reads as
 * it will on yours. Its children (an effect) go over it.
 */
export function MiniCard({ userId, accent, className, children }: { userId: string; accent: number; className?: string; children?: ReactNode }) {
  const banner: CSSProperties = accent < 0 ? hue(userId) : { backgroundColor: colorCss(accent) };
  const plain = accent < 0;
  return (
    <span className={cn("relative block aspect-[4/5] w-full overflow-hidden rounded-xl border bg-card shadow-sm", className)}>
      <span style={banner} className={cn("absolute inset-x-0 top-0 h-[28%]", plain && "server-gradient")} />
      <span style={banner} className={cn("absolute top-[17%] left-[11%] aspect-square w-[30%] rounded-full ring-2 ring-card", plain && "server-gradient")} />
      <span className="absolute top-[56%] left-[11%] h-[6%] w-[55%] rounded-full bg-foreground/15" />
      <span className="absolute top-[67%] left-[11%] h-[5%] w-[38%] rounded-full bg-foreground/10" />
      {children}
    </span>
  );
}

/** The color an effect paints with as "profile", for someone's card. */
export const cardColor = (userId: string, accent: number) => (accent < 0 ? `hsl(${hueOf(userId)} 85% 72%)` : colorCss(accent));

function Tile({
  label,
  title,
  active,
  onClick,
  userId,
  accent,
  pickedId,
  children,
}: {
  label: string;
  title?: string;
  active: boolean;
  onClick: () => void;
  userId: string;
  accent: number;
  pickedId: string;
  children: ReactNode | ((lively: boolean) => ReactNode);
}) {
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const lively = hovered || focused || active;
  return (
    <motion.button
      type="button"
      role="radio"
      aria-checked={active}
      aria-label={label}
      title={title}
      onClick={onClick}
      onPointerEnter={() => setHovered(true)}
      onPointerLeave={() => setHovered(false)}
      onFocus={() => setFocused(true)}
      onBlur={() => setFocused(false)}
      whileHover={{ y: -3 }}
      whileTap={{ scale: 0.95 }}
      transition={{ type: "spring", stiffness: 500, damping: 26 }}
      className="group flex flex-col items-center gap-1.5 rounded-2xl p-1 text-xs font-bold outline-none"
    >
      <MiniCard userId={userId} accent={accent} className="transition-shadow group-hover:shadow-md group-focus-visible:ring-2 group-focus-visible:ring-ring">
        {typeof children === "function" ? children(lively) : children}
        {active && (
          <motion.span
            layoutId={pickedId}
            transition={{ type: "spring", stiffness: 520, damping: 34 }}
            className="pointer-events-none absolute inset-0 z-20 rounded-xl ring-2 ring-primary ring-inset"
          />
        )}
      </MiniCard>
      <span className={cn("truncate transition-colors", active ? "text-foreground" : "text-muted-foreground group-hover:text-foreground")}>{label}</span>
    </motion.button>
  );
}
