import {
  autoUpdate,
  flip,
  FloatingFocusManager,
  FloatingPortal,
  offset,
  shift,
  useClick,
  useDismiss,
  useFloating,
  useInteractions,
  useRole,
} from "@floating-ui/react";
import { CornerDownRightIcon, LockKeyholeIcon, PinIcon, PinOffIcon, RotateCwIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useState, type ReactNode } from "react";
import type { DmPin } from "@/gen/fuwa/v1/dm_pb";
import type { Message, User } from "@/gen/fuwa/v1/types_pb";
import { isMessage, type Item } from "@/e2ee/vault";
import type { FuwaError } from "@/fuwa/errors";
import { loadChannelPins, loadDmPins, pinDm, pinMessage, useChannelPins, useDmPins } from "@/fuwa/pins";
import { useFuwa } from "@/fuwa/store";
import { MessageBody } from "@/components/chat/MessageList";
import { UserAvatar } from "@/components/Icons";
import { SPRING } from "@/lib/motion";
import { useI18n } from "@/i18n/react";
import { displayName, formatFull, formatStamp, toDate } from "@/lib/format";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

/*
 * Pinned messages: the pin in a channel's, thread's or conversation's header
 * opens a list of what's pinned there, the latest pin first, to read, jump to
 * or (for people who may) unpin. Messages carry a small pin while pinned.
 */

const NO_PINS: DmPin[] = [];
const NO_ITEMS: Item[] = [];

/** The small pin beside a pinned message's text. */
export function PinMark() {
  const { t } = useI18n();
  return (
    <motion.span
      initial={{ scale: 0, rotate: -40 }}
      animate={{ scale: 1, rotate: 0 }}
      transition={{ type: "spring", stiffness: 600, damping: 18 }}
      title={t("chattools.pins.pinned")}
      className="ml-1.5 inline-flex translate-y-[-1px] items-center gap-1 rounded-full bg-primary/10 px-1.5 py-px align-middle text-[0.65rem] font-bold text-primary"
    >
      <PinIcon className="size-3 -rotate-45" />
      <span className="sr-only">{t("chattools.pins.pinned")}</span>
    </motion.span>
  );
}

/** The header button and the list it opens beside it. */
function PinsPopover({ count, onOpen, children }: { count: number; onOpen?: () => void; children: (close: () => void) => ReactNode }) {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  const { refs, floatingStyles, context } = useFloating({
    open,
    onOpenChange: (next) => {
      // Read when opened, so a channel costs nothing until someone looks.
      if (next) onOpen?.();
      setOpen(next);
    },
    placement: "bottom-end",
    whileElementsMounted: autoUpdate,
    middleware: [offset(6), flip(), shift({ padding: 8 })],
  });
  const { getReferenceProps, getFloatingProps } = useInteractions([useClick(context), useDismiss(context), useRole(context, { role: "dialog" })]);
  return (
    <>
      <motion.button
        ref={refs.setReference}
        {...getReferenceProps()}
        type="button"
        aria-label={t("chattools.pins.button")}
        title={t("chattools.pins.button")}
        aria-expanded={open}
        whileTap={{ scale: 0.85 }}
        className={cn(
          "relative grid size-9 shrink-0 place-items-center rounded-full transition-colors hover:bg-muted",
          open ? "bg-primary/10 text-primary" : "text-muted-foreground",
        )}
      >
        <motion.span initial={false} animate={{ rotate: open ? 0 : -45, scale: open ? 1.08 : 1 }} transition={SPRING}>
          <PinIcon className="size-5" />
        </motion.span>
      </motion.button>
      <FloatingPortal>
        <AnimatePresence>
          {open && (
            <FloatingFocusManager context={context} initialFocus={-1} modal={false}>
              <div ref={refs.setFloating} style={floatingStyles} className="z-50" {...getFloatingProps()}>
                <motion.div
                  initial={{ opacity: 0, scale: 0.94, y: -8 }}
                  animate={{ opacity: 1, scale: 1, y: 0 }}
                  exit={{ opacity: 0, scale: 0.96, y: -6 }}
                  transition={SPRING}
                  style={{ transformOrigin: "top right" }}
                  className="flex max-h-[min(32rem,75vh)] w-[26rem] max-w-[calc(100vw-1rem)] flex-col overflow-hidden rounded-2xl border bg-popover shadow-2xl"
                >
                  <header className="flex items-center gap-2 border-b px-4 py-3">
                    <PinIcon className="size-4 -rotate-45 text-primary" />
                    <h2 className="font-extrabold">{t("chattools.pins.title")}</h2>
                    {count > 0 && <span className="text-xs font-bold text-muted-foreground">{t("chattools.pins.count", { count })}</span>}
                  </header>
                  <div className="scroll-thin min-h-0 flex-1 overflow-y-auto p-2">{children(() => setOpen(false))}</div>
                </motion.div>
              </div>
            </FloatingFocusManager>
          )}
        </AnimatePresence>
      </FloatingPortal>
    </>
  );
}

