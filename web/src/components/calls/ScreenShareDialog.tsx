import { MonitorUpIcon, Volume2Icon } from "lucide-react";
import { m as motion } from "motion/react";
import { useState } from "react";
import { shareScreen } from "@/calls/engine";
import { useCalls } from "@/calls/state";
import { canPickSurface, canShareSound } from "@/calls/video";
import { Count } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogHeader } from "@/components/ui/dialog";
import { Switch } from "@/components/ui/switch";
import { useI18n } from "@/i18n/react";
import { SPRING } from "@/lib/motion";
import { getPrefs } from "@/lib/prefs";
import { SHARE_FPS, SHARE_HEIGHTS, SHARE_SURFACES, shareMbps, shareQuality, type ShareFps, type ShareHeight, type ShareSurface } from "@/lib/screen-share";
import { cn } from "@/lib/utils";

/**
 * Starting a screen share: what the browser's picker opens on (a screen, a
 * window or a tab, drawn as small pictures of each), how sharp and how
 * smooth it goes out, and its sound. The browser itself then asks which
 * one; a browser whose picker can't be steered skips the first part.
 * Starts from the last share's choices, and keeps these for next time.
 */
export function ScreenShareDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useI18n();
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader title={t("dms-calls.calls.share.title")} description={t("dms-calls.calls.share.description")} />
        {/* Mounted only while open, so each time starts from the saved choices. */}
        <ShareForm onDone={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}

function ShareForm({ onDone }: { onDone: () => void }) {
  const { t } = useI18n();
  const saved = getPrefs();
  const [surface, setSurface] = useState<ShareSurface>(saved.shareSurface);
  const [quality, setQuality] = useState(() => shareQuality(saved.shareHeight, saved.shareFps));
  const [sound, setSound] = useState(saved.shareSound);
  const offered = useCalls((s) => s.screenSoundOffered);
  const browserSound = canShareSound();
  const picks = canPickSurface();

  function start() {
    onDone();
    void shareScreen({ sound: sound && offered && browserSound, surface, quality });
  }

  return (
    <div className="flex flex-col gap-5">
      <section>
        <Heading>{t("dms-calls.calls.share.what")}</Heading>
        {picks ? (
          <>
            <div role="radiogroup" aria-label={t("dms-calls.calls.share.what")} className="grid grid-cols-3 gap-2">
              {SHARE_SURFACES.map((s) => (
                <SurfaceChoice key={s} surface={s} active={surface === s} onPick={() => setSurface(s)} />
              ))}
            </div>
            <p className="mt-2 text-xs text-muted-foreground">{t("dms-calls.calls.share.surfaceHint")}</p>
          </>
        ) : (
          <p className="text-sm text-muted-foreground">{t("dms-calls.calls.share.surfaceNext")}</p>
        )}
      </section>

      <section>
        <Heading>{t("dms-calls.calls.share.resolution")}</Heading>
        <Segmented
          name="share-height"
          label={t("dms-calls.calls.share.resolution")}
          options={SHARE_HEIGHTS}
          value={quality.height}
          onPick={(height: ShareHeight) => setQuality((q) => ({ ...q, height }))}
          title={(h) => `${h}p`}
          hint={(h) => t(`dms-calls.calls.share.height.${h}`)}
        />
      </section>

      <section>
        <Heading>{t("dms-calls.calls.share.frameRate")}</Heading>
        <Segmented
          name="share-fps"
          label={t("dms-calls.calls.share.frameRate")}
          options={SHARE_FPS}
          value={quality.fps}
          onPick={(fps: ShareFps) => setQuality((q) => ({ ...q, fps }))}
          title={(fps) => t("dms-calls.calls.share.fps", { fps })}
          hint={(fps) => t(`dms-calls.calls.share.fps.${fps}`)}
        />
        <p className="mt-2 text-xs text-muted-foreground tabular-nums">
          <UploadLine mbps={shareMbps(quality)} />
        </p>
      </section>

      <label className={cn("flex items-start gap-3 rounded-2xl border p-3 transition-colors", offered && browserSound ? "cursor-pointer hover:border-primary/40" : "opacity-60")}>
        <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-muted text-muted-foreground">
          <Volume2Icon className="size-[18px]" />
        </span>
        <span className="min-w-0 flex-1">
          <span className="block text-sm font-bold">{t("dms-calls.calls.share.sound")}</span>
          <span className="block text-xs text-muted-foreground">
            {!offered ? t("dms-calls.calls.share.noServerSound") : browserSound ? t("dms-calls.calls.video.withSoundText") : t("dms-calls.calls.video.noSoundHere")}
          </span>
        </span>
        <Switch checked={sound && offered && browserSound} disabled={!offered || !browserSound} onCheckedChange={setSound} className="mt-1" />
      </label>

      <Button size="lg" onClick={start} className="btn h-11 rounded-xl font-bold">
        <MonitorUpIcon />
        <span>{t("dms-calls.calls.share.start")}</span>
      </Button>
    </div>
  );
}

function Heading({ children }: { children: string }) {
  return <h3 className="mb-2 text-xs font-bold tracking-wide text-muted-foreground uppercase">{children}</h3>;
}

/** "Up to 3.4 Mbit/s of upload", the number rolling as the choice changes. */
function UploadLine({ mbps }: { mbps: number }) {
  const { t } = useI18n();
  const [before, after] = t("dms-calls.calls.share.upload", { mbps: "\u0000" }).split("\u0000");
  return (
    <>
      {before}
      <Count value={mbps} />
      {after}
    </>
  );
}

