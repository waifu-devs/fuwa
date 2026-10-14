import {
  BookOpenIcon,
  CheckIcon,
  CodeIcon,
  DownloadIcon,
  FileJsonIcon,
  ImagePlusIcon,
  LoaderCircleIcon,
  PencilIcon,
  SparklesIcon,
  Trash2Icon,
  TriangleAlertIcon,
  WandIcon,
  WandSparklesIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useRef, useState, type DragEvent, type ReactNode } from "react";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import { ProfileItemKind, type ProfileItem, type User } from "@/gen/fuwa/v1/types_pb";
import { createProfileItem, deleteProfileItem, loadInstanceProfileItems, run, updateProfileItem, uploadPicture, type ItemChange } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { UserAvatar } from "@/components/Icons";
import { Count, SwapText } from "@/components/motion";
import { DecorationImage } from "@/components/ProfileDecoration";
import { ProfileEffect } from "@/components/ProfileEffect";
import { cardColor, MiniCard } from "@/components/settings/account/EffectPicker";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { type Key, useI18n } from "@/i18n/react";
import { shaderId, shaderProblem, STARTERS as SHADER_STARTERS } from "@/lib/effects/custom";
import { BUILTIN_EFFECTS, MAX_PICTURES } from "@/lib/effects/profile";
import { retryShader, useShaderStatus } from "@/lib/effects/status";
import { effectKeys } from "@/lib/effects/text";
import { formatBytes } from "@/lib/format";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { instanceHas } from "@/lib/compat";
import { PICTURE_TYPES } from "@/lib/pictures";
import { shownPicture } from "@/lib/shown";
import {
  checkEffectText,
  DECORATION_DOCS,
  DECORATION_PROMPT,
  EFFECT_DOCS,
  EFFECT_PROMPT,
  ITEM_DECORATION,
  ITEM_EFFECT,
  itemEffect,
  itemsOfKind,
  joinShader,
  nameFromFile,
  splitShader,
  starterText,
  TEMPLATE_AVATAR,
  TEMPLATE_SIZE,
  type EffectProblem,
} from "@/lib/profile-items";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const NAME_MAX = 40;
const DESCRIPTION_MAX = 120;

const PROBLEM: Record<EffectProblem, Key> = {
  empty: "serversettings.profileItems.effectEmpty",
  json: "serversettings.profileItems.effectNotJson",
  spec: "serversettings.profileItems.effectNothing",
};

type Draft = { kind: "decoration"; file: File } | { kind: "effect" };

/**
 * The profile effects and avatar decorations offered here
 * (docs/profile-items.md): the instance's (no `serverId`, for its admins)
 * or a server's (for those with Manage Server). Decorations are pictures
 * dropped in; effects are specs, a `.json` file or pasted, checked and
 * played before they're added. Each can be renamed, described, an effect
 * given a new spec, and deleted, which takes it off everyone wearing it.
 */
