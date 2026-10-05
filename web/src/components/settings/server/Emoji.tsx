import { CheckIcon, ImagePlusIcon, LoaderCircleIcon, SmilePlusIcon, Trash2Icon, XIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useRef, useState, type DragEvent } from "react";
import { MediaPurpose } from "@/gen/fuwa/v1/media_pb";
import type { Emoji as EmojiT } from "@/gen/fuwa/v1/types_pb";
import { createEmoji, deleteEmoji, renameEmoji, run, serverUsage, uploadPicture } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useInstance } from "@/fuwa/hooks";
import { UserAvatar } from "@/components/Icons";
import { Count, SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { EMOJI_NAME, emojiPicture, nameFromFile } from "@/lib/emoji";
import { displayName, formatBytes } from "@/lib/format";
import { useI18n } from "@/i18n/react";
import { PICTURE_TYPES } from "@/lib/pictures";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/** A file on its way up: its picture, the name it'll get, and how far it's gone. */
type Pending = { key: string; preview: string; name: string; sent: number; error?: string };

/**
 * The server's own emoji: drop pictures in (several at once), name them, and
 * everyone can use them as `:name:`. They count in the server's files.
 */
export function Emoji({ instanceKey, serverId }: { instanceKey: string; serverId: string }) {
  const inst = useInstance(instanceKey);
  const emojis = inst?.emojis[serverId];
  const members = inst?.members[serverId];
  const [pending, setPending] = useState<Pending[]>([]);
  const [dragging, setDragging] = useState(false);
  const [cap, setCap] = useState<number | null>(null);
  const input = useRef<HTMLInputElement>(null);
  const shake = useAnimationControls();

  useEffect(() => {
    run(serverUsage(instanceKey, serverId)).then(
      (r) => setCap(r.limits?.emojis === undefined ? null : Number(r.limits.emojis)),
      () => {},
    );
  }, [instanceKey, serverId]);

  const count = emojis?.length ?? 0;
  const full = cap !== null && count + pending.filter((p) => !p.error).length >= cap;

  async function add(files: File[]) {
    const pictures = files.filter((f) => PICTURE_TYPES.includes(f.type));
    if (pictures.length < files.length) {
      toast("Emoji are PNG, JPEG, GIF, WebP or AVIF pictures.");
      void shake.start({ x: [0, -8, 7, -5, 3, 0], transition: { duration: 0.4 } });
    }
    const taken = new Set((emojis ?? []).map((e) => e.name.toLowerCase()));
    for (const file of pictures) {
      let name = nameFromFile(file.name);
      for (let n = 2; taken.has(name.toLowerCase()); n++) name = `${nameFromFile(file.name).slice(0, 29)}_${n}`;
      taken.add(name.toLowerCase());
      const key = `${file.name}-${Math.random()}`;
      setPending((list) => [...list, { key, preview: URL.createObjectURL(file), name, sent: 0 }]);
      void send(key, file, name);
    }
  }

  async function send(key: string, file: File, name: string) {
    const update = (patch: Partial<Pending>) => setPending((list) => list.map((p) => (p.key === key ? { ...p, ...patch } : p)));
    try {
      const picture = await emojiPicture(file);
      const url = await run(uploadPicture(instanceKey, MediaPurpose.EMOJI, picture, (sent) => update({ sent: sent * 0.9 }), serverId));
      await run(createEmoji(instanceKey, serverId, name, url));
      update({ sent: 1 });
      setTimeout(() => setPending((list) => list.filter((p) => p.key !== key)), 500);
    } catch (err) {
      update({ error: (err as FuwaError).message || "that didn't go up" });
    }
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
      void add([...e.dataTransfer.files]);
    },
  };

  return (
    <div className="flex flex-col gap-5">
      <motion.button
        type="button"
        animate={shake}
        whileHover={{ y: -2 }}
        whileTap={{ scale: 0.98 }}
        disabled={full}
        onClick={() => input.current?.click()}
        {...drop}
        className={cn(
          "group relative flex flex-col items-center gap-2 overflow-hidden rounded-3xl border-2 border-dashed p-6 text-center transition-colors",
          dragging ? "border-primary bg-primary/10" : "border-border hover:border-primary/50 hover:bg-primary/5",
          full && "cursor-not-allowed opacity-60",
        )}
      >
        <motion.span
          animate={dragging ? { scale: 1.2, rotate: -12, y: -4 } : { scale: 1, rotate: 0, y: 0 }}
          transition={{ type: "spring", stiffness: 500, damping: 14 }}
          className="grid size-12 place-items-center rounded-2xl bg-primary/15 text-primary"
        >
          <SmilePlusIcon className="size-6 transition group-hover:rotate-12" />
        </motion.span>
        <span className="font-extrabold">{dragging ? "Let go to add them" : "Drop pictures here, or pick some"}</span>
        <span className="max-w-sm text-xs text-muted-foreground">
          Several at once is fine. Each is shrunk to 128 pixels; GIFs keep moving. The file name becomes the emoji's name, and you can change it.
        </span>
        <span className="mt-1 flex items-center gap-1 rounded-full bg-muted px-2.5 py-0.5 text-xs font-bold tabular-nums">
          <Count value={count} /> {cap !== null ? `of ${cap}` : "emoji"}
        </span>
      </motion.button>
      <input
        ref={input}
        type="file"
        multiple
        accept={PICTURE_TYPES.join(",")}
        className="hidden"
        onChange={(e) => {
          void add([...(e.target.files ?? [])]);
          e.target.value = "";
        }}
      />

      {!emojis ? (
        <div className="grid gap-2 sm:grid-cols-2">
          {[0, 1, 2, 3].map((n) => (
            <div key={n} className="shimmer h-16 rounded-2xl" />
          ))}
        </div>
      ) : count === 0 && pending.length === 0 ? (
        <motion.div initial={{ opacity: 0, y: 8 }} animate={{ opacity: 1, y: 0 }} className="flex flex-col items-center gap-2 py-8 text-center">
          <motion.span
            animate={{ rotate: [0, -10, 10, -6, 0], y: [0, -4, 0] }}
            transition={{ duration: 2.4, repeat: Infinity, repeatDelay: 1.2 }}
            className="text-4xl"
          >
            🫥
          </motion.span>
          <p className="font-bold">No emoji yet</p>
          <p className="text-sm text-muted-foreground">Add the first and it shows up when people type a colon.</p>
        </motion.div>
      ) : (
        <ul className="grid gap-2 sm:grid-cols-2">
          <AnimatePresence initial={false}>
            {pending.map((p) => (
              <motion.li
                key={p.key}
                layout
                initial={{ opacity: 0, scale: 0.9 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.9 }}
                transition={SPRING}
                className={cn("relative flex items-center gap-3 overflow-hidden rounded-2xl border p-3", p.error && "border-destructive/50 bg-destructive/5")}
              >
                <motion.span
                  className="absolute inset-y-0 left-0 bg-primary/10"
                  initial={{ width: 0 }}
                  animate={{ width: `${p.sent * 100}%` }}
                  transition={{ ease: "easeOut" }}
                />
                <img src={p.preview} alt="" className="relative size-10 object-contain" />
                <span className="relative min-w-0 flex-1">
                  <span className="block truncate font-bold">:{p.name}:</span>
                  <span className={cn("block truncate text-xs", p.error ? "text-destructive first-letter:uppercase" : "text-muted-foreground")}>
                    {p.error ?? (p.sent >= 1 ? "Added" : "Uploading…")}
                  </span>
                </span>
                {p.error ? (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    aria-label="Dismiss"
                    className="relative rounded-full"
                    onClick={() => setPending((list) => list.filter((x) => x.key !== p.key))}
                  >
                    <XIcon />
                  </Button>
                ) : p.sent >= 1 ? (
                  <motion.span initial={{ scale: 0 }} animate={{ scale: 1 }} transition={{ type: "spring", stiffness: 700, damping: 15 }} className="relative text-emerald-500">
                    <CheckIcon className="size-5" strokeWidth={3} />
                  </motion.span>
                ) : (
                  <LoaderCircleIcon className="relative size-5 animate-spin text-muted-foreground" />
                )}
              </motion.li>
            ))}
            {[...emojis]
              .sort((a, b) => a.name.localeCompare(b.name))
              .map((emoji) => (
                <EmojiRow
                  key={emoji.id}
                  instanceKey={instanceKey}
                  serverId={serverId}
                  emoji={emoji}
                  creator={members?.find((m) => m.user?.id === emoji.creatorId)?.user}
                />
              ))}
          </AnimatePresence>
        </ul>
      )}
      {cap !== null && full && (
        <p className="flex items-center gap-1.5 text-xs text-amber-600 dark:text-amber-400">
          <ImagePlusIcon className="size-3.5" /> This server has all the emoji it can hold. Delete one to make room.
        </p>
      )}
    </div>
  );
}