/** One kind of thing to share, as a card with a small picture of it. */
function SurfaceChoice({ surface, active, onPick }: { surface: ShareSurface; active: boolean; onPick: () => void }) {
  const { t } = useI18n();
  return (
    <motion.button
      type="button"
      role="radio"
      aria-checked={active}
      onClick={onPick}
      whileHover={{ y: -2 }}
      whileTap={{ scale: 0.97 }}
      transition={SPRING}
      className={cn("relative flex flex-col items-stretch gap-2 rounded-2xl border p-2 text-left transition-colors", active ? "border-primary/60" : "hover:border-primary/40")}
    >
      {active && <motion.span layoutId="share-surface" transition={SPRING} className="absolute inset-0 rounded-2xl bg-primary/10 ring-2 ring-primary/40" />}
      <SurfacePicture surface={surface} active={active} />
      <span className="relative px-1 pb-0.5">
        <span className="block text-sm font-bold">{t(`dms-calls.calls.share.surface.${surface}`)}</span>
        <span className="block text-[11px] leading-tight text-muted-foreground">{t(`dms-calls.calls.share.surface.${surface}Hint`)}</span>
      </span>
    </motion.button>
  );
}

/** A screen with its windows, one window, or a browser with its tabs, drawn small; the shared part lit. */
function SurfacePicture({ surface, active }: { surface: ShareSurface; active: boolean }) {
  const lit = active ? "bg-primary/70" : "bg-foreground/25";
  const dim = "bg-foreground/10";
  return (
    <span className={cn("relative grid aspect-video place-items-center overflow-hidden rounded-xl border transition-colors", active ? "border-primary/40 bg-primary/5" : "bg-muted/60")}>
      <motion.span key={String(active)} initial={active ? { scale: 0.9 } : false} animate={{ scale: 1 }} transition={SPRING} className="absolute inset-[12%] block">
        {surface === "monitor" && (
          <span className={cn("absolute inset-0 rounded-md p-[6%] transition-colors", active ? "bg-primary/25 ring-2 ring-primary/70" : "bg-foreground/10")}>
            <span className={cn("absolute top-[12%] left-[8%] h-[45%] w-[48%] rounded-sm", lit)} />
            <span className={cn("absolute top-[30%] right-[8%] h-[45%] w-[38%] rounded-sm", lit, "opacity-70")} />
            <span className={cn("absolute inset-x-0 bottom-0 h-[14%] rounded-b-md", lit, "opacity-60")} />
          </span>
        )}
        {surface === "window" && (
          <>
            <span className={cn("absolute top-[8%] left-[2%] h-[60%] w-[50%] rounded-sm", dim)} />
            <span className={cn("absolute top-[20%] left-[26%] h-[74%] w-[70%] overflow-hidden rounded-md transition-colors", active ? "bg-primary/25 ring-2 ring-primary/70" : "bg-foreground/15")}>
              <span className={cn("absolute inset-x-0 top-0 h-[18%]", lit)} />
              <span className="absolute top-[5%] left-[6%] flex gap-[3px]">
                <span className="size-1 rounded-full bg-background/80" />
                <span className="size-1 rounded-full bg-background/80" />
                <span className="size-1 rounded-full bg-background/80" />
              </span>
            </span>
          </>
        )}
        {surface === "browser" && (
          <span className="absolute inset-0 overflow-hidden rounded-md bg-foreground/10">
            <span className="absolute top-[4%] left-[6%] flex h-[16%] w-[70%] gap-[6%]">
              <span className={cn("h-full flex-1 rounded-t-sm transition-colors", active ? "bg-primary/70" : "bg-foreground/30")} />
              <span className={cn("h-full flex-1 rounded-t-sm", dim)} />
              <span className={cn("h-full flex-1 rounded-t-sm", dim)} />
            </span>
            <span className={cn("absolute inset-x-0 top-[20%] bottom-0 transition-colors", active ? "bg-primary/25 ring-2 ring-primary/70" : "bg-foreground/15")}>
              <span className={cn("absolute top-[14%] left-[8%] h-[10%] w-[60%] rounded-full", lit)} />
              <span className={cn("absolute top-[34%] left-[8%] h-[40%] w-[84%] rounded-sm", lit, "opacity-50")} />
            </span>
          </span>
        )}
      </motion.span>
    </span>
  );
}

/** A row of choices with a pill that glides to the picked one. */
function Segmented<T extends number>({
  name,
  label,
  options,
  value,
  onPick,
  title,
  hint,
}: {
  name: string;
  label: string;
  options: readonly T[];
  value: T;
  onPick: (value: T) => void;
  title: (value: T) => string;
  hint: (value: T) => string;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="grid grid-flow-col auto-cols-fr gap-1 rounded-2xl bg-muted p-1">
      {options.map((option) => {
        const active = option === value;
        return (
          <motion.button
            key={option}
            type="button"
            role="radio"
            aria-checked={active}
            onClick={() => onPick(option)}
            whileTap={{ scale: 0.96 }}
            transition={SPRING}
            className={cn("relative rounded-xl px-2 py-2 text-center transition-colors", active ? "text-foreground" : "text-muted-foreground hover:text-foreground")}
          >
            {active && <motion.span layoutId={`${name}-pill`} transition={SPRING} className="absolute inset-0 rounded-xl bg-card shadow-sm ring-1 ring-primary/30" />}
            <span className="relative block text-sm font-extrabold tabular-nums">{title(option)}</span>
            <span className="relative block text-[11px] leading-tight">{hint(option)}</span>
          </motion.button>
        );
      })}
    </div>
  );
}
