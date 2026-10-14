import { ArchiveIcon, ArchiveRestoreIcon, CalendarIcon, CheckIcon, ColumnsIcon, EyeOffIcon, PencilIcon, PlusIcon, TagIcon, Trash2Icon, UsersIcon, XIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Permission, type Attachment, type BoardColumn, type BoardLabel, type Card, type Channel, type Emoji, type Message } from "@/gen/fuwa/v1/types_pb";
import { deleteMessage, focusThread, run } from "@/fuwa/actions";
import { moveCard, updateCard, type CardEdit } from "@/fuwa/board";
import { useAccess } from "@/fuwa/hooks";
import { useFuwa } from "@/fuwa/store";
import { Attachments, FileBadge } from "@/components/chat/Attachments";
import { AttachButton, StagedTray } from "@/components/chat/ComposerFiles";
import { Composer } from "@/components/chat/Composer";
import { MessageBody, MessageList, type MessageListHandle } from "@/components/chat/MessageList";
import { addFiles, takeFiles, useStaged } from "@/components/chat/staged";
import { UserAvatar } from "@/components/Icons";
import { Count, SwapText } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuRadioGroup, DropdownMenuRadioItem, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { T, useI18n } from "@/i18n/react";
import { colorCss, ms, TITLE_MAX, ASSIGNEES_MAX } from "@/lib/board";
import { displayName } from "@/lib/format";
import { SPRING } from "@/lib/motion";
import { filesIn } from "@/lib/pasted";
import { hasIn } from "@/lib/permissions";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";
import { LabelChip, DueDate } from "./BoardCard";

const NO_MEMBERS: never[] = [];
const MAX_BODY = 4000;

/** `yyyy-mm-dd` for a date input, in this device's time. */
const dayOf = (at: number) => {
  if (!at) return "";
  const d = new Date(at);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
};
/** A date input's day as a time: noon there, so it reads as the same day anywhere near. */
const dayAt = (day: string) => {
  const [y, m, d] = day.split("-").map(Number);
  return y && m && d ? new Date(y, m - 1, d, 12).getTime() : 0;
};

/**
 * A card opened beside its board: its title, column, people, labels and due
 * date, its body (Markdown and files, like any message) and the comments
 * under it, which are the body's thread.
 */
