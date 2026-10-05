import { m as motion } from "motion/react";
import type { CSSProperties } from "react";
import { hueOf } from "@/lib/format";
import { cssColor } from "@/lib/permissions";
import { usePrefs } from "@/lib/prefs";
import { cn } from "@/lib/utils";

/**
 * Someone's name, colored by their highest colored role the way the Role
 * colors setting asks: the name itself, a dot beside it, or neither. Without
 * a role color it keeps the person's own tint.
 */
export function RoleName({ id, name, color, className }: { id: string; name: string; color?: number; className?: string }) {
  const mode = usePrefs((p) => p.roleColors);
  const colored = color !== undefined && mode === "names";
  return (
    <span className={cn("inline-flex min-w-0 items-center gap-1.5", className)}>
      <span
        className={cn("truncate font-bold transition-colors duration-300", !colored && "name-tint")}
        style={colored ? { color: cssColor(color) } : ({ "--h": hueOf(id) } as CSSProperties)}
      >
        {name}
      </span>
      {color !== undefined && mode === "beside" && (
        <motion.span
          initial={{ scale: 0 }}
          animate={{ scale: 1 }}
          transition={{ type: "spring", stiffness: 600, damping: 22 }}
          aria-hidden
          className="size-2 shrink-0 rounded-full ring-2 ring-background"
          style={{ background: cssColor(color) }}
        />
      )}
    </span>
  );
}
