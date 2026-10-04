import { timestampDate } from "@bufbuild/protobuf/wkt";
import { CheckIcon, EyeIcon, PencilLineIcon, PipetteIcon, SparklesIcon, XIcon } from "lucide-react";
import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState, type CSSProperties, type FormEvent, type ReactNode } from "react";
import { loadProfile, run, updateProfile, type ProfilePatch } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { hue, UserAvatar } from "@/components/Icons";
import { Markdown } from "@/components/Markdown";
import { Count, SPRING } from "@/components/motion";
import { PictureField } from "@/components/PictureField";
import { Private } from "@/components/Private";
import { ProfileCard } from "@/components/ProfileCard";
import { Chips, Row, Segmented, Warn } from "@/components/settings/account/common";
import { EffectPicker } from "@/components/settings/account/EffectPicker";
import { SaveBar, WithPreview } from "@/components/settings/controls";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { builtinEffect } from "@/lib/effects/profile";
import { colorCss, shownStatus } from "@/lib/format";
import { reportUsage } from "@/lib/reports";
import { cn } from "@/lib/utils";

const NAME_MAX = 64;
const PRONOUNS_MAX = 40;
const STATUS_MAX = 128;
const BIO_MAX = 2000;

/** When a status clears by itself. "keep" leaves the time it already has. */
type Clear = "keep" | "never" | "30m" | "1h" | "4h" | "today";

const CLEAR: { value: Exclude<Clear, "keep">; label: string }[] = [
  { value: "never", label: "Don't clear" },
  { value: "30m", label: "30 minutes" },
  { value: "1h", label: "1 hour" },
  { value: "4h", label: "4 hours" },
  { value: "today", label: "Today" },
];

function clearsAt(clear: Clear, now = new Date()): Date | null {
  const later = (minutes: number) => new Date(now.getTime() + minutes * 60_000);
  if (clear === "30m") return later(30);
  if (clear === "1h") return later(60);
  if (clear === "4h") return later(240);
  if (clear === "today") {
    const end = new Date(now);
    end.setHours(23, 59, 59, 0);
    return end;
  }
  return null;
}

/** "4:30 PM" today, "Tue 4:30 PM" another day. */
function at(date: Date) {
  const time = date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  return date.toDateString() === new Date().toDateString() ? time : `${date.toLocaleDateString(undefined, { weekday: "short" })} ${time}`;
}

/** Colors for the banner and the card, picked to sit well on every theme. */
const COLORS = [0xff6b9d, 0xf472b6, 0xc084fc, 0x818cf8, 0x60a5fa, 0x22d3ee, 0x34d399, 0xa3e635, 0xfbbf24, 0xfb923c, 0xf87171, 0x94a3b8];

type Draft = {
  displayName: string;
  pronouns: string;
  status: string;
  clear: Clear;
  bio: string;
  avatarUrl: string;
  bannerUrl: string;
  /** 0xRRGGBB, or -1 for the color fuwa picks from your id. */
  accent: number;
  /** A profile effect's id, or "" for none. */
  effect: string;
};

const isUrl = (value: string) => !value.trim() || /^https?:\/\/\S+$/i.test(value.trim());

/**
 * Your profile on this instance, beside a live copy of the card others open
 * from your name: name, pronouns, status, about me, pictures, color and
 * effect.
 */
