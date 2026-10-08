import { CheckIcon, FileJsonIcon, ImagePlusIcon, LoaderCircleIcon, PencilIcon, SparklesIcon, Trash2Icon, WandIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useMemo, useRef, useState, type DragEvent, type ReactNode } from "react";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import { ProfileItemKind, type ProfileItem, type User } from "@/gen/fuwa/v1/types_pb";
import { createProfileItem, deleteProfileItem, loadInstanceProfileItems, run, updateProfileItem, uploadPicture, type ItemChange } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { Count } from "@/components/motion";
import { DecorationImage } from "@/components/ProfileDecoration";
import { ProfileEffect } from "@/components/ProfileEffect";
import { cardColor, MiniCard } from "@/components/settings/account/EffectPicker";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { type Key, useI18n } from "@/i18n/react";
import type { ProfileEffectSpec } from "@/lib/effects/profile";
import { formatBytes } from "@/lib/format";
import { SLIDE_IN, SPRING } from "@/lib/motion";
import { PICTURE_TYPES } from "@/lib/pictures";
import { checkEffectText, ITEM_DECORATION, ITEM_EFFECT, itemEffect, itemsOfKind, nameFromFile, type EffectProblem } from "@/lib/profile-items";
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

/** An effect about to be added: its spec (a file or pasted), name and line, playing as it will. */
function NewEffect({ instanceKey, serverId, me, onDone }: { instanceKey: string; serverId: string; me: User; onDone: () => void }) {
  const { t } = useI18n();
  const [text, setText] = useState("");
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const checked = useMemo(() => checkEffectText(text), [text]);

  async function add() {
    if (!("spec" in checked)) return;
    setBusy(true);
    setError(null);
    try {
      await run(createProfileItem(instanceKey, serverId, { kind: ProfileItemKind.EFFECT, name: name.trim(), description: description.trim(), effect: text }));
      onDone();
    } catch (err) {
      setError((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <DraftPanel title={t("serversettings.profileItems.newEffect")} error={error} busy={busy} canAdd={"spec" in checked && !!name.trim()} onAdd={() => void add()} onCancel={onDone}>
      <EffectSource
        text={text}
        me={me}
        disabled={busy}
        onText={(v, fileName) => {
          setText(v);
          setError(null);
          if (fileName && !name.trim()) setName(nameFromFile(fileName));
        }}
        problem={"problem" in checked ? checked.problem : null}
        spec={"spec" in checked ? checked.spec : null}
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

/** A spec to paste or load from a `.json` file, beside a card playing it once it's good. */
function EffectSource({
  text,
  me,
  disabled,
  onText,
  problem,
  spec,
}: {
  text: string;
  me: User;
  disabled?: boolean;
  onText: (text: string, fileName?: string) => void;
  problem: EffectProblem | null;
  spec: ProfileEffectSpec | null;
}) {
  const { t } = useI18n();
  const file = useRef<HTMLInputElement>(null);

  async function load(f: File | undefined) {
    if (!f) return;
    onText(await f.text(), f.name);
  }

  return (
    <div className="flex flex-col gap-3 sm:flex-row">
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        <Textarea
          value={text}
          disabled={disabled}
          spellCheck={false}
          aria-label={t("serversettings.profileItems.spec")}
          placeholder={t("serversettings.profileItems.specPlaceholder")}
          onChange={(e) => onText(e.target.value)}
          className="min-h-36 rounded-xl font-mono text-xs"
        />
        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" variant="outline" className="rounded-full" disabled={disabled} onClick={() => file.current?.click()}>
            <FileJsonIcon /> {t("serversettings.profileItems.openFile")}
          </Button>
          <AnimatePresence mode="wait" initial={false}>
            {text.trim() && problem && (
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
      <div className="w-32 shrink-0 self-center sm:self-start">
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
              : !spec && t("serversettings.profileItems.cantPlay")}
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
            <ReplaceEffect item={item} me={me} onSave={(effect) => change({ effect })} onDone={() => setReplacing(false)} />
          </motion.div>
        )}
      </AnimatePresence>
    </motion.li>
  );
}

/** A new spec for an effect, starting from the one it has. */
function ReplaceEffect({ item, me, onSave, onDone }: { item: ProfileItem; me: User; onSave: (effect: string) => Promise<unknown>; onDone: () => void }) {
  const { t } = useI18n();
  const [text, setText] = useState(() => pretty(item.effect));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const checked = useMemo(() => checkEffectText(text), [text]);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      await onSave(text);
      onDone();
    } catch (err) {
      setError((err as FuwaError).message);
    } finally {
      setBusy(false);
    }
  }

  return (
    <DraftPanel title={t("serversettings.profileItems.replace")} error={error} busy={busy} canAdd={"spec" in checked && text !== pretty(item.effect)} onAdd={() => void save()} onCancel={onDone}>
      <EffectSource
        text={text}
        me={me}
        disabled={busy}
        onText={(v) => {
          setText(v);
          setError(null);
        }}
        problem={"problem" in checked ? checked.problem : null}
        spec={"spec" in checked ? checked.spec : null}
      />
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