function Empty({ hint }: { hint: string }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col items-center px-6 py-8 text-center">
      <motion.span
        initial={{ y: -14, rotate: -70, opacity: 0 }}
        animate={{ y: 0, rotate: -45, opacity: 1 }}
        transition={{ type: "spring", stiffness: 420, damping: 14 }}
        className="grid size-12 place-items-center rounded-2xl bg-primary/10 text-primary"
      >
        <PinIcon className="size-6" />
      </motion.span>
      <p className="mt-3 font-extrabold">{t("chattools.pins.empty")}</p>
      <p className="mt-1 max-w-xs text-sm text-muted-foreground">{hint}</p>
    </div>
  );
}

function Failed({ onRetry }: { onRetry: () => void }) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col items-center gap-2 px-6 py-6 text-center text-sm text-muted-foreground">
      {t("chattools.pins.failed")}
      <button type="button" onClick={onRetry} className="flex items-center gap-1.5 rounded-full bg-muted px-3 py-1 text-xs font-bold text-foreground transition hover:bg-muted/70">
        <RotateCwIcon className="size-3.5" />
        {t("chattools.pins.retry")}
      </button>
    </div>
  );
}

function Loading() {
  return (
    <div className="flex flex-col gap-3 p-2">
      {[0, 1, 2].map((n) => (
        <div key={n} className="flex gap-3" style={{ opacity: 1 - n * 0.25 }}>
          <div className="shimmer size-8 shrink-0 rounded-full" />
          <div className="flex flex-1 flex-col gap-2 pt-1">
            <div className="shimmer h-3 w-28 rounded" />
            <div className="shimmer h-3 w-3/4 rounded" />
          </div>
        </div>
      ))}
    </div>
  );
}

/** One pinned message in the list: who wrote it, when it was pinned, what it says, and its buttons. Not memoized: its body is a fresh element on every draw. */
function PinRow({
  author,
  at,
  body,
  onJump,
  onUnpin,
  index,
}: {
  author: User | undefined;
  /** When it was sent. */
  at: Date;
  body: ReactNode;
  onJump?: () => void;
  onUnpin?: () => Promise<void>;
  index: number;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, x: 24, transition: { duration: 0.18 } }}
      transition={{ ...SPRING, delay: Math.min(index, 8) * 0.03 }}
      className="group/pin relative flex gap-3 rounded-xl px-2 py-2 transition-colors hover:bg-muted/60"
    >
      <UserAvatar user={author} className="size-8 shrink-0 text-xs" />
      <div className="min-w-0 flex-1">
        <div className="flex items-baseline gap-2">
          <span className="truncate text-sm font-bold">{displayName(author)}</span>
          <time className="shrink-0 text-xs text-muted-foreground" dateTime={at.toISOString()} title={formatFull(at)}>
            {formatStamp(at)}
          </time>
        </div>
        <div className="line-clamp-4 text-sm break-words">{body}</div>
      </div>
      <div className="absolute top-1.5 right-1.5 flex items-center gap-1 opacity-0 transition-opacity group-focus-within/pin:opacity-100 group-hover/pin:opacity-100 max-sm:opacity-100">
        {onJump && (
          <button
            type="button"
            onClick={onJump}
            className="flex items-center gap-1 rounded-full border bg-card px-2 py-0.5 text-xs font-bold shadow-sm transition hover:border-primary hover:text-primary"
          >
            <CornerDownRightIcon className="size-3" />
            {t("chattools.pins.jump")}
          </button>
        )}
        {onUnpin && (
          <button
            type="button"
            aria-label={t("chattools.pins.unpin")}
            title={t("chattools.pins.unpin")}
            disabled={busy}
            onClick={() => {
              setBusy(true);
              onUnpin().catch((err: FuwaError) => {
                toast(err.message);
                setBusy(false);
              });
            }}
            className="grid size-6 place-items-center rounded-full border bg-card shadow-sm transition hover:border-destructive hover:text-destructive disabled:opacity-50"
          >
            <PinOffIcon className="size-3" />
          </button>
        )}
      </div>
    </motion.li>
  );
}

/** A channel's (or a thread's) pins, behind the pin in its header. */
export function ChannelPinsButton({
  instanceKey,
  serverId,
  channelId,
  threadId = "",
  canUnpin,
  onJump,
}: {
  instanceKey: string;
  serverId: string;
  channelId: string;
  threadId?: string;
  canUnpin: boolean;
  onJump: (messageId: string) => void;
}) {
  const pins = useChannelPins(instanceKey, channelId, threadId);
  return (
    <PinsPopover
      count={pins?.hasMore ? 0 : (pins?.messages.length ?? 0)}
      onOpen={() => void loadChannelPins(instanceKey, serverId, channelId, threadId)}
    >
      {(close) => (
        <ChannelPinsList
          instanceKey={instanceKey}
          serverId={serverId}
          channelId={channelId}
          threadId={threadId}
          canUnpin={canUnpin}
          onJump={(id) => {
            close();
            onJump(id);
          }}
        />
      )}
    </PinsPopover>
  );
}