export function Profile({ instanceKey }: { instanceKey: string }) {
  const inst = useInstance(instanceKey);
  const me = inst?.me;
  const profile = useFuwa((s) => (me ? s.instances[instanceKey]?.profiles[me.id] : undefined));
  const [failed, setFailed] = useState(false);
  const [edits, setEdits] = useState<Partial<Draft>>({});
  const [bioTab, setBioTab] = useState<"write" | "preview">("write");
  const save = useAction(updateProfile);
  // Instances from before profile effects don't say, and can't keep one.
  const effectsOn = useFuwa((s) => !!s.instances[instanceKey]?.node?.profileEffects);

  const meId = me?.id;
  useEffect(() => {
    if (!meId) return;
    run(loadProfile(instanceKey, meId)).catch(() => setFailed(true));
  }, [instanceKey, meId]);

  if (!me) return null;
  const ready = !!profile || failed;
  const status = shownStatus(me);
  const base: Draft = {
    displayName: me.displayName,
    pronouns: profile?.pronouns ?? "",
    status,
    clear: status && me.statusExpiresAt ? "keep" : "never",
    bio: profile?.bio ?? "",
    avatarUrl: me.avatarUrl,
    bannerUrl: profile?.bannerUrl ?? "",
    accent: profile?.accentColor ?? -1,
    effect: profile?.effect ?? "",
  };
  const draft: Draft = { ...base, ...edits };
  const differs = (k: keyof Draft) => edits[k] !== undefined && edits[k] !== base[k];
  const statusChanged = differs("status") || (draft.status.trim() !== "" && differs("clear"));
  const changed = (["displayName", "pronouns", "bio", "avatarUrl", "bannerUrl", "accent", "effect"] as const).filter(differs).length + (statusChanged ? 1 : 0);
  const set = (patch: Partial<Draft>) => {
    setEdits((e) => ({ ...e, ...patch }));
    save.setError(null);
  };
  const keptUntil = me.statusExpiresAt ? timestampDate(me.statusExpiresAt) : null;

  const problem = !draft.displayName.trim()
    ? "Your display name can't be empty."
    : !isUrl(draft.avatarUrl)
      ? "The avatar link needs to start with https://"
      : !isUrl(draft.bannerUrl)
        ? "The banner link needs to start with https://"
        : null;

  async function submit(e?: FormEvent) {
    e?.preventDefault();
    if (problem) return save.setError(problem);
    const patch: ProfilePatch = {};
    if (differs("displayName")) patch.displayName = draft.displayName.trim();
    if (differs("pronouns")) patch.pronouns = draft.pronouns.trim();
    if (differs("bio")) patch.bio = draft.bio.trim();
    if (differs("avatarUrl")) patch.avatarUrl = draft.avatarUrl.trim();
    if (differs("bannerUrl")) patch.bannerUrl = draft.bannerUrl.trim();
    if (differs("accent")) patch.accentColor = draft.accent;
    if (differs("effect")) patch.effect = draft.effect;
    if (statusChanged) {
      patch.status = draft.status.trim();
      patch.statusExpiresAt = draft.clear === "keep" ? keptUntil : clearsAt(draft.clear);
    }
    if (!(await save.go(instanceKey, patch))) return;
    if (patch.effect) reportUsage("profile-effect/picked");
    setEdits({});
  }

  const preview = (
    <ProfileCard
      editing
      me
      instanceKey={instanceKey}
      loading={!ready}
      user={{ ...me, displayName: draft.displayName.trim() || me.username, avatarUrl: isUrl(draft.avatarUrl) ? draft.avatarUrl.trim() : me.avatarUrl, status: draft.status.trim(), statusExpiresAt: undefined }}
      profile={{
        pronouns: draft.pronouns.trim(),
        bio: draft.bio.trim(),
        bannerUrl: isUrl(draft.bannerUrl) ? draft.bannerUrl.trim() : "",
        accentColor: draft.accent < 0 ? undefined : draft.accent,
        createdAt: profile?.createdAt,
        effect: draft.effect,
      }}
    />
  );

  return (
    <form onSubmit={submit}>
      <WithPreview preview={preview}>
        <div className="flex flex-col">
          <Row id="display-name" label="Display name" htmlFor="profile-name" hint={!draft.displayName.trim() ? <Warn>Pick a name people will see.</Warn> : "What people see next to your messages."}>
            <Input
              id="profile-name"
              maxLength={NAME_MAX}
              value={draft.displayName}
              placeholder={me.username}
              onChange={(e) => set({ displayName: e.target.value })}
              className="h-11 rounded-xl"
            />
          </Row>
          <Row id="pronouns" label="Pronouns" htmlFor="profile-pronouns" hint="Shown beside your username on your card.">
            <Input
              id="profile-pronouns"
              maxLength={PRONOUNS_MAX}
              value={draft.pronouns}
              disabled={!ready}
              placeholder="Add your pronouns"
              onChange={(e) => set({ pronouns: e.target.value })}
              className="h-11 max-w-xs rounded-xl"
            />
          </Row>
          <Row id="avatar" label="Avatar" hint={isUrl(draft.avatarUrl) ? "Drop a picture on it or pick one. GIFs keep moving. Without one you get your initial on your own color." : <Warn>Links start with https://</Warn>}>
            <PictureField
              id="profile-avatar"
              instanceKey={instanceKey}
              kind="avatar"
              value={draft.avatarUrl}
              onChange={(avatarUrl) => set({ avatarUrl })}
              fallback={<UserAvatar user={{ ...me, avatarUrl: "" }} className="size-full text-3xl" />}
            />
          </Row>
          <Row id="banner" label="Banner" hint={isUrl(draft.bannerUrl) ? "A wide picture for the top of your card. Without one it's your profile color." : <Warn>Links start with https://</Warn>}>
            <PictureField
              id="profile-banner"
              instanceKey={instanceKey}
              kind="banner"
              value={draft.bannerUrl}
              disabled={!ready}
              onChange={(bannerUrl) => set({ bannerUrl })}
              fallback={
                <span
                  style={draft.accent < 0 ? hue(me.id) : { backgroundColor: colorCss(draft.accent) }}
                  className={cn("block size-full transition-colors duration-500", draft.accent < 0 && "server-gradient")}
                />
              }
            />
          </Row>
          <Row id="profile-color" label="Profile color" hint="Colors your card's banner when there's no picture.">
            <ColorPicker value={draft.accent} userId={me.id} disabled={!ready} onChange={(accent) => set({ accent })} />
          </Row>
          {effectsOn && (
            <Row
              id="profile-effect"
              label="Profile effect"
              hint={builtinEffect(draft.effect)?.description ?? "Plays over your card when people open it. Hovering the card plays it again."}
            >
              <EffectPicker value={draft.effect} userId={me.id} accent={draft.accent} disabled={!ready} onChange={(effect) => set({ effect })} />
            </Row>
          )}
          <Row id="status" label="Custom status" htmlFor="profile-status" hint="Under your name in member lists, on every server here.">
            <div className="relative">
              <Input
                id="profile-status"
                maxLength={STATUS_MAX}
                value={draft.status}
                placeholder="What are you up to?"
                onChange={(e) => set({ status: e.target.value })}
                className="h-11 rounded-xl pr-11"
              />
              <AnimatePresence>
                {draft.status && (
                  <motion.button
                    type="button"
                    initial={{ scale: 0, opacity: 0 }}
                    animate={{ scale: 1, opacity: 1 }}
                    exit={{ scale: 0, opacity: 0 }}
                    transition={SPRING}
                    onClick={() => set({ status: "" })}
                    aria-label="Clear status"
                    className="absolute top-1/2 right-1.5 grid size-8 -translate-y-1/2 place-items-center rounded-lg text-muted-foreground transition hover:bg-muted hover:text-foreground"
                  >
                    <XIcon className="size-4" />
                  </motion.button>
                )}
              </AnimatePresence>
            </div>
            <AnimatePresence initial={false}>
              {draft.status.trim() && (
                <motion.div initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="overflow-hidden">
                  <p className="pt-1 pb-2 text-xs font-bold text-muted-foreground">Clear after</p>
                  <Chips
                    value={draft.clear}
                    onChange={(clear) => set({ clear })}
                    options={[
                      ...(keptUntil && base.clear === "keep"
                        ? [{ value: "keep" as Clear, label: `At ${at(keptUntil)}` }]
                        : []),
                      ...CLEAR,
                    ]}
                  />
                </motion.div>
              )}
            </AnimatePresence>
          </Row>
          <Row
            id="about-me"
            label="About me"
            htmlFor="profile-bio"
            hint={
              <span className="flex flex-wrap items-center justify-between gap-2">
                <span>Markdown works: **bold**, *italics*, `code`, lists and links.</span>
                <span className={cn("font-bold tabular-nums transition-colors", draft.bio.length > BIO_MAX * 0.9 ? "text-amber-500" : "text-muted-foreground")}>
                  <Count value={draft.bio.length} /> / {BIO_MAX}
                </span>
              </span>
            }
          >
            <Segmented
              label="About me"
              value={bioTab}
              onChange={setBioTab}
              className="self-start"
              options={[
                { value: "write", label: "Write", icon: <PencilLineIcon className="size-3.5" /> },
                { value: "preview", label: "Preview", icon: <EyeIcon className="size-3.5" /> },
              ]}
            />
            <AnimatePresence mode="wait" initial={false}>
              {bioTab === "write" ? (
                <motion.div key="write" initial={{ opacity: 0, x: -10 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -10 }} transition={{ duration: 0.15 }}>
                  <Textarea
                    id="profile-bio"
                    maxLength={BIO_MAX}
                    value={draft.bio}
                    disabled={!ready}
                    placeholder="Say a little about yourself"
                    onChange={(e) => set({ bio: e.target.value })}
                    className="min-h-32 rounded-xl"
                  />
                </motion.div>
              ) : (
                <motion.div
                  key="preview"
                  initial={{ opacity: 0, x: 10 }}
                  animate={{ opacity: 1, x: 0 }}
                  exit={{ opacity: 0, x: 10 }}
                  transition={{ duration: 0.15 }}
                  className="min-h-32 rounded-xl border bg-muted/40 px-3 py-2"
                >
                  {draft.bio.trim() ? <Markdown className="text-sm">{draft.bio}</Markdown> : <p className="text-sm text-muted-foreground">Nothing to preview yet.</p>}
                </motion.div>
              )}
            </AnimatePresence>
          </Row>
          <Row id="username" label="Username" hint="Set when the account was made.">
            <p className="text-sm font-bold">
              @<Private text={me.username} kind="name" />
            </p>
          </Row>
        </div>
        <SaveBar
          count={changed}
          saving={save.pending}
          error={save.error}
          onSave={() => void submit()}
          onDiscard={() => {
            setEdits({});
            save.setError(null);
          }}
        />
      </WithPreview>
    </form>
  );
}

/** Swatches for the profile color: fuwa's pick for you, a palette, and any color at all. */
function ColorPicker({ value, userId, onChange, disabled }: { value: number; userId: string; onChange: (value: number) => void; disabled?: boolean }) {
  const custom = value >= 0 && !COLORS.includes(value);
  return (
    <div role="radiogroup" aria-label="Profile color" className={cn("flex flex-wrap gap-2", disabled && "pointer-events-none opacity-50")}>
      <Swatch label="Auto: fuwa picks from your account" active={value < 0} onClick={() => onChange(-1)} style={hue(userId)} className="server-gradient">
        <SparklesIcon className="size-4 text-white drop-shadow" />
      </Swatch>
      {COLORS.map((color) => (
        <Swatch key={color} label={colorCss(color)} active={value === color} onClick={() => onChange(color)} style={{ backgroundColor: colorCss(color) }} />
      ))}
      <motion.label
        whileHover={{ y: -2 }}
        whileTap={{ scale: 0.9 }}
        title="Any color"
        className={cn("relative grid size-9 cursor-pointer place-items-center rounded-full ring-offset-2 ring-offset-background transition-shadow", custom && "ring-2 ring-primary")}
        style={custom ? { backgroundColor: colorCss(value) } : { background: "conic-gradient(from 90deg, #f87171, #fbbf24, #a3e635, #22d3ee, #818cf8, #f472b6, #f87171)" }}
      >
        <PipetteIcon className="size-4 text-white drop-shadow" />
        <input
          type="color"
          aria-label="Any color"
          value={value >= 0 ? colorCss(value) : "#ff6b9d"}
          onChange={(e) => onChange(parseInt(e.target.value.slice(1), 16))}
          className="absolute inset-0 cursor-pointer opacity-0"
        />
      </motion.label>
    </div>
  );
}

function Swatch({
  label,
  active,
  onClick,
  style,
  className,
  children,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
  style: CSSProperties;
  className?: string;
  children?: ReactNode;
}) {
  return (
    <motion.button
      type="button"
      role="radio"
      aria-checked={active}
      aria-label={label}
      title={label}
      whileHover={{ y: -2 }}
      whileTap={{ scale: 0.9 }}
      onClick={onClick}
      style={style}
      className={cn("relative grid size-9 place-items-center rounded-full ring-offset-2 ring-offset-background transition-shadow", active && "ring-2 ring-primary", className)}
    >
      {children}
      <AnimatePresence>
        {active && !children && (
          <motion.span initial={{ scale: 0, rotate: -45 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0 }} transition={{ type: "spring", stiffness: 600, damping: 18 }}>
            <CheckIcon className="size-4 text-white drop-shadow" strokeWidth={3} />
          </motion.span>
        )}
      </AnimatePresence>
    </motion.button>
  );
}