export function ProfileItems({ instanceKey, serverId = "" }: { instanceKey: string; serverId?: string }) {
  const { t } = useI18n();
  const inst = useInstance(instanceKey);
  const items = serverId ? inst?.serverProfileItems[serverId] : inst?.profileItems;
  const me = inst?.me;
  const [loaded, setLoaded] = useState(!!serverId);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [dragging, setDragging] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const shake = useAnimationControls();

  // The instance's list is kept from sign-in; read it again here so an admin sees what's there now.
  useEffect(() => {
    if (serverId) return;
    run(loadInstanceProfileItems(instanceKey)).then(
      () => setLoaded(true),
      () => setLoaded(true),
    );
  }, [instanceKey, serverId]);

  const decorations = itemsOfKind(items, ITEM_DECORATION);
  const effects = itemsOfKind(items, ITEM_EFFECT);

  function pickPicture(files: File[]) {
    const file = files.find((f) => PICTURE_TYPES.includes(f.type));
    if (!file) {
      if (files.length) {
        toast(t("serversettings.profileItems.badType"));
        void shake.start({ x: [0, -8, 7, -5, 3, 0], transition: { duration: 0.4 } });
      }
      return;
    }
    setDraft({ kind: "decoration", file });
  }

  const drop = {
    onDragOver: (e: DragEvent) => {
      if (!e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      setDragging(true);
    },
    onDragLeave: (e: DragEvent) => {
      if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
    },
    onDrop: (e: DragEvent) => {
      e.preventDefault();
      setDragging(false);
      pickPicture([...e.dataTransfer.files]);
    },
  };

  if (!me) return null;
  return (
    <div className="flex flex-col gap-5">
      <p className="text-sm text-muted-foreground">{t(serverId ? "serversettings.profileItems.introServer" : "serversettings.profileItems.introInstance")}</p>
      <div className="grid gap-3 sm:grid-cols-2">
        <div className="flex flex-col gap-2">
          <motion.button
            type="button"
            animate={shake}
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.98 }}
            onClick={() => input.current?.click()}
            {...drop}
            className={cn(
              "group relative flex flex-col items-center gap-2 overflow-hidden rounded-3xl border-2 border-dashed p-5 text-center transition-colors",
              dragging ? "border-primary bg-primary/10" : "border-border hover:border-primary/50 hover:bg-primary/5",
            )}
          >
            <motion.span
              animate={dragging ? { scale: 1.2, rotate: -12, y: -4 } : { scale: 1, rotate: 0, y: 0 }}
              transition={{ type: "spring", stiffness: 500, damping: 14 }}
              className="grid size-11 place-items-center rounded-2xl bg-primary/15 text-primary"
            >
              <ImagePlusIcon className="size-5 transition group-hover:rotate-12" />
            </motion.span>
            <span className="font-extrabold">{dragging ? t("serversettings.emoji.letGo") : t("serversettings.profileItems.addDecoration")}</span>
            <span className="max-w-xs text-xs text-muted-foreground">{t("serversettings.profileItems.addDecorationHint")}</span>
          </motion.button>
          <Helpers>
            <TemplateButton />
            <CopyPrompt text={DECORATION_PROMPT} hint={t("serversettings.profileItems.askAiDecorationHint")} />
            <GuideLink href={DECORATION_DOCS} />
          </Helpers>
        </div>
        <div className="flex flex-col gap-2">
          <motion.button
            type="button"
            whileHover={{ y: -2 }}
            whileTap={{ scale: 0.98 }}
            onClick={() => setDraft({ kind: "effect" })}
            className="group relative flex flex-col items-center gap-2 overflow-hidden rounded-3xl border-2 border-dashed border-border p-5 text-center transition-colors hover:border-primary/50 hover:bg-primary/5"
          >
            <span className="grid size-11 place-items-center rounded-2xl bg-primary/15 text-primary">
              <SparklesIcon className="size-5 transition group-hover:scale-110 group-hover:-rotate-12" />
            </span>
            <span className="font-extrabold">{t("serversettings.profileItems.addEffect")}</span>
            <span className="max-w-xs text-xs text-muted-foreground">{t("serversettings.profileItems.addEffectHint")}</span>
          </motion.button>
          <Helpers>
            <CopyPrompt text={EFFECT_PROMPT} hint={t("serversettings.profileItems.askAiEffectHint")} />
            <GuideLink href={EFFECT_DOCS} />
          </Helpers>
        </div>
      </div>
      <input
        ref={input}
        type="file"
        accept={PICTURE_TYPES.join(",")}
        className="hidden"
        onChange={(e) => {
          pickPicture([...(e.target.files ?? [])]);
          e.target.value = "";
        }}
      />

      <AnimatePresence mode="popLayout" initial={false}>
        {draft?.kind === "decoration" && (
          <motion.div key={`decoration-${draft.file.name}-${draft.file.lastModified}`} {...SLIDE_IN} transition={SPRING}>
            <NewDecoration instanceKey={instanceKey} serverId={serverId} me={me} file={draft.file} onDone={() => setDraft(null)} />
          </motion.div>
        )}
        {draft?.kind === "effect" && (
          <motion.div key="effect" {...SLIDE_IN} transition={SPRING}>
            <NewEffect instanceKey={instanceKey} serverId={serverId} me={me} onDone={() => setDraft(null)} />
          </motion.div>
        )}
      </AnimatePresence>

      {!items || !loaded ? (
        <div className="grid gap-2 sm:grid-cols-2">
          {[0, 1, 2, 3].map((n) => (
            <div key={n} className="shimmer h-20 rounded-2xl" />
          ))}
        </div>
      ) : (
        <>
          <Section title={t("serversettings.profileItems.decorations")} count={decorations.length} empty={t("serversettings.profileItems.noDecorations")}>
            {decorations.map((item) => (
              <ItemRow key={item.id} instanceKey={instanceKey} serverId={serverId} item={item} me={me} />
            ))}
          </Section>
          <Section title={t("serversettings.profileItems.effects")} count={effects.length} empty={t("serversettings.profileItems.noEffects")}>
            {effects.map((item) => (
              <ItemRow key={item.id} instanceKey={instanceKey} serverId={serverId} item={item} me={me} />
            ))}
          </Section>
        </>
      )}
    </div>
  );
}

/** One kind's items under a heading, or a line saying there are none. */
function Section({ title, count, empty, children }: { title: string; count: number; empty: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-2">
      <h3 className="flex items-center gap-2 text-xs font-extrabold tracking-wide text-muted-foreground uppercase">
        {title}
        <span className="rounded-full bg-muted px-2 py-0.5 tabular-nums">
          <Count value={count} />
        </span>
      </h3>
      <AnimatePresence initial={false}>
        {count === 0 && (
          <motion.p key="empty" {...SLIDE_IN} transition={SPRING} className="text-sm text-muted-foreground">
            {empty}
          </motion.p>
        )}
      </AnimatePresence>
      <ul className="grid gap-2 sm:grid-cols-2">
        <AnimatePresence initial={false}>{children}</AnimatePresence>
      </ul>
    </section>
  );
}

/** Your avatar with a decoration around it, as people will see it. */
function DecorationPreview({ me, item, className }: { me: User; item: Pick<ProfileItem, "pictureUrl" | "animated">; className?: string }) {
  return (
    <span className={cn("relative block size-12 shrink-0", className)}>
      <UserAvatar user={me} className="size-full" />
      <DecorationImage item={item} />
    </span>
  );
}

/** A picture on a fuwa instance isn't there yet for a file not uploaded: drawn from the file itself, the same way. */
function LocalDecoration({ me, file }: { me: User; file: File }) {
  const [url, setUrl] = useState<string>();
  useEffect(() => {
    const u = URL.createObjectURL(file);
    setUrl(u);
    return () => URL.revokeObjectURL(u);
  }, [file]);
  return (
    <span className="relative block size-20 shrink-0">
      <UserAvatar user={me} className="size-full text-2xl" />
      {url && <img src={url} alt="" aria-hidden draggable={false} className="pointer-events-none absolute -inset-[10%] z-[1] size-[120%] max-w-none object-contain" />}
    </span>
  );
}