export function CardPanel({
  instanceKey,
  serverId,
  channel,
  card,
  body,
  columns,
  labels,
  onChanged,
  onClose,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  card: Card;
  body: Message | undefined;
  columns: BoardColumn[];
  labels: BoardLabel[];
  /** The card as the server has it after a change from here. */
  onChanged?: (card: Card) => void;
  onClose: () => void;
}) {
  const { t } = useI18n();
  const access = useAccess(instanceKey, serverId);
  const me = useFuwa((s) => s.instances[instanceKey]?.me?.id ?? "");
  const canManage = hasIn(access, channel.id, Permission.MANAGE_CARDS);
  const canDelete = card.authorId === me || hasIn(access, channel.id, Permission.MANAGE_MESSAGES);
  const archived = !!ms(card.archivedAt);
  const list = useRef<MessageListHandle>(null);
  const column = columns.find((c) => c.id === card.columnId);

  useEffect(() => {
    focusThread(instanceKey, card.id);
    return () => focusThread(instanceKey, null);
  }, [instanceKey, card.id]);

  const change = (edit: CardEdit) =>
    updateCard(instanceKey, serverId, channel.id, card, edit)
      .then((c) => c && onChanged?.(c))
      .catch((err: Error) => toast(err.message));
  const [confirming, setConfirming] = useState(false);
  const remove = () =>
    run(deleteMessage(instanceKey, serverId, channel.id, card.id))
      .then(onClose)
      .catch((err: Error) => toast(err.message));

  return (
    <div className="flex h-full min-h-0 flex-col">
      <header className="flex h-14 shrink-0 items-center gap-2 border-b px-3">
        <span className="size-2.5 shrink-0 rounded-full bg-muted-foreground" style={{ background: colorCss(column?.color ?? 0) }} />
        <p className="min-w-0 flex-1 truncate text-sm font-bold text-muted-foreground">
          <SwapText>{archived ? t("chattools.board.archivedIn", { column: column?.name ?? "" }) : (column?.name ?? "")}</SwapText>
        </p>
        {canManage && (
          <PanelButton label={archived ? t("chattools.board.restore") : t("chattools.board.archive")} onClick={() => void change({ archived: !archived })}>
            {archived ? <ArchiveRestoreIcon /> : <ArchiveIcon />}
          </PanelButton>
        )}
        {canDelete && (
          <PanelButton label={t("chattools.board.delete")} onClick={() => setConfirming(true)} danger>
            <Trash2Icon />
          </PanelButton>
        )}
        <PanelButton label={t("chattools.board.close")} onClick={onClose}>
          <XIcon />
        </PanelButton>
      </header>
      <AnimatePresence initial={false}>
        {confirming && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: "auto", opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={SPRING}
            className="shrink-0 overflow-hidden border-b bg-destructive/10"
          >
            <div className="flex items-center gap-2 px-3 py-2 text-sm">
              <p className="flex-1 font-bold">{t("chattools.board.deleteConfirm")}</p>
              <Button size="sm" variant="ghost" className="rounded-xl" onClick={() => setConfirming(false)}>
                {t("chattools.board.cancel")}
              </Button>
              <Button size="sm" variant="destructive" className="rounded-xl" onClick={() => void remove()}>
                {t("chattools.board.delete")}
              </Button>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
      <div className="scroll-thin max-h-[55%] shrink-0 overflow-y-auto border-b px-4 py-3">
        <Title card={card} editable={canManage} onSave={(title) => change({ title })} />
        <div className="mt-3 grid grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-2 text-sm">
          <Field icon={ColumnsIcon} label={t("chattools.board.column")}>
            <ColumnPicker columns={columns} value={card.columnId} disabled={!canManage || archived} onPick={(columnId) => void moveCard(instanceKey, serverId, channel.id, card.id, columnId, "").catch((err: Error) => toast(err.message))} />
          </Field>
          <Field icon={UsersIcon} label={t("chattools.board.people")}>
            <PeoplePicker instanceKey={instanceKey} serverId={serverId} value={card.assigneeIds} disabled={!canManage} onPick={(assigneeIds) => change({ assigneeIds })} />
          </Field>
          <Field icon={TagIcon} label={t("chattools.board.labels")}>
            <LabelPicker labels={labels} value={card.labelIds} disabled={!canManage} onPick={(labelIds) => change({ labelIds })} />
          </Field>
          <Field icon={CalendarIcon} label={t("chattools.board.due")}>
            <span className="flex items-center gap-2">
              {canManage ? (
                <input
                  type="date"
                  value={dayOf(ms(card.dueAt))}
                  onChange={(e) => void change({ dueAt: e.target.value ? dayAt(e.target.value) : null })}
                  aria-label={t("chattools.board.due")}
                  className="rounded-lg border bg-card px-2 py-1 text-sm"
                />
              ) : (
                <DueDate card={card} column={column} now={Date.now()} />
              )}
              {canManage && !!card.dueAt && (
                <button type="button" onClick={() => void change({ dueAt: null })} className="text-xs font-bold text-muted-foreground hover:text-destructive">
                  {t("chattools.board.clearDue")}
                </button>
              )}
            </span>
          </Field>
        </div>
        <Body instanceKey={instanceKey} serverId={serverId} channel={channel} card={card} body={body} editable={canManage} onSave={(b) => change({ body: b })} />
      </div>
      <div className="flex shrink-0 items-center gap-2 px-4 pt-2 text-xs font-bold text-muted-foreground">
        <T k="chattools.board.commentsHeading" values={{ count: <Count value={body?.thread?.replyCount ?? 0} /> }} count={body?.thread?.replyCount ?? 0} />
        <span className="h-px flex-1 bg-border" />
      </div>
      <MessageList key={card.id} ref={list} instanceKey={instanceKey} serverId={serverId} channel={channel} threadId={card.id} header={<p className="px-4 py-3 text-xs text-muted-foreground">{t("chattools.board.commentsStart")}</p>} />
      {!archived && (
        <Composer
          instanceKey={instanceKey}
          serverId={serverId}
          channel={channel}
          thread={{ id: card.id, locked: !!body?.thread?.locked && !hasIn(access, channel.id, Permission.MANAGE_MESSAGES), started: true }}
          placeholder={t("chattools.board.commentPlaceholder")}
          onEditLast={() => list.current?.editLast()}
        />
      )}
    </div>
  );

}

function PanelButton({ label, onClick, children, danger }: { label: string; onClick: () => void; children: ReactNode; danger?: boolean }) {
  return (
    <motion.button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      whileTap={{ scale: 0.85 }}
      className={cn("grid size-9 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted [&_svg]:size-[18px]", danger && "hover:text-destructive")}
    >
      {children}
    </motion.button>
  );
}

function Field({ icon: Icon, label, children }: { icon: typeof TagIcon; label: string; children: ReactNode }) {
  return (
    <>
      <span className="flex items-center gap-1.5 text-xs font-bold text-muted-foreground">
        <Icon className="size-3.5" /> {label}
      </span>
      <div className="min-w-0">{children}</div>
    </>
  );
}

/** The card's title, edited in place. */
function Title({ card, editable, onSave }: { card: Card; editable: boolean; onSave: (title: string) => void }) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<string | null>(null);
  const save = () => {
    const title = draft?.trim() ?? "";
    setDraft(null);
    if (title && title !== card.title) onSave(title);
  };
  if (draft !== null)
    return (
      <textarea
        autoFocus
        value={draft}
        maxLength={TITLE_MAX}
        rows={2}
        aria-label={t("chattools.board.title")}
        onChange={(e) => setDraft(e.target.value.replace(/\n/g, " "))}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            save();
          }
          if (e.key === "Escape") setDraft(null);
        }}
        className="w-full resize-none rounded-xl border bg-card px-2 py-1 text-lg leading-snug font-extrabold outline-none focus:border-primary/60"
      />
    );
  return (
    <h2 className="group flex items-start gap-2 text-lg leading-snug font-extrabold break-words">
      <SwapText className="min-w-0 flex-1">{card.title}</SwapText>
      {editable && (
        <button
          type="button"
          aria-label={t("chattools.board.rename")}
          onClick={() => setDraft(card.title)}
          className="mt-1 shrink-0 rounded-md p-1 text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:bg-muted focus-visible:opacity-100"
        >
          <PencilIcon className="size-3.5" />
        </button>
      )}
    </h2>
  );
}

