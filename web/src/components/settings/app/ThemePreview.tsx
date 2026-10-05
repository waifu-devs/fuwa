import { m as motion } from "motion/react";
import { BackdropLayers } from "@/components/Backdrop";
import { usePageVisible } from "@/hooks/use-page-visible";
import { SPRING } from "@/lib/motion";
import { type Key, useI18n } from "@/i18n/react";
import { hasBackdrop, type Backdrop } from "@/lib/backdrop";
import { reduceMotion, usePrefs } from "@/lib/prefs";
import { isDark, themeStyle, type Theme } from "@/lib/themes";
import { cn } from "@/lib/utils";

const LINES = [
  { id: "hana", name: "Hana", width: "78%", second: "52%" },
  { id: "ren", name: "Ren", width: "64%", second: null },
  { id: "you", name: null, width: "70%", second: "38%" },
];

const CHANNELS: Key[] = ["appsettings.preview.general", "appsettings.preview.art", "appsettings.preview.music", "appsettings.preview.games"];

/**
 * fuwa in miniature, in a theme and over a backdrop: the rail, the sidebar
 * and a few messages. Every color is the theme's own token, so it shows
 * exactly what the app will look like. The backdrop runs live here too.
 */
export function ThemePreview({ theme, backdrop, className }: { theme: Theme; backdrop: Backdrop; className?: string }) {
  const { t: text } = useI18n();
  const visible = usePageVisible();
  const still = usePrefs(reduceMotion);
  const t = theme.variant.tokens;
  const on = hasBackdrop(backdrop);
  const panel = (color: string, more = 0) => (on ? `color-mix(in srgb, ${color} ${Math.min(100, backdrop.panels + more)}%, transparent)` : color);
  return (
    <motion.div
      layout
      transition={SPRING}
      style={{ ...themeStyle(theme.variant), background: t.background, color: t.foreground, borderColor: t.border }}
      className={cn("theme-preview relative isolate flex h-64 overflow-hidden rounded-3xl border shadow-lg", isDark(theme) && "dark", className)}
    >
      {on && <BackdropLayers backdrop={backdrop} tokens={t} running={visible} still={still} className="-z-10" />}
      <div className="flex w-12 shrink-0 flex-col items-center gap-2 py-3" style={{ background: panel(`color-mix(in srgb, ${t.primary} 9%, ${t.muted})`, 20) }}>
        {[t.primary, t["muted-foreground"], t.border].map((c, n) => (
          <motion.span
            key={n}
            className="size-7"
            style={{ background: c, borderRadius: n === 0 ? "32%" : "50%" }}
            whileHover={{ borderRadius: "32%" }}
          />
        ))}
      </div>
      <div className="flex w-28 shrink-0 flex-col gap-1.5 border-r p-3" style={{ background: panel(`color-mix(in srgb, ${t.card} 55%, ${t.background})`, 20), borderColor: t.border }}>
        <span className="mb-1 h-2.5 w-16 rounded-full" style={{ background: t.foreground, opacity: 0.8 }} />
        {CHANNELS.map((name, n) => (
          <span
            key={name}
            className="flex items-center gap-1 rounded-md px-1.5 py-1 text-[0.6rem] font-bold"
            style={n === 0 ? { background: t.accent, color: t["accent-foreground"] } : { color: t["muted-foreground"] }}
          >
            # {text(name)}
          </span>
        ))}
      </div>
      <div className="flex min-w-0 flex-1 flex-col gap-3 p-3" style={{ background: panel(t.background) }}>
        {LINES.map((line, n) => (
          <div key={line.id} className="flex gap-2">
            <span className="size-6 shrink-0 rounded-full" style={{ background: n === 2 ? t.primary : t.secondary, border: `1px solid ${t.border}` }} />
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-[0.65rem] font-extrabold" style={{ color: n === 2 ? t.primary : t.foreground }}>
                {line.name ?? text("appsettings.preview.you")}
              </span>
              <span className="h-2 rounded-full" style={{ width: line.width, background: t.foreground, opacity: 0.55 }} />
              {line.second && <span className="h-2 rounded-full" style={{ width: line.second, background: t.foreground, opacity: 0.35 }} />}
            </div>
          </div>
        ))}
        <div className="mt-auto flex items-center gap-2 rounded-[var(--radius)] border px-2 py-1.5" style={{ background: panel(t.card, 40), borderColor: t.border }}>
          <span className="h-2 flex-1 rounded-full" style={{ background: t["muted-foreground"], opacity: 0.35 }} />
          <span className="rounded-[calc(var(--radius)-4px)] px-2 py-0.5 text-[0.6rem] font-bold" style={{ background: t.primary, color: t["primary-foreground"] }}>
            {text("appsettings.preview.send")}
          </span>
        </div>
      </div>
    </motion.div>
  );
}