function ChannelPinsList({
  instanceKey,
  serverId,
  channelId,
  threadId,
  canUnpin,
  onJump,
}: {
  instanceKey: string;
  serverId: string;
  channelId: string;
  threadId: string;
  canUnpin: boolean;
  onJump: (messageId: string) => void;
}) {
  const { t } = useI18n();
  const pins = useChannelPins(instanceKey, channelId, threadId);
  const users = useFuwa((s) => s.instances[instanceKey]?.users);
  if (!pins || (pins.status === "loading" && pins.messages.length === 0)) return <Loading />;
  if (pins.status === "failed" && pins.messages.length === 0)
    return <Failed onRetry={() => void loadChannelPins(instanceKey, serverId, channelId, threadId)} />;
  if (pins.messages.length === 0) return <Empty hint={t("chattools.pins.emptyChannel")} />;
  return (
    <>
      <ul className="flex flex-col">
        <AnimatePresence initial={false}>
          {pins.messages.map((m: Message, n) => (
            <PinRow
              key={m.id}
              index={n}
              author={users?.[m.authorId]}
              at={toDate(m.createdAt)}
              body={
                m.content ? (
                  <MessageBody content={m.content} emojis={m.emojis} display="cozy" />
                ) : (
                  <span className="text-muted-foreground italic">{t("chattools.pins.files")}</span>
                )
              }
              onJump={() => onJump(m.id)}
              onUnpin={canUnpin ? () => pinMessage(instanceKey, serverId, channelId, m.id, false) : undefined}
            />
          ))}
        </AnimatePresence>
      </ul>
      {pins.hasMore && <MoreButton onClick={() => void loadChannelPins(instanceKey, serverId, channelId, threadId, true)} />}
    </>
  );
}

function MoreButton({ onClick }: { onClick: () => void }) {
  const { t } = useI18n();
  return (
    <button
      type="button"
      onClick={onClick}
      className="mx-auto mt-1 mb-2 block rounded-full bg-muted px-3 py-1 text-xs font-bold transition hover:bg-muted/70"
    >
      {t("chattools.pins.more")}
    </button>
  );
}

/** A conversation's pins, drawn from what this device opened: the instance only says which messages. */
export function DmPinsButton({
  instanceKey,
  conversationId,
  users,
  onJump,
}: {
  instanceKey: string;
  conversationId: string;
  users: User[];
  onJump: (seq: number) => void;
}) {
  const pins = useDmPins(instanceKey, conversationId);
  return (
    <PinsPopover count={pins?.pins.length ?? 0} onOpen={() => void loadDmPins(instanceKey, conversationId)}>
      {(close) => (
        <DmPinsList
          instanceKey={instanceKey}
          conversationId={conversationId}
          users={users}
          onJump={(seq) => {
            close();
            onJump(seq);
          }}
        />
      )}
    </PinsPopover>
  );
}

function DmPinsList({
  instanceKey,
  conversationId,
  users,
  onJump,
}: {
  instanceKey: string;
  conversationId: string;
  users: User[];
  onJump: (seq: number) => void;
}) {
  const { t } = useI18n();
  const pins = useDmPins(instanceKey, conversationId);
  const items = useFuwa((s) => s.instances[instanceKey]?.dms.items[conversationId] ?? NO_ITEMS);
  const list = pins?.pins ?? NO_PINS;
  if (!pins || (pins.status === "loading" && list.length === 0)) return <Loading />;
  if (pins.status === "failed" && list.length === 0) return <Failed onRetry={() => void loadDmPins(instanceKey, conversationId)} />;
  if (list.length === 0) return <Empty hint={t("chattools.pins.emptyDm")} />;
  return (
    <>
      <ul className="flex flex-col">
        <AnimatePresence initial={false}>
          {list.map((pin, n) => {
            const seq = Number(pin.sequence);
            const item = items.find((i) => i.seq === seq && isMessage(i));
            const author = users.find((u) => u.id === item?.senderId);
            const body = !item ? (
              <span className="inline-flex items-center gap-1.5 text-muted-foreground italic">
                <LockKeyholeIcon className="size-3.5 shrink-0" />
                {t("chattools.pins.notOnDevice")}
              </span>
            ) : item.kind === "voice" ? (
              <span className="text-muted-foreground italic">{t("chattools.pins.voice")}</span>
            ) : item.content ? (
              <MessageBody content={item.content} display="cozy" />
            ) : (
              <span className="text-muted-foreground italic">{t("chattools.pins.files")}</span>
            );
            return (
              <PinRow
                key={seq}
                index={n}
                author={author}
                at={new Date(item?.at ?? Number(pin.pinnedAt?.seconds ?? 0n) * 1000)}
                body={body}
                onJump={item ? () => onJump(seq) : undefined}
                onUnpin={() => pinDm(instanceKey, conversationId, seq, false)}
              />
            );
          })}
        </AnimatePresence>
      </ul>
      {pins.hasMore && <MoreButton onClick={() => void loadDmPins(instanceKey, conversationId, true)} />}
    </>
  );
}