function ColumnPicker({ columns, value, disabled, onPick }: { columns: BoardColumn[]; value: string; disabled: boolean; onPick: (id: string) => void }) {
  const current = columns.find((c) => c.id === value);
  const chip = (
    <span className="flex items-center gap-1.5 font-bold">
      <span className="size-2 rounded-full bg-muted-foreground" style={{ background: colorCss(current?.color ?? 0) }} />
      {current?.name ?? "?"}
    </span>
  );
  if (disabled) return chip;
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className="rounded-lg px-2 py-1 transition-colors hover:bg-muted">
          {chip}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-48">
        <DropdownMenuRadioGroup value={value} onValueChange={(id) => id !== value && onPick(id)}>
          {columns.map((c) => (
            <DropdownMenuRadioItem key={c.id} value={c.id}>
              <span className="size-2 rounded-full bg-muted-foreground" style={{ background: colorCss(c.color) }} />
              {c.name}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

function PeoplePicker({ instanceKey, serverId, value, disabled, onPick }: { instanceKey: string; serverId: string; value: string[]; disabled: boolean; onPick: (ids: string[]) => void }) {
  const { t } = useI18n();
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  const members = useFuwa((s) => s.instances[instanceKey]?.members[serverId] ?? NO_MEMBERS);
  const people = value.map((id) => users?.[id]);
  const chips = (
    <span className="flex flex-wrap items-center gap-1.5">
      <AnimatePresence initial={false} mode="popLayout">
        {people.map((u, i) => (
          <motion.span
            key={value[i]}
            layout
            initial={{ opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.6 }}
            transition={SPRING}
            className="flex items-center gap-1 rounded-full bg-muted py-0.5 pr-2 pl-0.5 text-xs font-bold"
          >
            <UserAvatar user={u} className="size-5 text-[0.55rem]" />
            {displayName(u)}
          </motion.span>
        ))}
      </AnimatePresence>
      {!people.length && <span className="text-muted-foreground">{t("chattools.board.nobody")}</span>}
    </span>
  );
  if (disabled) return chips;
  const sorted = [...members].filter((m) => m.user).sort((a, b) => displayName(a.user).localeCompare(displayName(b.user)));
  return (
    <span className="flex flex-wrap items-center gap-1.5">
      {chips}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label={t("chattools.board.addPeople")} className="grid size-6 place-items-center rounded-full border border-dashed text-muted-foreground transition-colors hover:border-primary hover:text-primary">
            <PlusIcon className="size-3.5" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="scroll-thin max-h-72 w-60 overflow-y-auto">
          {sorted.map((m) => {
            const id = m.user!.id;
            const on = value.includes(id);
            return (
              <DropdownMenuCheckboxItem
                key={id}
                checked={on}
                disabled={!on && value.length >= ASSIGNEES_MAX}
                onSelect={(e) => e.preventDefault()}
                onCheckedChange={(checked) => onPick(checked ? [...value, id] : value.filter((x) => x !== id))}
              >
                <UserAvatar user={m.user} className="size-5 text-[0.55rem]" />
                <span className="truncate">{m.nickname || displayName(m.user)}</span>
              </DropdownMenuCheckboxItem>
            );
          })}
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  );
}

function LabelPicker({ labels, value, disabled, onPick }: { labels: BoardLabel[]; value: string[]; disabled: boolean; onPick: (ids: string[]) => void }) {
  const { t } = useI18n();
  const shown = labels.filter((l) => value.includes(l.id));
  const chips = (
    <span className="flex flex-wrap items-center gap-1">
      {shown.map((l) => (
        <LabelChip key={l.id} label={l} />
      ))}
      {!shown.length && <span className="text-muted-foreground">{labels.length ? t("chattools.board.noLabels") : t("chattools.board.noBoardLabels")}</span>}
    </span>
  );
  if (disabled || !labels.length) return chips;
  return (
    <span className="flex flex-wrap items-center gap-1.5">
      {chips}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" aria-label={t("chattools.board.addLabels")} className="grid size-6 place-items-center rounded-full border border-dashed text-muted-foreground transition-colors hover:border-primary hover:text-primary">
            <PlusIcon className="size-3.5" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-52">
          {labels.map((l) => (
            <DropdownMenuCheckboxItem
              key={l.id}
              checked={value.includes(l.id)}
              onSelect={(e) => e.preventDefault()}
              onCheckedChange={(checked) => onPick(checked ? [...value, l.id] : value.filter((x) => x !== l.id))}
            >
              <LabelChip label={l} />
            </DropdownMenuCheckboxItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </span>
  );
}

/** The card's body: shown like a message, or edited with its files. */
function Body({
  instanceKey,
  serverId,
  channel,
  card,
  body,
  editable,
  onSave,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  card: Card;
  body: Message | undefined;
  editable: boolean;
  onSave: (body: { content: string; emojis: Emoji[]; attachments: Attachment[] }) => Promise<unknown>;
}) {
  const { t } = useI18n();
  const [editing, setEditing] = useState(false);
  const empty = !body?.content && !body?.attachments.length;
  return (
    <section className="mt-4">
      <div className="mb-1.5 flex items-center gap-2">
        <h3 className="text-xs font-bold tracking-wide text-muted-foreground uppercase">{t("chattools.board.description")}</h3>
        {editable && !editing && (
          <button type="button" onClick={() => setEditing(true)} className="flex items-center gap-1 text-xs font-bold text-primary hover:underline">
            <PencilIcon className="size-3" /> {empty ? t("chattools.board.addDescription") : t("chattools.board.edit")}
          </button>
        )}
      </div>
      <AnimatePresence mode="wait" initial={false}>
        {editing ? (
          <motion.div key="edit" initial={{ opacity: 0, y: 6 }} animate={{ opacity: 1, y: 0 }} exit={{ opacity: 0, y: -6 }} transition={SPRING}>
            <BodyEditor instanceKey={instanceKey} serverId={serverId} channel={channel} card={card} body={body} onDone={() => setEditing(false)} onSave={onSave} />
          </motion.div>
        ) : (
          <motion.div key="view" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }}>
            {empty ? (
              <p className="text-sm text-muted-foreground">{t("chattools.board.noDescription")}</p>
            ) : (
              <div className="text-sm">
                {body?.content && <MessageBody content={body.content} emojis={body.emojis} display="cozy" />}
                {body?.editedAt && <span className="text-[0.65rem] text-muted-foreground">{t("chattools.board.edited")}</span>}
                <Attachments files={body?.attachments ?? []} animate={false} />
              </div>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </section>
  );
}

function BodyEditor({
  instanceKey,
  serverId,
  channel,
  card,
  body,
  onDone,
  onSave,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  card: Card;
  body: Message | undefined;
  onDone: () => void;
  onSave: (body: { content: string; emojis: Emoji[]; attachments: Attachment[] }) => Promise<unknown>;
}) {
  const { t } = useI18n();
  const access = useAccess(instanceKey, serverId);
  const [text, setText] = useState(body?.content ?? "");
  const [kept, setKept] = useState<Attachment[]>(body?.attachments ?? []);
  const [saving, setSaving] = useState(false);
  const where = useMemo(() => ({ instanceKey, serverId, channelId: `card:${card.id}` }), [instanceKey, serverId, card.id]);
  const staged = useStaged(where.channelId);
  const uploading = staged.some((s) => !s.done);
  const canAttach = hasIn(access, channel.id, Permission.ATTACH_FILES);
  async function save() {
    setSaving(true);
    try {
      await onSave({ content: text, emojis: body?.emojis ?? [], attachments: [...kept, ...takeFiles(where.channelId)] });
      onDone();
    } finally {
      setSaving(false);
    }
  }
  return (
    <div className="flex flex-col gap-2">
      <textarea
        autoFocus
        value={text}
        maxLength={MAX_BODY}
        rows={6}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Escape") onDone();
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) void save();
        }}
        // Pasted or dropped pictures and files join the description's, as the paperclip's do.
        onPaste={(e) => {
          const pasted = filesIn(e.clipboardData);
          if (!pasted.length || !canAttach) return;
          e.preventDefault();
          addFiles(where.instanceKey, where.serverId, where.channelId, pasted);
        }}
        onDragOver={(e) => {
          if (!canAttach || !e.dataTransfer.types.includes("Files")) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "copy";
        }}
        onDrop={(e) => {
          const dropped = filesIn(e.dataTransfer);
          if (!dropped.length || !canAttach) return;
          // Taken here, so the comments' drop sheet leaves it be.
          e.preventDefault();
          addFiles(where.instanceKey, where.serverId, where.channelId, dropped);
        }}
        aria-label={t("chattools.board.description")}
        placeholder={t("chattools.board.descriptionPlaceholder")}
        className="composer w-full resize-y rounded-xl border bg-card px-3 py-2 text-sm leading-6 outline-none focus:border-primary/60"
      />
      {kept.length > 0 && (
        <ul className="flex flex-wrap gap-1.5">
          <AnimatePresence initial={false} mode="popLayout">
            {kept.map((file) => (
              <motion.li
                key={file.id || file.url}
                layout
                initial={{ opacity: 0, scale: 0.8 }}
                animate={{ opacity: 1, scale: 1 }}
                exit={{ opacity: 0, scale: 0.8 }}
                transition={SPRING}
                className="flex items-center gap-1.5 rounded-lg border bg-card py-1 pr-1 pl-2 text-xs"
              >
                <FileBadge name={file.filename} className="size-5" />
                <span className="max-w-40 truncate font-bold">{file.filename}</span>
                {file.spoiler && <EyeOffIcon aria-label={t("chat.files.spoilerBadge")} className="size-3 shrink-0 text-muted-foreground" />}
                <button
                  type="button"
                  aria-label={t("chattools.board.removeFile", { name: file.filename })}
                  onClick={() => setKept((list) => list.filter((f) => f !== file))}
                  className="rounded p-0.5 text-muted-foreground hover:bg-muted hover:text-destructive"
                >
                  <XIcon className="size-3" />
                </button>
              </motion.li>
            ))}
          </AnimatePresence>
        </ul>
      )}
      <AnimatePresence>{staged.length > 0 && <StagedTray where={where} staged={staged} />}</AnimatePresence>
      <div className="flex items-center gap-2">
        {canAttach && <AttachButton where={where} />}
        <p className="flex-1 text-[0.7rem] text-muted-foreground">{t("chattools.board.markdownHint")}</p>
        <Button size="sm" variant="ghost" className="rounded-xl" onClick={onDone}>
          {t("chattools.board.cancel")}
        </Button>
        <Button size="sm" className="btn rounded-xl font-bold" disabled={saving || uploading} onClick={() => void save()}>
          <CheckIcon /> {t("chattools.board.save")}
        </Button>
      </div>
    </div>
  );
}