/** Name and description fields, shared by both new kinds. */
function NameFields({ name, description, onName, onDescription, disabled }: { name: string; description: string; onName: (v: string) => void; onDescription: (v: string) => void; disabled?: boolean }) {
  const { t } = useI18n();
  return (
    <div className="flex min-w-0 flex-1 flex-col gap-2">
      <Input value={name} maxLength={NAME_MAX} disabled={disabled} placeholder={t("serversettings.profileItems.namePlaceholder")} aria-label={t("serversettings.profileItems.name")} onChange={(e) => onName(e.target.value)} className="h-10 rounded-xl" />
      <Input
        value={description}
        maxLength={DESCRIPTION_MAX}
        disabled={disabled}
        placeholder={t("serversettings.profileItems.descriptionPlaceholder")}
        aria-label={t("serversettings.profileItems.description")}
        onChange={(e) => onDescription(e.target.value)}
        className="h-10 rounded-xl"
      />
    </div>
  );
}

/** A panel for something about to be added: its preview and fields, then Add or Cancel. */
function DraftPanel({ title, children, error, busy, progress, canAdd, onAdd, onCancel }: { title: string; children: ReactNode; error: string | null; busy: boolean; progress?: number; canAdd: boolean; onAdd: () => void; onCancel: () => void }) {
  const { t } = useI18n();
  return (
    <div className="relative flex flex-col gap-3 overflow-hidden rounded-3xl border bg-background/60 p-4 shadow-sm">
      {progress !== undefined && (
        <motion.span className="absolute inset-x-0 top-0 h-1 origin-left bg-primary" initial={{ scaleX: 0 }} animate={{ scaleX: progress }} transition={{ ease: "easeOut" }} />
      )}
      <p className="text-sm font-extrabold">{title}</p>
      {children}
      <AnimatePresence initial={false}>
        {error && (
          <motion.p key={error} {...SLIDE_IN} transition={SPRING} role="alert" className="text-sm text-destructive first-letter:uppercase">
            {error}
          </motion.p>
        )}
      </AnimatePresence>
      <div className="flex justify-end gap-2">
        <Button type="button" variant="ghost" className="rounded-full" disabled={busy} onClick={onCancel}>
          {t("serversettings.profileItems.cancel")}
        </Button>
        <Button type="button" className="rounded-full font-bold" disabled={!canAdd || busy} onClick={onAdd}>
          {busy ? <LoaderCircleIcon className="animate-spin" /> : <CheckIcon />}
          {t("serversettings.profileItems.add")}
        </Button>
      </div>
    </div>
  );
}

