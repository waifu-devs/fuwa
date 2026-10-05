import { CheckIcon, ClipboardPenIcon, MonitorIcon, PaletteIcon, PartyPopperIcon, SendIcon, SmartphoneIcon, SparklesIcon, WandSparklesIcon } from "lucide-react";
import { AnimatePresence, LayoutGroup, m as motion } from "motion/react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent, type ReactNode } from "react";
import { type JoinForm, type Onboarding, type Server, type WelcomeScreen } from "@/gen/fuwa/v1/types_pb";
import { getJoinForm, getOnboarding, getWelcomeScreen, run, setOnboarding, setWelcomeScreen, updateServer } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance, useRoles } from "@/fuwa/hooks";
import { BannerHero, ServerBanner } from "@/components/join/Banner";
import { OnboardingFlow, stepsFor } from "@/components/join/Onboarding";
import { RulesList } from "@/components/join/Rules";
import { WelcomeCard } from "@/components/join/Welcome";
import { SPRING } from "@/lib/motion";
import { PictureField } from "@/components/PictureField";
import { SaveBar } from "@/components/settings/controls";
import { OnboardingFields } from "@/components/settings/server/OnboardingEditor";
import { onboardingChanges, onboardingDraft, onboardingOf, type OnboardingDraft } from "@/components/settings/server/onboarding-draft";
import { WelcomeFields, welcomeChanges, welcomeDraft, welcomeScreen, type WelcomeDraft } from "@/components/settings/server/WelcomeScreenEditor";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useI18n } from "@/i18n/react";
import { accentVars, bannerColors, bannerPosition, hex, parseHex, type BannerServer } from "@/lib/banner";
import { shownPicture } from "@/lib/shown";
import { cn } from "@/lib/utils";

type Look = { bannerUrl: string; bannerFocusX: number; bannerFocusY: number; accentColor: number | undefined };
const lookOf = (s: Server): Look => ({ bannerUrl: s.bannerUrl, bannerFocusX: s.bannerFocusX, bannerFocusY: s.bannerFocusY, accentColor: s.accentColor });

type View = "welcome" | "apply" | "onboarding";
type Device = "desktop" | "phone";
const DEVICES: Record<Device, { width: number; height: number; label: string; icon: typeof MonitorIcon }> = {
  desktop: { width: 1280, height: 800, label: "1280", icon: MonitorIcon },
  phone: { width: 390, height: 844, label: "390", icon: SmartphoneIcon },
};

/**
 * Welcome and onboarding: the server's banner and color, its welcome
 * screen, and the steps new members go through, with the real screens
 * previewed live at desktop and phone width as you change them. One bar
 * saves all three.
 */