/** One emoji: its picture, a name you can change in place, who added it, and delete. */
function EmojiRow({ instanceKey, serverId, emoji, creator }: { instanceKey: string; serverId: string; emoji: EmojiT; creator: Parameters<typeof UserAvatar>[0]["user"] }) {
  const lang = useI18n();
  const [name, setName] = useState(emoji.name);
  const [saving, setSaving] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const shake = useAnimationControls();
  useEffect(() => setName(emoji.name), [emoji.name]);
  const valid = EMOJI_NAME.test(name);

  async function rename() {
    if (name === emoji.name) return;
    if (!valid) {
      void shake.start({ x: [0, -6, 5, -3, 0], transition: { duration: 0.35 } });
      toast("Emoji names are 2 to 32 letters, digits and underscores.");
      return setName(emoji.name);
    }
    setSaving(true);
    await run(renameEmoji(instanceKey, serverId, emoji.id, name)).catch((err: FuwaError) => {
      toast(err.message);
      setName(emoji.name);
    });
    setSaving(false);
  }

  async function remove() {
    await run(deleteEmoji(instanceKey, serverId, emoji.id)).catch((err: FuwaError) => toast(err.message));
  }

  return (
    <motion.li
      layout
      initial={{ opacity: 0, scale: 0.9 }}
      animate={{ opacity: 1, scale: 1 }}
      exit={{ opacity: 0, scale: 0.8, filter: "blur(4px)" }}
      transition={SPRING}
      className="group flex items-center gap-3 rounded-2xl border bg-background/50 p-3 transition-colors hover:border-primary/30"
    >
      <motion.img
        src={emoji.url}
        alt={`:${emoji.name}:`}
        whileHover={{ scale: 1.25, rotate: -8 }}
        transition={{ type: "spring", stiffness: 600, damping: 12 }}
        className="size-10 shrink-0 object-contain"
      />
      <span className="min-w-0 flex-1">
        <motion.span animate={shake} className="flex items-center rounded-lg font-bold">
          <span className="text-muted-foreground">:</span>
          <Input
            value={name}
            aria-label={`Name of :${emoji.name}:`}
            maxLength={32}
            spellCheck={false}
            onChange={(e) => setName(e.target.value.replace(/\s+/g, "_"))}
            onBlur={() => void rename()}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") setName(emoji.name);
            }}
            className={cn(
              "h-7 w-auto max-w-full min-w-[3ch] rounded-md border-transparent bg-transparent px-0.5 font-bold shadow-none [field-sizing:content] focus-visible:border-input",
              !valid && "text-destructive",
            )}
          />
          <span className="text-muted-foreground">:</span>
          {saving && <LoaderCircleIcon className="ml-1 size-3.5 shrink-0 animate-spin text-muted-foreground" />}
        </motion.span>
        <span className="mt-0.5 flex items-center gap-1.5 truncate text-xs text-muted-foreground">
          <UserAvatar user={creator} className="size-4" />
          <span className="truncate">
            {creator ? displayName(creator) : "Someone"} · {formatBytes(lang, Number(emoji.size))}
            {emoji.animated && " · moves"}
          </span>
        </span>
      </span>
      <AnimatePresence mode="popLayout" initial={false}>
        {confirm ? (
          <motion.span key="sure" initial={{ opacity: 0, x: 8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: 8 }} transition={SPRING} className="flex gap-1">
            <Button type="button" size="sm" variant="destructive" className="h-8 rounded-full px-3 text-xs font-bold" onClick={() => void remove()}>
              Delete
            </Button>
            <Button type="button" size="icon" variant="ghost" aria-label="Keep it" className="size-8 rounded-full" onClick={() => setConfirm(false)}>
              <XIcon />
            </Button>
          </motion.span>
        ) : (
          <motion.span key="ask" initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} exit={{ opacity: 0, x: -8 }} transition={SPRING}>
            <Button
              type="button"
              size="icon"
              variant="ghost"
              aria-label={`Delete :${emoji.name}:`}
              className="size-8 rounded-full text-muted-foreground opacity-60 transition hover:text-destructive group-hover:opacity-100"
              onClick={() => setConfirm(true)}
            >
              <Trash2Icon />
            </Button>
          </motion.span>
        )}
      </AnimatePresence>
    </motion.li>
  );
}