/** A decoration about to be added: the picture around your avatar, its name and line, then uploading it. */
function NewDecoration({ instanceKey, serverId, me, file, onDone }: { instanceKey: string; serverId: string; me: User; file: File; onDone: () => void }) {
  const { t } = useI18n();
  const [name, setName] = useState(() => nameFromFile(file.name));
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState<number | undefined>();
  const [error, setError] = useState<string | null>(null);

  async function add() {
    setBusy(true);
    setError(null);
    try {
      const url = await run(uploadPicture(instanceKey, MediaPurpose.DECORATION, file, (s) => setSent(s * 0.9), serverId));
      await run(createProfileItem(instanceKey, serverId, { kind: ProfileItemKind.DECORATION, name: name.trim(), description: description.trim(), pictureUrl: url }));
      setSent(1);
      onDone();
    } catch (err) {
      setError((err as FuwaError).message || t("serversettings.emoji.uploadFailed"));
      setSent(undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <DraftPanel title={t("serversettings.profileItems.newDecoration")} error={error} busy={busy} progress={sent} canAdd={!!name.trim()} onAdd={() => void add()} onCancel={onDone}>
      <div className="flex items-center gap-5 px-2">
        <LocalDecoration me={me} file={file} />
        <NameFields
          name={name}
          description={description}
          disabled={busy}
          onName={(v) => {
            setName(v);
            setError(null);
          }}
          onDescription={setDescription}
        />
      </div>
      <p className="text-xs text-muted-foreground">{t("serversettings.profileItems.decorationTip")}</p>
    </DraftPanel>
  );
}

/** One of an effect's pictures while it's edited: on the instance already, or a file to upload (shown from this page). */
type DraftPicture = { url: string; animated: boolean; file?: File };

/** How long the shader's code rests before the preview compiles it. */
const SHADER_REST_MS = 450;

/**
 * An effect being written: the spec's JSON, its shader's WGSL apart from it,
 * and its pictures, checked together and played as they'll be. `item` is the
 * one being changed, if any.
 */
function useEffectDraft(item?: ProfileItem) {
  const [initial] = useState(() => {
    if (!item) return { json: "", wgsl: "" };
    const parts = splitShader(item.effect);
    return parts.wgsl ? parts : { json: pretty(item.effect), wgsl: "" };
  });
  const [text, setText] = useState(initial.json);
  const [wgsl, setWgsl] = useState(initial.wgsl);
  const [pictures, setPictures] = useState<DraftPicture[]>(() => item?.pictures.map((p) => ({ url: p.url, animated: p.animated })) ?? []);
  // The preview takes the code once typing rests, so it isn't compiled on every key.
  const [resting, setResting] = useState(wgsl);
  useEffect(() => {
    const timer = setTimeout(() => setResting(wgsl), SHADER_REST_MS);
    return () => clearTimeout(timer);
  }, [wgsl]);
  const checked = useMemo(() => checkEffectText(joinShader(text, resting), "", pictures), [text, resting, pictures]);
  // Files shown from this page are let go with it.
  const shown = useRef(pictures);
  shown.current = pictures;
  useEffect(
    () => () => {
      for (const p of shown.current) if (p.file) URL.revokeObjectURL(p.url);
    },
    [],
  );
  const effect = () => joinShader(text, wgsl);
  const changed = () => effect() !== joinShader(initial.json, initial.wgsl) || pictures.map((p) => p.url).join() !== (item?.pictures.map((p) => p.url).join() ?? "");
  return { text, setText, wgsl, setWgsl, pictures, setPictures, checked, effect, changed, shaderAlone: !text.trim() && !!wgsl.trim() };
}

type EffectDraft = ReturnType<typeof useEffectDraft>;

/** Uploads a draft's new pictures (for the server, or the instance), in order, and hands back every picture's link. */
async function sendPictures(instanceKey: string, serverId: string, draft: EffectDraft, progress: (sent: number) => void): Promise<string[]> {
  const files = draft.pictures.filter((p) => p.file).length;
  let done = 0;
  const urls: string[] = [];
  for (const picture of draft.pictures) {
    if (!picture.file) {
      urls.push(picture.url);
      continue;
    }
    const url = await run(uploadPicture(instanceKey, MediaPurpose.DECORATION, picture.file, (s) => progress(((done + s) / files) * 0.9), serverId));
    done++;
    urls.push(url);
    // Uploaded once: trying again after a refusal doesn't upload it again.
    URL.revokeObjectURL(picture.url);
    draft.setPictures((list) => list.map((p) => (p === picture ? { url, animated: p.animated } : p)));
  }
  return urls;
}

/** An effect about to be added: its spec (a file or pasted), pictures and shader, name and line, playing as it will. */
function NewEffect({ instanceKey, serverId, me, onDone }: { instanceKey: string; serverId: string; me: User; onDone: () => void }) {
  const { t } = useI18n();
  const draft = useEffectDraft();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState<number | undefined>();
  const [error, setError] = useState<string | null>(null);
  const { checked } = draft;

  async function add() {
    if (!("spec" in checked)) return;
    setBusy(true);
    setError(null);
    try {
      const pictureUrls = await sendPictures(instanceKey, serverId, draft, setSent);
      await run(createProfileItem(instanceKey, serverId, { kind: ProfileItemKind.EFFECT, name: name.trim(), description: description.trim(), effect: draft.effect(), pictureUrls }));
      setSent(1);
      onDone();
    } catch (err) {
      setError((err as FuwaError).message);
      setSent(undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <DraftPanel title={t("serversettings.profileItems.newEffect")} error={error} busy={busy} progress={sent} canAdd={"spec" in checked && !!name.trim()} onAdd={() => void add()} onCancel={onDone}>
      <Starters
        disabled={busy}
        onPick={(spec, starterName) => {
          draft.setText(spec);
          setError(null);
          if (!name.trim()) setName(starterName);
        }}
      />
      <EffectSource
        instanceKey={instanceKey}
        draft={draft}
        me={me}
        disabled={busy}
        onEdit={(fileName) => {
          setError(null);
          if (fileName && !name.trim()) setName(nameFromFile(fileName));
        }}
      />
      <NameFields
        name={name}
        description={description}
        disabled={busy}
        onName={(v) => {
          setName(v);
          setError(null);
        }}
        onDescription={setDescription}
      />
    </DraftPanel>
  );
}

/**
 * A spec to paste or load from a `.json` file, the pictures its layers draw
 * and its shader's WGSL, beside a card playing it once it's good.
 */
function EffectSource({ instanceKey, draft, me, disabled, onEdit }: { instanceKey: string; draft: EffectDraft; me: User; disabled?: boolean; onEdit: (fileName?: string) => void }) {
  const { t } = useI18n();
  // Instances from before them would refuse pictures and drop a shader.
  const media = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "profile-effect-media"));
  const file = useRef<HTMLInputElement>(null);
  const problem = "problem" in draft.checked ? draft.checked.problem : null;
  const spec = "spec" in draft.checked ? draft.checked.spec : null;

  // A spec with a shader in it (a file, a paste, an AI's answer) has its code moved to the shader's own field.
  function setText(text: string, fileName?: string) {
    const parts = splitShader(text);
    draft.setText(parts.json);
    if (parts.wgsl) draft.setWgsl(parts.wgsl);
    onEdit(fileName);
  }

  async function load(f: File | undefined) {
    if (!f) return;
    setText(await f.text(), f.name);
  }

  return (
    <div className="flex flex-col gap-3 sm:flex-row">
      <div className="flex min-w-0 flex-1 flex-col gap-3">
        <div className="flex flex-col gap-2">
          <Textarea
            value={draft.text}
            disabled={disabled}
            spellCheck={false}
            aria-label={t("serversettings.profileItems.spec")}
            placeholder={t("serversettings.profileItems.specPlaceholder")}
            onChange={(e) => setText(e.target.value)}
            className="min-h-36 rounded-xl font-mono text-xs"
          />
          <div className="flex flex-wrap items-center gap-2">
            <Button type="button" size="sm" variant="outline" className="rounded-full" disabled={disabled} onClick={() => file.current?.click()}>
              <FileJsonIcon /> {t("serversettings.profileItems.openFile")}
            </Button>
            <CopyPrompt text={EFFECT_PROMPT} hint={t("serversettings.profileItems.askAiEffectHint")} />
            <GuideLink href={EFFECT_DOCS} />
            <AnimatePresence mode="wait" initial={false}>
              {(draft.text.trim() || draft.wgsl.trim()) && problem && !draft.shaderAlone && (
                <motion.span key={problem} initial={{ opacity: 0, x: -6 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0 }} transition={SPRING} className="text-xs text-destructive">
                  {t(PROBLEM[problem])}
                </motion.span>
              )}
              {spec && (
                <motion.span key="ok" initial={{ opacity: 0, x: -6 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0 }} transition={SPRING} className="flex items-center gap-1 text-xs text-emerald-600 dark:text-emerald-400">
                  <CheckIcon className="size-3.5" /> {t("serversettings.profileItems.effectOk")}
                </motion.span>
              )}
            </AnimatePresence>
          </div>
          <input
            ref={file}
            type="file"
            accept="application/json,.json"
            className="hidden"
            onChange={(e) => {
              void load(e.target.files?.[0]);
              e.target.value = "";
            }}
          />
        </div>
        {media && (
          <>
            <EffectPictures pictures={draft.pictures} disabled={disabled} onChange={(next) => (draft.setPictures(next), onEdit())} />
            <ShaderField wgsl={draft.wgsl} disabled={disabled} onChange={(next) => (draft.setWgsl(next), onEdit())} />
          </>
        )}
      </div>
      <div className="w-32 shrink-0 self-center sm:sticky sm:top-2 sm:self-start">
        <MiniCard userId={me.id} accent={-1}>
          {spec ? (
            <ProfileEffect effect={spec} seed={me.id} color={cardColor(me.id, -1)} measure={false} />
          ) : (
            <span className="absolute inset-x-0 top-[28%] bottom-0 grid place-items-center bg-card/70">
              <WandIcon className="size-5 text-muted-foreground" />
            </span>
          )}
        </MiniCard>
      </div>
    </div>
  );
}

/**
 * An effect's own pictures, in order: each with the number its layers name
 * it by (`"image": 0`), to take out, and a tile to add more (picked or
 * dropped) up to the most an effect has.
 */
function EffectPictures({ pictures, disabled, onChange }: { pictures: DraftPicture[]; disabled?: boolean; onChange: (pictures: DraftPicture[]) => void }) {
  const { t } = useI18n();
  const input = useRef<HTMLInputElement>(null);
  const [dragging, setDragging] = useState(false);
  const shake = useAnimationControls();
  const full = pictures.length >= MAX_PICTURES;

  function add(files: File[]) {
    const fit = files.filter((f) => PICTURE_TYPES.includes(f.type));
    if (files.length && !fit.length) {
      toast(t("serversettings.profileItems.badType"));
      void shake.start({ x: [0, -8, 7, -5, 3, 0], transition: { duration: 0.4 } });
      return;
    }
    const room = MAX_PICTURES - pictures.length;
    if (fit.length > room) toast(t("serversettings.profileItems.picturesFull", { count: MAX_PICTURES }));
    const added = fit.slice(0, Math.max(0, room)).map((file) => ({ url: URL.createObjectURL(file), animated: file.type === "image/gif", file }));
    if (added.length) onChange([...pictures, ...added]);
  }

  function remove(picture: DraftPicture) {
    if (picture.file) URL.revokeObjectURL(picture.url);
    onChange(pictures.filter((p) => p !== picture));
  }

  return (
    <div className="flex flex-col gap-1.5">
      <p className="flex items-baseline gap-2 text-xs">
        <span className="font-extrabold">{t("serversettings.profileItems.pictures")}</span>
        <span className="text-muted-foreground">{t("serversettings.profileItems.picturesHint")}</span>
      </p>
      <motion.ul
        animate={shake}
        className={cn("flex flex-wrap gap-2 rounded-2xl border border-dashed p-2 transition-colors", dragging ? "border-primary bg-primary/10" : "border-border")}
        onDragOver={(e) => {
          if (disabled || full || !e.dataTransfer.types.includes("Files")) return;
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={(e) => {
          if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDragging(false);
        }}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          if (!disabled) add([...e.dataTransfer.files]);
        }}
      >
        <AnimatePresence initial={false} mode="popLayout">
          {pictures.map((picture, n) => (
            <motion.li key={picture.url} layout initial={{ opacity: 0, scale: 0.6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0, scale: 0.6 }} transition={SPRING} className="group/picture relative size-14 rounded-xl bg-muted/60">
              <img src={picture.file ? picture.url : shownPicture(picture.url)} alt="" draggable={false} className="size-full rounded-xl object-contain p-1" />
              <span className="absolute -bottom-1 left-1/2 -translate-x-1/2 rounded-full bg-background px-1.5 font-mono text-[0.65rem] font-bold tabular-nums shadow-sm">
                {n}
              </span>
              <button
                type="button"
                disabled={disabled}
                aria-label={t("serversettings.profileItems.removePicture", { n })}
                onClick={() => remove(picture)}
                className="absolute -top-1.5 -right-1.5 grid size-5 place-items-center rounded-full bg-background text-muted-foreground opacity-0 shadow-sm transition group-hover/picture:opacity-100 hover:text-destructive focus-visible:opacity-100"
              >
                <XIcon className="size-3" />
              </button>
            </motion.li>
          ))}
          {!full && (
            <motion.li key="add" layout transition={SPRING}>
              <motion.button
                type="button"
                disabled={disabled}
                whileHover={{ y: -2 }}
                whileTap={{ scale: 0.94 }}
                aria-label={t("serversettings.profileItems.addPicture")}
                title={t("serversettings.profileItems.addPicture")}
                onClick={() => input.current?.click()}
                className="grid size-14 place-items-center rounded-xl bg-primary/10 text-primary transition-colors hover:bg-primary/20 disabled:opacity-50"
              >
                <ImagePlusIcon className="size-5" />
              </motion.button>
            </motion.li>
          )}
        </AnimatePresence>
      </motion.ul>
      <input
        ref={input}
        type="file"
        multiple
        accept={PICTURE_TYPES.join(",")}
        className="hidden"
        onChange={(e) => {
          add([...(e.target.files ?? [])]);
          e.target.value = "";
        }}
      />
    </div>
  );
}

/**
 * An effect's shader: WGSL in the custom shaders' format, drawn under the
 * particles. Closed until someone adds one; while it's open it says what
 * happened when this device last ran it (from the painter, by its code),
 * and offers the backdrop's starters to begin from.
 */
function ShaderField({ wgsl, disabled, onChange }: { wgsl: string; disabled?: boolean; onChange: (wgsl: string) => void }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(() => !!wgsl.trim());
  const id = wgsl.trim() ? shaderId(wgsl) : null;
  const status = useShaderStatus(id);
  const problem = wgsl.trim() ? shaderProblem(t, wgsl) : null;
  const nothing = t("appsettings.shader.nothing");
  if (wgsl.trim() && !open) setOpen(true);

  const note = problem
    ? { text: problem, bad: true }
    : status?.state === "broken"
      ? { text: status.line === null ? status.message : `${t("appsettings.shader.line", { line: status.line })} ${status.message}`, bad: true }
      : status?.state === "slow"
        ? { text: t("appsettings.shader.slow", { fallback: nothing }), bad: true }
        : status?.state === "stopped"
          ? { text: t("appsettings.shader.stopped", { fallback: nothing }), bad: true }
          : status?.state === "running"
            ? { text: t(status.scale > 0.6 ? "appsettings.shader.running" : status.scale > 0.4 ? "appsettings.shader.runningHalf" : "appsettings.shader.runningThird"), bad: false }
            : null;

  if (!open) {
    return (
      <motion.button
        type="button"
        disabled={disabled}
        whileHover={{ y: -1 }}
        whileTap={{ scale: 0.97 }}
        onClick={() => setOpen(true)}
        className="flex items-center gap-2 self-start rounded-full border border-dashed px-3 py-1.5 text-xs font-bold text-primary transition-colors hover:border-primary/50 hover:bg-primary/5 disabled:opacity-50"
      >
        <CodeIcon className="size-3.5" /> {t("serversettings.profileItems.addShader")}
        <span className="font-normal text-muted-foreground">{t("serversettings.profileItems.addShaderHint")}</span>
      </motion.button>
    );
  }
  return (
    <motion.div {...SLIDE_IN} transition={SPRING} className="flex flex-col gap-1.5">
      <p className="flex items-center gap-2 text-xs">
        <span className="font-extrabold">{t("serversettings.profileItems.shader")}</span>
        <span className="text-muted-foreground">{t("serversettings.profileItems.addShaderHint")}</span>
        <Button
          type="button"
          size="icon"
          variant="ghost"
          disabled={disabled}
          aria-label={t("serversettings.profileItems.removeShader")}
          title={t("serversettings.profileItems.removeShader")}
          className="ml-auto size-7 rounded-full text-muted-foreground hover:text-destructive"
          onClick={() => {
            onChange("");
            setOpen(false);
          }}
        >
          <Trash2Icon />
        </Button>
      </p>
      <div className="flex flex-wrap items-center gap-1.5">
        <span className="mr-1 text-xs font-bold text-muted-foreground">{t("serversettings.profileItems.startFrom")}</span>
        {SHADER_STARTERS.map((starter) => (
          <motion.button
            key={starter.id}
            type="button"
            disabled={disabled}
            whileHover={{ y: -1 }}
            whileTap={{ scale: 0.94 }}
            title={t(starter.hint)}
            onClick={() => onChange(starter.shader.code)}
            className="rounded-full border bg-background/60 px-2.5 py-1 text-xs font-bold transition-colors hover:border-primary/50 hover:bg-primary/10 disabled:opacity-50"
          >
            {starter.name}
          </motion.button>
        ))}
      </div>
      <Textarea
        value={wgsl}
        disabled={disabled}
        spellCheck={false}
        aria-label={t("serversettings.profileItems.shader")}
        placeholder="fn shade(uv: vec2f) -> vec4f { ... }"
        onChange={(e) => onChange(e.target.value)}
        className="min-h-36 rounded-xl font-mono text-xs"
      />
      <AnimatePresence mode="wait" initial={false}>
        {note && (
          <motion.p
            key={note.text}
            initial={{ opacity: 0, x: -6 }}
            animate={{ opacity: 1, x: 0 }}
            exit={{ opacity: 0 }}
            transition={SPRING}
            className={cn("flex flex-wrap items-center gap-2 text-xs", note.bad ? "text-destructive" : "text-emerald-600 dark:text-emerald-400")}
          >
            {note.bad ? <TriangleAlertIcon className="size-3.5" /> : <CheckIcon className="size-3.5" />}
            {note.text}
            {id && (status?.state === "slow" || status?.state === "stopped") && (
              <button type="button" onClick={() => retryShader(id)} className="font-bold text-primary hover:underline">
                {t("appsettings.shader.retry")}
              </button>
            )}
          </motion.p>
        )}
      </AnimatePresence>
    </motion.div>
  );
}

/** One item: its preview, a name and line to change in place, a new spec for an effect, and delete. */
function ItemRow({ instanceKey, serverId, item, me }: { instanceKey: string; serverId: string; item: ProfileItem; me: User }) {
  const lang = useI18n();
  const { t } = lang;
  const [confirm, setConfirm] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const [lively, setLively] = useState(false);
  const spec = item.kind === ITEM_EFFECT ? itemEffect(item) : null;

  const change = (patch: ItemChange) => run(updateProfileItem(instanceKey, serverId, item.id, patch));

  async function remove() {
    await run(deleteProfileItem(instanceKey, serverId, item.id)).catch((err: FuwaError) => toast(err.message));
  }

  return (
    <motion.li
      layout
      initial={{ opacity: 0, scale: 0.9 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.8, filter: "blur(4px)" }}
      transition={SPRING}
      onPointerEnter={() => setLively(true)}
      onPointerLeave={() => setLively(false)}
      className={cn("group flex flex-col gap-3 rounded-2xl border bg-background/50 p-3 transition-colors hover:border-primary/30", replacing && "sm:col-span-2")}
    >
      <div className="flex items-center gap-3">
        {item.kind === ITEM_DECORATION ? (
          <span className="grid size-16 shrink-0 place-items-center">
            <DecorationPreview me={me} item={item} />
          </span>
        ) : (
          <span className="w-14 shrink-0">
            <MiniCard userId={me.id} accent={-1}>
              {spec && <ProfileEffect effect={spec} seed={me.id} color={cardColor(me.id, -1)} play={lively} measure={false} replayOnHover={false} />}
            </MiniCard>
          </span>
        )}
        <span className="flex min-w-0 flex-1 flex-col">
          <InlineField value={item.name} max={NAME_MAX} required label={t("serversettings.profileItems.nameOf", { name: item.name })} className="font-bold" onSave={(name) => change({ name })} />
          <InlineField
            value={item.description}
            max={DESCRIPTION_MAX}
            placeholder={t("serversettings.profileItems.addDescription")}
            label={t("serversettings.profileItems.descriptionOf", { name: item.name })}
            className="text-xs text-muted-foreground"
            onSave={(description) => change({ description })}
          />
          <span className="truncate px-0.5 text-[0.7rem] text-muted-foreground">
            {item.kind === ITEM_DECORATION
              ? [formatBytes(lang, Number(item.size)), item.animated && t("serversettings.emoji.moves")].filter(Boolean).join(" · ")
              : !spec
                ? t("serversettings.profileItems.cantPlay")
                : [
                    item.pictures.length > 0 && `${t("serversettings.profileItems.pictureCount", { count: item.pictures.length })} (${formatBytes(lang, Number(item.size))})`,
                    spec.shader && t("serversettings.profileItems.shader"),
                  ]
                    .filter(Boolean)
                    .join(" · ")}
          </span>
        </span>
        <AnimatePresence mode="popLayout" initial={false}>
          {confirm ? (
            <motion.span key="sure" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={SPRING} className="flex gap-1">
              <Button type="button" size="sm" variant="destructive" className="h-8 rounded-full px-3 text-xs font-bold" onClick={() => void remove()}>
                {t("serversettings.shared.delete")}
              </Button>
              <Button type="button" size="icon" variant="ghost" aria-label={t("serversettings.shared.keepIt")} className="size-8 rounded-full" onClick={() => setConfirm(false)}>
                <XIcon />
              </Button>
            </motion.span>
          ) : (
            <motion.span key="ask" initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING} className="flex gap-1">
              {item.kind === ITEM_EFFECT && (
                <Button
                  type="button"
                  size="icon"
                  variant="ghost"
                  aria-label={t("serversettings.profileItems.replaceNamed", { name: item.name })}
                  title={t("serversettings.profileItems.replace")}
                  className="size-8 rounded-full text-muted-foreground opacity-60 transition hover:text-foreground group-hover:opacity-100"
                  onClick={() => setReplacing((r) => !r)}
                >
                  <PencilIcon />
                </Button>
              )}
              <Button
                type="button"
                size="icon"
                variant="ghost"
                aria-label={t("serversettings.profileItems.deleteNamed", { name: item.name })}
                className="size-8 rounded-full text-muted-foreground opacity-60 transition hover:text-destructive group-hover:opacity-100"
                onClick={() => setConfirm(true)}
              >
                <Trash2Icon />
              </Button>
            </motion.span>
          )}
        </AnimatePresence>
      </div>
      <AnimatePresence initial={false}>
        {confirm && (
          <motion.p key="warn" {...SLIDE_IN} transition={SPRING} className="text-xs text-muted-foreground">
            {t("serversettings.profileItems.deleteWarn")}
          </motion.p>
        )}
        {replacing && (
          <motion.div key="replace" {...SLIDE_IN} transition={SPRING}>
            <ReplaceEffect instanceKey={instanceKey} serverId={serverId} item={item} me={me} onDone={() => setReplacing(false)} />
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** A new spec, pictures and shader for an effect, starting from the ones it has. */
function ReplaceEffect({ instanceKey, serverId, item, me, onDone }: { instanceKey: string; serverId: string; item: ProfileItem; me: User; onDone: () => void }) {
  const { t } = useI18n();
  const draft = useEffectDraft(item);
  const [busy, setBusy] = useState(false);
  const [sent, setSent] = useState<number | undefined>();
  const [error, setError] = useState<string | null>(null);
  const picturesChanged = draft.pictures.map((p) => p.url).join() !== item.pictures.map((p) => p.url).join() || draft.pictures.some((p) => p.file);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const pictures = picturesChanged ? await sendPictures(instanceKey, serverId, draft, setSent) : undefined;
      await run(updateProfileItem(instanceKey, serverId, item.id, { effect: draft.effect(), pictures }));
      setSent(1);
      onDone();
    } catch (err) {
      setError((err as FuwaError).message);
      setSent(undefined);
    } finally {
      setBusy(false);
    }
  }

  return (
    <DraftPanel title={t("serversettings.profileItems.replace")} error={error} busy={busy} progress={sent} canAdd={"spec" in draft.checked && draft.changed()} onAdd={() => void save()} onCancel={onDone}>
      <EffectSource instanceKey={instanceKey} draft={draft} me={me} disabled={busy} onEdit={() => setError(null)} />
    </DraftPanel>
  );
}

/** A spec laid out to read and edit, or as it is if it isn't JSON. */
function pretty(json: string) {
  try {
    return JSON.stringify(JSON.parse(json), null, 2);
  } catch {
    return json;
  }
}

/** A row of small helpers under something to add. */
function Helpers({ children }: { children: ReactNode }) {
  return <div className="flex flex-wrap items-center justify-center gap-x-3 gap-y-1 px-2 text-xs">{children}</div>;
}

/** The built-in effects as specs to start from, each filling in its layers to change. */
function Starters({ disabled, onPick }: { disabled?: boolean; onPick: (spec: string, name: string) => void }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-wrap items-center gap-1.5">
      <span className="mr-1 text-xs font-bold text-muted-foreground">{t("serversettings.profileItems.startFrom")}</span>
      {BUILTIN_EFFECTS.map((effect) => {
        const keys = effectKeys(effect.id);
        const name = keys ? t(keys.name) : effect.name;
        return (
          <motion.button
            key={effect.id}
            type="button"
            disabled={disabled}
            whileHover={{ y: -1 }}
            whileTap={{ scale: 0.94 }}
            title={keys ? t(keys.about) : effect.description}
            onClick={() => onPick(starterText(effect.id) ?? "", name)}
            className="rounded-full border bg-background/60 px-2.5 py-1 text-xs font-bold transition-colors hover:border-primary/50 hover:bg-primary/10 disabled:opacity-50"
          >
            {name}
          </motion.button>
        );
      })}
    </div>
  );
}

/** Copies a prompt for an AI, which the person finishes by describing what they want. */
function CopyPrompt({ text, hint }: { text: string; hint: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout>>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const copy = () => {
    void navigator.clipboard?.writeText(text).then(
      () => {
        setCopied(true);
        clearTimeout(timer.current);
        timer.current = setTimeout(() => setCopied(false), 2400);
      },
      () => toast(t("serversettings.profileItems.askAiFailed")),
    );
  };
  return (
    <motion.button
      type="button"
      onClick={copy}
      title={hint}
      whileTap={{ scale: 0.94 }}
      className={cn("flex items-center gap-1 text-xs font-bold hover:underline", copied ? "text-emerald-500" : "text-primary")}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={copied ? "copied" : "copy"}
          initial={{ opacity: 0, scale: 0.6, rotate: -30 }}
          animate={{ opacity: 1, scale: 1, rotate: 0 }}
          exit={{ opacity: 0, scale: 0.6 }}
          transition={SPRING}
        >
          {copied ? <CheckIcon className="size-3.5" /> : <WandSparklesIcon className="size-3.5" />}
        </motion.span>
      </AnimatePresence>
      <SwapText>{copied ? t("serversettings.profileItems.askAiCopied") : t("serversettings.profileItems.askAi")}</SwapText>
    </motion.button>
  );
}

/** A link to the docs for one kind. */
function GuideLink({ href }: { href: string }) {
  const { t } = useI18n();
  return (
    <a href={href} target="_blank" rel="noreferrer" className="flex items-center gap-1 text-xs font-bold text-primary hover:underline">
      <BookOpenIcon className="size-3.5" /> {t("serversettings.profileItems.guide")}
    </a>
  );
}

/** Downloads a picture to draw a decoration on: the size to make it, with the avatar's circle marked. */
function TemplateButton() {
  const { t } = useI18n();
  const download = () => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = TEMPLATE_SIZE;
    const g = canvas.getContext("2d");
    if (!g) return;
    const mid = TEMPLATE_SIZE / 2;
    g.beginPath();
    g.arc(mid, mid, TEMPLATE_AVATAR / 2, 0, Math.PI * 2);
    g.fillStyle = "rgba(128, 128, 128, 0.35)";
    g.fill();
    g.setLineDash([12, 8]);
    g.lineWidth = 3;
    g.strokeStyle = "rgba(128, 128, 128, 0.9)";
    g.stroke();
    canvas.toBlob((blob) => {
      if (!blob) return;
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = "fuwa-decoration-template.png";
      a.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    }, "image/png");
  };
  return (
    <motion.button
      type="button"
      onClick={download}
      title={t("serversettings.profileItems.templateHint")}
      whileTap={{ scale: 0.94 }}
      className="flex items-center gap-1 text-xs font-bold text-primary hover:underline"
    >
      <DownloadIcon className="size-3.5" /> {t("serversettings.profileItems.template")}
    </motion.button>
  );
}

/** Text changed in place: saved when it loses focus or on Enter, put back on Escape or when the server says no. */
function InlineField({
  value,
  max,
  required = false,
  placeholder,
  label,
  className,
  onSave,
}: {
  value: string;
  max: number;
  required?: boolean;
  placeholder?: string;
  label: string;
  className?: string;
  onSave: (value: string) => Promise<unknown>;
}) {
  const [draft, setDraft] = useState(value);
  const [saving, setSaving] = useState(false);
  const shake = useAnimationControls();
  const [last, setLast] = useState(value);
  if (last !== value) {
    setLast(value);
    setDraft(value);
  }

  async function save() {
    const next = draft.trim();
    if (next === value) return setDraft(value);
    if (required && !next) {
      void shake.start({ x: [0, -6, 5, -3, 0], transition: { duration: 0.35 } });
      return setDraft(value);
    }
    setSaving(true);
    try {
      await onSave(next);
    } catch (err) {
      toast((err as FuwaError).message);
      setDraft(value);
    } finally {
      setSaving(false);
    }
  }

  return (
    <motion.span animate={shake} className="flex min-w-0 items-center">
      <Input
        value={draft}
        aria-label={label}
        placeholder={placeholder}
        maxLength={max}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={() => void save()}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          if (e.key === "Escape") setDraft(value);
        }}
        className={cn("h-7 min-w-0 rounded-md border-transparent bg-transparent px-0.5 shadow-none focus-visible:border-input", className)}
      />
      {saving && <LoaderCircleIcon className="ml-1 size-3.5 shrink-0 animate-spin text-muted-foreground" />}
    </motion.span>
  );
}