export function WelcomeAndOnboarding({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const { t } = useI18n();
  const [saved, setSaved] = useState<{ welcome: WelcomeScreen; onboarding: Onboarding; form: JoinForm } | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [look, setLook] = useState<Look>(() => lookOf(server));
  const [welcome, setWelcome] = useState<WelcomeDraft | null>(null);
  const [onboarding, setOnboardingDraft] = useState<OnboardingDraft | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [view, setView] = useState<View>("welcome");
  const [device, setDevice] = useState<Device>("desktop");
  const [focusStep, setFocusStep] = useState(0);

  useEffect(() => {
    let cancelled = false;
    Promise.all([run(getWelcomeScreen(instanceKey, server.id)), run(getOnboarding(instanceKey, server.id)), run(getJoinForm(instanceKey, server.id))]).then(
      ([w, o, form]) => {
        if (cancelled) return;
        setSaved({ welcome: w, onboarding: o, form });
        setWelcome(welcomeDraft(w));
        setOnboardingDraft(onboardingDraft(o));
      },
      (e: FuwaError) => !cancelled && setProblem(e.message),
    );
    return () => {
      cancelled = true;
    };
  }, [instanceKey, server.id]);

  if (problem) return <p className="text-sm text-muted-foreground first-letter:uppercase">{problem}</p>;
  if (!saved || !welcome || !onboarding) return <div className="shimmer h-96 rounded-3xl" />;

  const was = lookOf(server);
  const lookChanged = [look.bannerUrl !== was.bannerUrl, look.bannerFocusX !== was.bannerFocusX || look.bannerFocusY !== was.bannerFocusY, look.accentColor !== was.accentColor].filter(Boolean).length;
  const changes = lookChanged + welcomeChanges(welcome, saved.welcome) + onboardingChanges(onboarding, saved.onboarding);
  const previewServer = { ...server, ...look };

  async function save() {
    if (!saved || !welcome || !onboarding) return;
    setSaving(true);
    setError(null);
    try {
      if (lookChanged) {
        await run(
          updateServer(instanceKey, server.id, {
            ...(look.bannerUrl !== was.bannerUrl ? { bannerUrl: look.bannerUrl.trim() } : {}),
            bannerFocusX: look.bannerFocusX,
            bannerFocusY: look.bannerFocusY,
            accentColor: look.accentColor ?? -1,
          }),
        );
      }
      let w = saved.welcome;
      if (welcomeChanges(welcome, saved.welcome)) {
        const draft = welcomeScreen(welcome);
        if (draft.enabled && !draft.description && draft.channels.length === 0) throw new Error(t("serversettings.welcome.emptyScreen"));
        w = await run(setWelcomeScreen(instanceKey, server.id, draft));
        setWelcome(welcomeDraft(w));
      }
      let o = saved.onboarding;
      if (onboardingChanges(onboarding, saved.onboarding)) {
        o = await run(setOnboarding(instanceKey, server.id, onboardingOf(onboarding)));
        setOnboardingDraft(onboardingDraft(o));
      }
      setSaved({ ...saved, welcome: w, onboarding: o });
    } catch (err) {
      setError((err as FuwaError).message ?? t("serversettings.welcome.saveFailed"));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="flex flex-col gap-8">
      <Preview
        instanceKey={instanceKey}
        server={previewServer}
        view={view}
        onView={setView}
        device={device}
        onDevice={setDevice}
        welcome={welcomeScreen(welcome)}
        onboarding={onboardingOf(onboarding)}
        form={saved.form}
        focusStep={focusStep}
      />
      <Section id="banner" title={t("serversettings.welcome.banner")} hint={t("serversettings.welcome.bannerHint")}>
        <BannerFields instanceKey={instanceKey} server={server} look={look} onChange={setLook} />
      </Section>
      <Section id="welcome" title={t("serversettings.welcome.screen")} hint={t("serversettings.welcome.screenHint")}>
        <div onFocusCapture={() => setView("welcome")}>
          <WelcomeFields instanceKey={instanceKey} server={server} draft={welcome} onChange={setWelcome} />
        </div>
      </Section>
      <Section id="onboarding" title={t("serversettings.nav.onboarding")} hint={t("serversettings.welcome.onboardingHint")}>
        <div onFocusCapture={() => setView("onboarding")}>
          <OnboardingFields
            instanceKey={instanceKey}
            server={server}
            draft={onboarding}
            onChange={setOnboardingDraft}
            onFocusStep={(n) => {
              setView("onboarding");
              setFocusStep(n);
            }}
          />
        </div>
      </Section>
      <SaveBar
        count={changes}
        saving={saving}
        error={error}
        onSave={() => void save()}
        onDiscard={() => {
          setLook(lookOf(server));
          setWelcome(welcomeDraft(saved.welcome));
          setOnboardingDraft(onboardingDraft(saved.onboarding));
          setError(null);
        }}
      />
    </div>
  );
}

function Section({ id, title, hint, children }: { id: string; title: string; hint: string; children: ReactNode }) {
  return (
    <motion.section data-setting={id} initial={{ opacity: 0, y: 12 }} animate={{ opacity: 1, y: 0 }} transition={SPRING} className="flex flex-col gap-4">
      <div>
        <h3 className="text-lg font-extrabold tracking-tight">{title}</h3>
        <p className="text-sm text-muted-foreground">{hint}</p>
      </div>
      {children}
    </motion.section>
  );
}

// ───────────────────────── Banner ─────────────────────────

function BannerFields({ instanceKey, server, look, onChange }: { instanceKey: string; server: Server; look: Look; onChange: (look: Look) => void }) {
  const { t } = useI18n();
  const [colors, setColors] = useState<number[]>([]);
  // Only pictures on a trusted instance are ever loaded here: a pasted link
  // to another site shows nothing until it's saved (and refused, see check_picture).
  const src = shownPicture(look.bannerUrl);
  const shown = { ...server, ...look, bannerUrl: src };
  const custom = look.accentColor !== undefined && !colors.includes(look.accentColor);
  useEffect(() => {
    let cancelled = false;
    if (!src) {
      setColors([]);
      return;
    }
    void bannerColors(src).then((c) => !cancelled && setColors(c));
    return () => {
      cancelled = true;
    };
  }, [src]);

  return (
    <div className="flex flex-col gap-5">
      <div data-setting="banner-picture">
        <PictureField
          instanceKey={instanceKey}
          kind="banner"
          serverId={server.id}
          value={look.bannerUrl}
          onChange={(bannerUrl) => onChange({ ...look, bannerUrl, bannerFocusX: 50, bannerFocusY: 50 })}
          fallback={<ServerBanner server={shown} fade={false} pan={false} className="size-full" />}
        />
      </div>
      <AnimatePresence initial={false}>
        {src && (
          <motion.div data-setting="banner-focus" initial={{ opacity: 0, y: -8 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -8 }} transition={SPRING}>
            <FocusPicker server={shown} onChange={(x, y) => onChange({ ...look, bannerFocusX: x, bannerFocusY: y })} />
          </motion.div>
        )}
      </AnimatePresence>
      <div data-setting="accent-color" className="flex flex-col gap-2">
        <span>
          <span className="flex items-center gap-1.5 text-sm font-bold">
            <PaletteIcon className="size-4 text-muted-foreground" /> {t("serversettings.nav.accentColor")}
          </span>
          <span className="block text-xs text-muted-foreground">{t("serversettings.welcome.accentHint")}</span>
        </span>
        <LayoutGroup>
          <div role="radiogroup" aria-label={t("serversettings.nav.accentColor")} className="flex flex-wrap items-center gap-2">
            <Swatch label={t("serversettings.welcome.ownHue")} selected={look.accentColor === undefined} onSelect={() => onChange({ ...look, accentColor: undefined })}>
              <span style={accentVars({ id: server.id, accentColor: undefined })} className="grid size-full place-items-center rounded-full bg-[var(--accent-server)]">
                <WandSparklesIcon className="size-3.5 text-white" />
              </span>
            </Swatch>
            <AnimatePresence initial={false}>
              {colors.map((c, n) => (
                <motion.span key={c} initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={{ ...SPRING, delay: n * 0.04 }}>
                  <Swatch label={t("serversettings.welcome.fromBanner", { color: hex(c) })} selected={look.accentColor === c} onSelect={() => onChange({ ...look, accentColor: c })}>
                    <span className="block size-full rounded-full" style={{ background: hex(c) }} />
                  </Swatch>
                </motion.span>
              ))}
            </AnimatePresence>
            <label className={cn("relative flex h-9 items-center gap-1.5 rounded-full border px-2 text-xs font-bold text-muted-foreground transition hover:border-primary/40", custom && "border-foreground ring-1 ring-foreground")}>
              <span className="size-5 rounded-full border" style={{ background: look.accentColor !== undefined ? hex(look.accentColor) : "transparent" }} />
              <Input
                aria-label={t("serversettings.welcome.accentHex")}
                value={look.accentColor !== undefined ? hex(look.accentColor) : ""}
                placeholder="#ff88aa"
                onChange={(e) => {
                  const c = parseHex(e.target.value);
                  if (c !== null) onChange({ ...look, accentColor: c });
                }}
                className="h-7 w-20 border-0 bg-transparent px-0 font-mono text-xs shadow-none focus-visible:ring-0"
              />
              <input
                type="color"
                aria-label={t("serversettings.welcome.pickAccent")}
                value={look.accentColor !== undefined ? hex(look.accentColor) : "#ff88aa"}
                onChange={(e) => onChange({ ...look, accentColor: parseHex(e.target.value) ?? undefined })}
                className="size-6 cursor-pointer rounded-full border-0 bg-transparent p-0"
              />
            </label>
          </div>
        </LayoutGroup>
      </div>
    </div>
  );
}

function Swatch({ label, selected, onSelect, children }: { label: string; selected: boolean; onSelect: () => void; children: ReactNode }) {
  return (
    <motion.button
      type="button"
      role="radio"
      aria-checked={selected}
      aria-label={label}
      title={label}
      onClick={onSelect}
      whileHover={{ scale: 1.1 }}
      whileTap={{ scale: 0.9 }}
      className="relative grid size-9 place-items-center rounded-full p-1"
    >
      {selected && <motion.span layoutId="accent-ring" transition={SPRING} className="absolute inset-0 rounded-full ring-2 ring-foreground" />}
      {children}
      <AnimatePresence>
        {selected && (
          <motion.span initial={{ scale: 0 }} animate={{ scale: 1 }} exit={{ scale: 0 }} transition={{ type: "spring", stiffness: 600, damping: 16 }} className="absolute -right-0.5 -bottom-0.5 grid size-4 place-items-center rounded-full bg-foreground text-background">
            <CheckIcon className="size-2.5" strokeWidth={4} />
          </motion.span>
        )}
      </AnimatePresence>
    </motion.button>
  );
}

/**
 * Where the banner keeps its focus: drag the dot (or use the arrow keys) on
 * the whole banner, and see how a phone header and a Browse card crop it.
 */
function FocusPicker({ server, onChange }: { server: BannerServer; onChange: (x: number, y: number) => void }) {
  const { t } = useI18n();
  const box = useRef<HTMLDivElement>(null);
  const [dragging, setDragging] = useState(false);
  const x = server.bannerFocusX;
  const y = server.bannerFocusY;
  function at(e: ReactPointerEvent) {
    const r = box.current?.getBoundingClientRect();
    if (!r) return;
    const clamp = (v: number) => Math.round(Math.min(100, Math.max(0, v)));
    onChange(clamp(((e.clientX - r.left) / r.width) * 100), clamp(((e.clientY - r.top) / r.height) * 100));
  }
  return (
    <div className="grid gap-3 sm:grid-cols-[minmax(0,1fr)_12rem]">
      <div>
        <p className="mb-1.5 text-sm font-bold">{t("serversettings.welcome.focalPoint")}</p>
        <div
          ref={box}
          role="slider"
          tabIndex={0}
          aria-label={t("serversettings.nav.bannerFocus")}
          aria-valuetext={t("serversettings.welcome.focusAt", { x, y })}
          aria-valuenow={x}
          onPointerDown={(e) => {
            e.currentTarget.setPointerCapture(e.pointerId);
            setDragging(true);
            at(e);
          }}
          onPointerMove={(e) => dragging && at(e)}
          onPointerUp={() => setDragging(false)}
          onPointerCancel={() => setDragging(false)}
          onLostPointerCapture={() => setDragging(false)}
          onKeyDown={(e) => {
            const step = e.shiftKey ? 10 : 2;
            const move: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
            const m = move[e.key];
            if (!m) return;
            e.preventDefault();
            onChange(Math.min(100, Math.max(0, x + m[0])), Math.min(100, Math.max(0, y + m[1])));
          }}
          className="relative aspect-[5/2] cursor-crosshair touch-none overflow-hidden rounded-2xl border outline-none focus-visible:ring-2 focus-visible:ring-primary"
        >
          <img src={shownPicture(server.bannerUrl)} alt="" draggable={false} className="size-full object-cover select-none" />
          <span aria-hidden className="absolute inset-0 bg-black/10" />
          <span aria-hidden style={{ left: `${x}%`, top: `${y}%` }} className="absolute">
            <motion.span
              animate={{ scale: dragging ? 1.25 : 1 }}
              transition={SPRING}
              className="absolute -top-4 -left-4 grid size-8 place-items-center rounded-full border-2 border-white bg-black/30 shadow-lg backdrop-blur-sm"
            >
              <span className="size-2 rounded-full bg-white" />
            </motion.span>
          </span>
        </div>
      </div>
      <div className="flex flex-col gap-2">
        <Crop label={t("serversettings.welcome.cropPhone")} ratio="aspect-[390/128]" server={server} />
        <Crop label={t("serversettings.welcome.cropBrowse")} ratio="aspect-[300/80]" server={server} />
        <Crop label={t("serversettings.welcome.cropDialog")} ratio="aspect-[672/160]" server={server} />
      </div>
    </div>
  );
}

function Crop({ label, ratio, server }: { label: string; ratio: string; server: BannerServer }) {
  return (
    <div>
      <p className="mb-1 text-[0.65rem] font-bold tracking-wide text-muted-foreground uppercase">{label}</p>
      <div className={cn("overflow-hidden rounded-xl border", ratio)}>
        <img src={shownPicture(server.bannerUrl)} alt="" draggable={false} className="size-full object-cover transition-[object-position] duration-300" style={{ objectPosition: bannerPosition(server) }} />
      </div>
    </div>
  );
}

// ───────────────────────── Preview ─────────────────────────

/**
 * The real screens at true size, 1280 or 390 wide, scaled down to fit: a
 * blurred app behind, and the welcome, applying or onboarding card over it
 * as a dialog (desktop) or a sheet (phone).
 */
function Preview({
  instanceKey,
  server,
  view,
  onView,
  device,
  onDevice,
  welcome,
  onboarding,
  form,
  focusStep,
}: {
  instanceKey: string;
  server: Server;
  view: View;
  onView: (v: View) => void;
  device: Device;
  onDevice: (d: Device) => void;
  welcome: WelcomeScreen;
  onboarding: Onboarding;
  form: JoinForm;
  focusStep: number;
}) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const roles = useRoles(instanceKey, server.id);
  const channels = inst?.channels[server.id] ?? [];
  const emojis = inst?.emojis[server.id];
  const outer = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = outer.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setWidth(e!.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const frame = DEVICES[device];
  const scale = width ? Math.min(device === "phone" ? 0.62 : 1, width / frame.width) : 0;
  const steps = useMemo(() => stepsFor(onboarding, { mustAgree: server.hasRules, canSeeChannel: () => true }), [onboarding, server.hasRules]);
  const phone = device === "phone";

  const views: { id: View; label: string; icon: typeof PartyPopperIcon }[] = [
    { id: "welcome", label: t("serversettings.welcome.viewWelcome"), icon: PartyPopperIcon },
    { id: "apply", label: t("serversettings.welcome.viewApplying"), icon: ClipboardPenIcon },
    { id: "onboarding", label: t("serversettings.nav.onboarding"), icon: SparklesIcon },
  ];

  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <Segments value={view} onChange={onView} options={views} id="preview-view" />
        <Segments value={device} onChange={onDevice} options={(Object.keys(DEVICES) as Device[]).map((d) => ({ id: d, label: DEVICES[d].label, icon: DEVICES[d].icon }))} id="preview-device" />
      </div>
      <div ref={outer} className="relative w-full">
        <div
          className={cn("relative mx-auto overflow-hidden rounded-3xl border bg-background shadow-xl", phone && "rounded-[2.2rem] border-4 border-foreground/80")}
          style={{ width: frame.width * scale, height: frame.height * scale }}
        >
          {scale > 0 && (
            <div style={{ width: frame.width, height: frame.height, transform: `scale(${scale})`, transformOrigin: "top left" }} className="absolute top-0 left-0">
              <FakeApp server={server} phone={phone} />
              <div className={cn("absolute inset-0 grid bg-black/50", phone ? "place-items-end" : "place-items-center p-8")}>
                <AnimatePresence mode="wait" initial={false}>
                  <motion.div
                    key={`${view}:${device}`}
                    initial={{ opacity: 0, y: 40, scale: 0.96 }}
                    animate={{ opacity: 1, y: 0, scale: 1 }}
                    exit={{ opacity: 0, y: 24, scale: 0.97 }}
                    transition={SPRING}
                    className={cn("scroll-thin max-h-[92%] w-full overflow-x-hidden overflow-y-auto border bg-card p-6 text-card-foreground shadow-2xl", phone ? "rounded-t-3xl" : "max-w-2xl rounded-3xl")}
                  >
                    {view === "welcome" ? (
                      welcome.enabled || welcome.description || welcome.channels.length ? (
                        <WelcomeCard server={server} screen={welcome} channels={channels} emojis={emojis} bleed wide={!phone} onPick={() => {}} />
                      ) : (
                        <Empty server={server} text={t("serversettings.welcome.emptyWelcome")} />
                      )
                    ) : view === "apply" ? (
                      <ApplyPreview server={server} form={form} />
                    ) : steps.length ? (
                      <OnboardingFlow
                        key={`${focusStep}:${steps.map((s) => s.id).join()}`}
                        server={server}
                        steps={steps}
                        rules={form.rules}
                        channels={channels}
                        roles={roles}
                        emojis={emojis}
                        welcome={welcome}
                        preview
                        compact={false}
                      />
                    ) : (
                      <Empty server={server} text={t("serversettings.welcome.emptyOnboarding")} />
                    )}
                  </motion.div>
                </AnimatePresence>
              </div>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

function Segments<T extends string>({ value, onChange, options, id }: { value: T; onChange: (v: T) => void; options: { id: T; label: string; icon: typeof PartyPopperIcon }[]; id: string }) {
  return (
    <LayoutGroup id={id}>
      <div role="tablist" className="flex rounded-xl bg-muted p-1">
        {options.map((o) => (
          <button
            key={o.id}
            type="button"
            role="tab"
            aria-selected={value === o.id}
            onClick={() => onChange(o.id)}
            className={cn("relative flex h-8 items-center gap-1.5 rounded-lg px-3 text-sm font-bold transition-colors", value === o.id ? "text-foreground" : "text-muted-foreground hover:text-foreground")}
          >
            {value === o.id && <motion.span layoutId="segment" transition={SPRING} className="absolute inset-0 rounded-lg bg-background shadow" />}
            <o.icon className="relative size-4" />
            <span className="relative">{o.label}</span>
          </button>
        ))}
      </div>
    </LayoutGroup>
  );
}

/** The app behind the preview's dialog: rail, channels and a few message lines, as shapes. */
function FakeApp({ server, phone }: { server: Server; phone: boolean }) {
  return (
    <div aria-hidden className="absolute inset-0 flex blur-[1.5px]">
      {!phone && (
        <>
          <div className="flex w-[72px] flex-col items-center gap-2 bg-muted/60 py-3">
            {[0, 1, 2, 3].map((n) => (
              <span key={n} className="size-12 rounded-2xl bg-muted" />
            ))}
          </div>
          <div className="flex w-60 flex-col gap-2 bg-muted/30 p-3">
            <span className="mb-2 h-5 w-32 rounded bg-muted" />
            {[40, 28, 34, 22, 30].map((w, n) => (
              <span key={n} className="h-3.5 rounded bg-muted" style={{ width: `${w * 4}px` }} />
            ))}
          </div>
        </>
      )}
      <div className="flex flex-1 flex-col gap-4 p-6">
        <span className="h-5 w-40 rounded bg-muted" />
        {[70, 50, 85, 40, 60, 75].map((w, n) => (
          <div key={n} className="flex items-start gap-3">
            <span className="size-10 shrink-0 rounded-full bg-muted" style={accentVars(server)} />
            <div className="flex flex-1 flex-col gap-1.5">
              <span className="h-3 w-24 rounded bg-muted" />
              <span className="h-3 rounded bg-muted/70" style={{ width: `${w}%` }} />
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

function Empty({ server, text }: { server: Server; text: string }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-4">
      <BannerHero server={server} bleed eyebrow={t("settings.controls.preview")} />
      <p className="rounded-2xl bg-muted/50 p-4 text-center text-sm text-muted-foreground">{text}</p>
    </div>
  );
}

/** Applying, as someone sees it: the banner, the rules and questions, and the button. */
function ApplyPreview({ server, form }: { server: Server; form: JoinForm }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-4">
      <BannerHero server={server} bleed eyebrow={t("join.apply")} badge={<ClipboardPenIcon className="size-3.5" />}>
        <p className="mt-1 text-sm text-muted-foreground">{t("join.applyDialog.description")}</p>
      </BannerHero>
      {form.rules.length > 0 && (
        <section className="flex flex-col gap-2">
          <h3 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("join.applyDialog.rules")}</h3>
          <RulesList rules={form.rules} className="max-h-56 overflow-hidden" />
        </section>
      )}
      {form.questions.length > 0 ? (
        <section className="flex flex-col gap-3">
          <h3 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("join.applyDialog.questions")}</h3>
          {form.questions.map((q, n) => (
            <motion.div key={`${n}:${q.prompt}`} initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} transition={{ ...SPRING, delay: 0.05 * n }} className="flex flex-col gap-1.5">
              <span className="text-sm font-bold">
                {q.prompt}
                {q.required && <span className="text-destructive"> *</span>}
              </span>
              {q.paragraph ? <Textarea rows={3} readOnly tabIndex={-1} className="rounded-xl" /> : <Input readOnly tabIndex={-1} className="h-11 rounded-xl" />}
            </motion.div>
          ))}
        </section>
      ) : (
        <p className="rounded-2xl bg-muted/50 p-3 text-sm text-muted-foreground">{t("serversettings.welcome.noQuestions")}</p>
      )}
      <Button type="button" tabIndex={-1} style={{ background: "var(--accent-server)", ...accentVars(server) }} className="h-11 rounded-xl font-bold text-white">
        <SendIcon /> {t("join.applyDialog.send")}
      </Button>
    </div>
  );
}
