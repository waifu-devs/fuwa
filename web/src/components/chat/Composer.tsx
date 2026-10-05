import { BarChart3Icon, HourglassIcon, LockIcon, ScrollTextIcon, SendHorizontalIcon, SmileIcon, SnailIcon } from "lucide-react";
import { AnimatePresence, m as motion, useAnimationControls } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState, type ClipboardEvent, type KeyboardEvent, type RefObject } from "react";
import { MessageKind, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { run, sendMessage, uploadVoice } from "@/fuwa/actions";
import { voiceLimits } from "@/fuwa/dms";
import { instanceHas } from "@/lib/compat";
import { VoiceRecorder } from "@/components/voice/VoiceRecorder";
import type { Clip } from "@/voice/recorder";
import { useAccess } from "@/fuwa/hooks";
import { AttachButton, DropOverlay, StagedTray, UploadRing } from "@/components/chat/ComposerFiles";
import { addFiles, takeFiles, useStaged, type Staged } from "@/components/chat/staged";
import { useFuwa } from "@/fuwa/store";
import { accountKey } from "@/fuwa/saved";
import { draftKey as toDraftKey, getDraft, setDraft } from "@/lib/drafts";
import { MentionPicker, useMentionPicker, type MentionPickerState } from "@/components/chat/MentionPicker";
import { CommandForm, CommandPicker, useCommandPicker, type CommandChoice, type CommandPickerState } from "@/components/chat/Commands";
import { TimestampPicker } from "@/components/chat/TimestampPicker";
import { EmojiPicker } from "@/components/EmojiPicker";
import { PollEditor } from "@/components/chat/PollEditor";
import { useContextMenu } from "@/components/ContextMenu";
import { composerMenu } from "@/components/menus/composer";
import { COMPOSER_INSERT } from "@/components/menus/member";
import { useCatalog } from "@/lib/emoji-catalog";
import { GifPicker } from "@/components/chat/GifPicker";
import { RulesDialog } from "@/components/join/Rules";
import { SPRING } from "@/lib/motion";
import { Button } from "@/components/ui/button";
import { formatDuration, formatLeft, timedOutUntil, toDate } from "@/lib/format";
import { type I18n, T, useI18n } from "@/i18n/react";
import { hasIn } from "@/lib/permissions";
import { comboLabel } from "@/lib/keybinds";
import { sendsMessage } from "@/components/chat/send-keys";
import { usePrefs, type SendWith } from "@/lib/prefs";
import { onCommand } from "@/lib/ui";
import { cn } from "@/lib/utils";

const MAX = 4000;

/** When you last sent here: your newest message, or one still on its way. */
function useLastSent(instanceKey: string, channelId: string) {
  return useFuwa((s) => {
    const i = s.instances[instanceKey];
    const me = i?.me?.id;
    const items = i?.messages[channelId]?.items ?? [];
    let last = 0;
    for (let n = items.length - 1; n >= 0; n--) {
      const m = items[n]!;
      if (m.authorId === me && m.kind === MessageKind.UNSPECIFIED) {
        last = toDate(m.createdAt).getTime();
        break;
      }
    }
    for (const p of i?.pending[channelId] ?? []) if (!p.failed) last = Math.max(last, p.createdAt);
    return last;
  });
}

/**
 * May send files here (`allowed`: a member with Attach files). The channel's
 * home takes and keeps them; one on another instance only once this instance
 * sends files there.
 */
function useCanAttach(instanceKey: string, channel: Channel, allowed: boolean) {
  const filesElsewhere = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "shared-files-elsewhere"));
  const elsewhere = !!channel.shared && !channel.shared.home && !!channel.shared.homeServer?.instance;
  return allowed && (filesElsewhere || !elsewhere);
}

/**
 * What keeps you from sending here right now: a time-out, or the wait slow
 * mode puts between your messages. People who manage messages or channels
 * here skip slow mode, as on the server. Ticks only while something is
 * counting down.
 */
function useSendGate(instanceKey: string, serverId: string, channel: Channel) {
  const member = useFuwa((s) => {
    const i = s.instances[instanceKey];
    return i?.members[serverId]?.find((m) => m.user?.id === i.me?.id);
  });
  const lastSent = useLastSent(instanceKey, channel.id);
  const access = useAccess(instanceKey, serverId);
  const canAttach = useCanAttach(instanceKey, channel, !!member && hasIn(access, channel.id, Permission.ATTACH_FILES));
  const [, tick] = useState(0);
  const now = Date.now();
  const exempt = hasIn(access, channel.id, Permission.MANAGE_MESSAGES) || hasIn(access, channel.id, Permission.MANAGE_CHANNELS);
  const slowmode = exempt ? 0 : channel.slowmodeSeconds;
  const until = timedOutUntil(member, now)?.getTime() ?? 0;
  const ready = slowmode && lastSent ? lastSent + slowmode * 1000 : 0;
  const counting = until > now || ready > now;
  useEffect(() => {
    if (!counting) return;
    const id = setInterval(() => tick((n) => n + 1), 250);
    return () => clearInterval(id);
  }, [counting]);
  return {
    now,
    /** Synced and allowed to write here. */
    canSend: !member || hasIn(access, channel.id, Permission.SEND_MESSAGES),
    canAttach,
    canStartThreads: !member || hasIn(access, channel.id, Permission.CREATE_THREADS),
    /** Joined, but hasn't agreed to the server's rules yet. */
    pending: !!member && access.pending,
    slowmode,
    /** Slow mode is on here, but it doesn't hold you back. */
    exempt: exempt && channel.slowmodeSeconds > 0,
    timedOutUntil: until > now ? until : 0,
    cooldownUntil: ready > now ? ready : 0,
  };
}

/** State that belongs to `key`: under a new key it starts over from `initial`. */
function useKeyedState<T>(key: string, initial: (key: string) => T) {
  const [state, setState] = useState(() => ({ key, value: initial(key) }));
  const value = state.key === key ? state.value : initial(key);
  const set = (next: T) => setState({ key, value: next });
  return [value, set] as const;
}

/** The box's text, kept as the draft for this channel or thread as you type. */
function useDraft(instanceKey: string, channelId: string, threadId: string | undefined) {
  // Drafts are kept per account and channel, and per thread apart from their channel.
  const meId = useFuwa((s) => s.instances[instanceKey]?.me?.id ?? "");
  const draftKey = toDraftKey(accountKey(instanceKey, meId), threadId ? `thread:${threadId}` : channelId);
  const [text, setShown] = useKeyedState(draftKey, getDraft);
  const setText = (next: string) => {
    setDraft(draftKey, next);
    setShown(next);
  };
  return { draftKey, text, setText };
}

/**
 * Where you type. Enter sends (or Ctrl+Enter, by the Chat setting), the other
 * adds a line, Up in an empty box edits your last message. Drafts survive
 * switching channels. A time-out swaps the box for a countdown; slow mode
 * winds the send button down until you can send again.
 */
export function Composer({
  instanceKey,
  serverId,
  channel,
  thread,
  placeholder,
  onEditLast,
}: {
  instanceKey: string;
  serverId: string;
  channel: Channel;
  /** Replying in the thread under this message: locked keeps you out; not started yet needs Start threads. */
  thread?: { id: string; locked: boolean; started: boolean };
  placeholder: string;
  onEditLast: () => void;
}) {
  const lang = useI18n();
  const { t } = lang;
  const channelId = channel.id;
  const { draftKey, text, setText } = useDraft(instanceKey, channelId, thread?.id);
  const [alsoToChannel, setAlsoToChannel] = useState(false);
  // A problem with a voice message stays with the channel it happened in.
  const [voiceProblem, setVoiceProblem] = useKeyedState<string | null>(draftKey, () => null);
  // Only where the instance takes voice messages in channels.
  const voiceHere = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "voice-messages-in-channels"));
  const box = useRef<HTMLTextAreaElement>(null);
  const plane = useAnimationControls();
  const nudge = useAnimationControls();
  const sendWith = usePrefs((p) => p.sendWith);
  const gate = useSendGate(instanceKey, serverId, channel);
  const cooling = gate.cooldownUntil > 0;
  const timedOut = gate.timedOutUntil > 0;
  const picker = useMentionPicker(instanceKey, serverId, channel, box, text, setText);
  const { commandsHere, commands, chosen } = useCommandsHere(instanceKey, serverId, channel, !!thread, text, setText);
  const command = chosen?.command.name;
  const server = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId));
  const [rules, setRules] = useState(false);
  const canPoll = useCanPoll(instanceKey, serverId, channel, !!thread);
  const [polling, setPolling] = useState(false);
  const catalog = useCatalog(instanceKey, serverId, channel);
  const staged = useStaged(channelId);
  const where = { instanceKey, serverId, channelId };

  const { insert, replaceSelection } = useCaretEdit(box, text, setText);

  const menu = useComposerMenu(box, { instanceKey, serverId, channel, insert }, replaceSelection);

  useBox({ box, draftKey, text, gate, insert });

  const { send, sendVoice, ready, uploading, uploaded } = useSender({
    where,
    draftKey,
    thread,
    alsoToChannel,
    text,
    setText,
    staged,
    gate,
    chosen,
    commands,
    picker,
    box,
    nudge,
    plane,
    onVoiceProblem: setVoiceProblem,
  });

  const { onKeyDown, onPaste } = useBoxInput({
    where,
    canAttach: gate.canAttach,
    commands: commandsHere ? commands : null,
    picker,
    sendWith,
    empty: !text,
    send,
    onEditLast,
  });

  const blocked = blockedNotice({ gate, thread, channelName: channel.name, onRead: () => setRules(true), t });
  const shown = partsShown({ gate, chosen: !!chosen, empty: !text && !staged.length, voiceHere });

  return (
    <div className="px-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] sm:px-4">
      <AnimatePresence mode="popLayout" initial={false}>
        {blocked ?? (
          <motion.div
            key="composer"
            initial={{ opacity: 0, y: 12, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 12, scale: 0.98 }}
            transition={SPRING}
          >
      <motion.div animate={nudge} className="composer relative rounded-2xl border bg-card px-3 py-2">
        <AnimatePresence initial={false}>{staged.length > 0 && <StagedTray key="files" where={where} staged={staged} />}</AnimatePresence>
        <div className="flex items-end gap-2">
        {shown.attach && <AttachButton where={where} />}
        <MentionPicker picker={picker} />
        {commandsHere && <CommandPicker picker={commands} />}
        {chosen ? (
          <CommandForm instanceKey={instanceKey} serverId={serverId} picker={commands} onDone={() => requestAnimationFrame(() => box.current?.focus())} />
        ) : (
        <textarea
          ref={box}
          data-composer
          rows={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onSelect={picker.onSelect}
          onKeyDown={(e) => {
            menu.onKeyDown(e);
            onKeyDown(e);
          }}
          onContextMenu={menu.onContextMenu}
          data-context-menu=""
          onPaste={onPaste}
          placeholder={placeholder}
          aria-label={placeholder}
          className="scroll-thin max-h-[40vh] min-h-6 flex-1 resize-none bg-transparent py-1.5 text-[0.95rem] leading-6 outline-none placeholder:text-muted-foreground"
        />
        )}
        <CharsLeft length={text.length} />
        {shown.tools && (
          <ComposerTools
            instanceKey={instanceKey}
            serverId={serverId}
            channelId={channelId}
            catalog={catalog}
            onInsert={insert}
            canPoll={canPoll}
            polling={polling}
            onPoll={() => setPolling(true)}
          />
        )}
        {shown.voice ? (
          <VoiceRecorder
            key={draftKey}
            maxMs={() => voiceLimits(instanceKey).then((l) => l.maxMs)}
            onSend={sendVoice}
            onProblem={setVoiceProblem}
          />
        ) : (
          <SendButton
            onSend={send}
            ready={ready}
            command={command}
            uploading={uploading}
            uploaded={uploaded}
            gate={gate}
            plane={plane}
          />
        )}
        </div>
      </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
      {shown.dropOverlay && <DropOverlay where={where} channelName={channel.name} />}
      <ComposerFooter
        thread={thread}
        gate={gate}
        channel={channel}
        alsoToChannel={alsoToChannel}
        onAlsoToChannel={setAlsoToChannel}
        voiceProblem={voiceProblem}
        command={command}
        commandsHere={commandsHere}
        sendWith={sendWith}
      />
      {canPoll && <PollEditor open={polling} onOpenChange={setPolling} instanceKey={instanceKey} serverId={serverId} channel={channel} />}
      {server && <RulesDialog open={rules} onOpenChange={setRules} instanceKey={instanceKey} server={server} agree={gate.pending} />}
    </div>
  );
}

type SendGate = ReturnType<typeof useSendGate>;
type Controls = ReturnType<typeof useAnimationControls>;

/**
 * Agents' commands run in a channel, not a thread, and stay with the server
 * a channel lives in, so none in channels shared between servers.
 */
function useCommandsHere(instanceKey: string, serverId: string, channel: Channel, inThread: boolean, text: string, setText: (text: string) => void) {
  const commandsHere = !inThread && !channel.shared;
  const commands = useCommandPicker(instanceKey, serverId, channel.id, commandsHere ? text : "", setText);
  return { commandsHere, commands, chosen: commandsHere ? commands.chosen : null };
}

/** The box's right-click menu. */
function useComposerMenu(
  box: RefObject<HTMLTextAreaElement | null>,
  at: { instanceKey: string; serverId: string; channel: Channel; insert: (piece: string) => void },
  replaceSelection: (piece: string) => void,
) {
  return useContextMenu("composer", () =>
    box.current
      ? composerMenu(at, {
          box: box.current,
          replaceSelection,
          openEmoji: () => box.current?.parentElement?.querySelector<HTMLElement>("[data-composer-emoji]")?.click(),
        })
      : null,
    { touch: false },
  );
}

/** Which of the box's parts show, by what you may do here and what's in it. */
function partsShown({ gate, chosen, empty, voiceHere }: { gate: SendGate; chosen: boolean; empty: boolean; voiceHere: boolean }) {
  return {
    attach: gate.canAttach && !chosen,
    /** Timestamps, emoji, GIFs and polls; a command being filled in has none. */
    tools: !chosen,
    /** The voice recorder takes the send button's place while there's nothing to send. */
    voice: voiceHere && gate.canAttach && !chosen && empty && gate.cooldownUntil <= 0,
    dropOverlay: gate.canAttach && !gate.pending && gate.timedOutUntil <= 0 && gate.canSend,
  };
}

/** Typing into the box from elsewhere: at the caret, or over what's selected. */
function useCaretEdit(box: RefObject<HTMLTextAreaElement | null>, text: string, setText: (text: string) => void) {
  /** Puts text at the caret, with a space before it when it would touch a word. */
  function insert(piece: string) {
    const el = box.current;
    const start = el?.selectionStart ?? text.length;
    const end = el?.selectionEnd ?? text.length;
    const gap = start > 0 && !/\s$/.test(text.slice(0, start)) ? " " : "";
    const next = text.slice(0, start) + gap + piece + text.slice(end);
    setText(next);
    const at = start + gap.length + piece.length;
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(at, at);
    });
  }

  /** Replaces what's selected (or nothing, at the caret) with `piece`, for the right-click menu's paste and cut. */
  function replaceSelection(piece: string) {
    const el = box.current;
    const start = el?.selectionStart ?? text.length;
    const end = el?.selectionEnd ?? text.length;
    setText(text.slice(0, start) + piece + text.slice(end));
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(start + piece.length, start + piece.length);
    });
  }

  return { insert, replaceSelection };
}

/** Sending what's in the box, or a voice message, and whether it's ready to go. */
function useSender({
  where,
  draftKey,
  thread,
  alsoToChannel,
  text,
  setText,
  staged,
  gate,
  chosen,
  commands,
  picker,
  box,
  nudge,
  plane,
  onVoiceProblem,
}: {
  where: { instanceKey: string; serverId: string; channelId: string };
  draftKey: string;
  thread?: { id: string };
  alsoToChannel: boolean;
  text: string;
  setText: (text: string) => void;
  staged: Staged[];
  gate: SendGate;
  chosen: CommandChoice | null;
  commands: CommandPickerState;
  picker: MentionPickerState;
  box: RefObject<HTMLTextAreaElement | null>;
  nudge: Controls;
  plane: Controls;
  onVoiceProblem: (problem: string | null) => void;
}) {
  const { t } = useI18n();
  const { instanceKey, serverId, channelId } = where;
  const cooling = gate.cooldownUntil > 0;
  const timedOut = gate.timedOutUntil > 0;
  const content = text.trim();
  const tooLong = text.length > MAX;
  const uploading = staged.some((s) => !s.done && !s.failed);
  const brokenFile = staged.some((s) => s.failed);
  const uploaded = staged.length ? staged.reduce((sum, s) => sum + (s.failed ? 0 : s.sent), 0) / staged.length : 0;
  const shake = () => void nudge.start({ x: [0, -5, 5, -3, 3, 0], transition: { duration: 0.4 } });

  function runCommand() {
    if (timedOut) return;
    if (commands.missing.length || commands.running) return shake();
    void commands.send().then((ran) => ran && box.current?.focus());
  }

  function send() {
    if (chosen) return runCommand();
    if ((!content && !staged.length) || tooLong || timedOut) return;
    if (cooling || uploading || brokenFile) return shake();
    const files = takeFiles(channelId);
    setText("");
    void plane.start({
      x: [0, 28, -18, 0],
      y: [0, -14, 8, 0],
      opacity: [1, 0, 0, 1],
      rotate: [0, -20, 0, 0],
      transition: { duration: 0.55, times: [0, 0.45, 0.5, 1], ease: "easeOut" },
    });
    run(
      sendMessage(
        instanceKey,
        serverId,
        channelId,
        picker.encode(content),
        files,
        thread ? { threadId: thread.id, alsoToChannel } : undefined,
      ),
    ).catch(() => {
      // The message stays in the list, marked as failed, with a retry.
    });
  }

  /** A recorded voice message goes up as its message's only file, then the message is sent. */
  function sendVoice(clip: Clip) {
    if (timedOut) return;
    const target = thread ? { threadId: thread.id, alsoToChannel } : undefined;
    run(uploadVoice(instanceKey, serverId, clip)).then(
      (file) =>
        run(sendMessage(instanceKey, serverId, channelId, "", [file], target)).catch(() => {
          // The message stays in the list, marked as failed, with a retry.
        }),
      (err: Error) => onVoiceProblem(t("chat.composer.voiceFailed", { error: err.message || t("chat.composer.tryAgain") })),
    );
  }

  const ready = chosen
    ? !commands.missing.length && !commands.running
    : (!!content || staged.length > 0) && !tooLong && !cooling && !uploading && !brokenFile;
  return { send, sendVoice, ready, uploading, uploaded };
}

/** Whether you may start a poll here. */
function useCanPoll(instanceKey: string, serverId: string, channel: Channel, inThread: boolean) {
  const access = useAccess(instanceKey, serverId);
  // Polls are counted where a channel lives: in a shared one, once this instance takes them there.
  const pollsShared = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "shared-polls"));
  // Polls stay out of threads in shared channels.
  return hasIn(access, channel.id, Permission.CREATE_POLLS) && (!channel.shared || (pollsShared && !inThread));
}

/** What shows in place of the box when you can't write here, or null when you can. */
function blockedNotice({
  gate,
  thread,
  channelName,
  onRead,
  t,
}: {
  gate: SendGate;
  thread?: { locked: boolean; started: boolean };
  channelName: string;
  onRead: () => void;
  t: I18n["t"];
}) {
  if (gate.pending) return <AgreeFirst key="rules" onRead={onRead} />;
  if (gate.timedOutUntil > 0) return <TimedOut key="timed-out" left={gate.timedOutUntil - gate.now} />;
  if (thread?.locked) return <ReadOnly key="locked" title={t("chat.composer.lockedTitle")} about={t("chat.composer.lockedAbout")} />;
  if (!gate.canSend || (thread && !thread.started && !gate.canStartThreads))
    return (
      <ReadOnly
        key="read-only"
        title={gate.canSend ? t("chat.composer.noThreads", { channel: channelName }) : t("chat.composer.noMessages", { channel: channelName })}
        about={t("chat.composer.readOnlyAbout")}
      />
    );
  return null;
}

/** The box's own ways: "Mention" from menus types here, it takes focus on a new channel, and it grows with the text. */
function useBox({
  box,
  draftKey,
  text,
  gate,
  insert,
}: {
  box: RefObject<HTMLTextAreaElement | null>;
  draftKey: string;
  text: string;
  gate: SendGate;
  insert: (piece: string) => void;
}) {
  // "Mention" in someone's menu types their name here, while you can write.
  const typing = gate.canSend && !gate.pending && gate.timedOutUntil <= 0;
  const inserts = useRef(insert);
  useEffect(() => {
    inserts.current = insert;
  });
  useEffect(() => (typing ? onCommand(COMPOSER_INSERT, (piece) => piece && inserts.current(piece)) : undefined), [typing]);

  useEffect(() => {
    if (window.matchMedia("(pointer: fine)").matches) box.current?.focus();
  }, [box, draftKey]);

  // Grow with the text, up to a point.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [box, text]);
}

/** Keys and pasting in the box: sending, the pickers, editing your last message, and pasted files. */
function useBoxInput({
  where,
  canAttach,
  commands,
  picker,
  sendWith,
  empty,
  send,
  onEditLast,
}: {
  where: { instanceKey: string; serverId: string; channelId: string };
  canAttach: boolean;
  /** The command picker, where commands run. */
  commands: CommandPickerState | null;
  picker: MentionPickerState;
  sendWith: SendWith;
  empty: boolean;
  send: () => void;
  onEditLast: () => void;
}) {
  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.defaultPrevented || e.nativeEvent.isComposing) return;
    if (commands?.onKeyDown(e)) return;
    if (picker.onKeyDown(e)) return;
    if (sendsMessage(e, sendWith)) {
      e.preventDefault();
      send();
    } else if (e.key === "ArrowUp" && empty) {
      e.preventDefault();
      onEditLast();
    }
  }

  function onPaste(e: ClipboardEvent<HTMLTextAreaElement>) {
    const pasted = Array.from(e.clipboardData.files);
    if (!pasted.length || !canAttach) return;
    e.preventDefault();
    addFiles(where.instanceKey, where.serverId, where.channelId, pasted);
  }

  return { onKeyDown, onPaste };
}

/** Under the box: sending to the channel too from a thread, which keys send, and slow mode. */
function ComposerFooter({
  thread,
  gate,
  channel,
  alsoToChannel,
  onAlsoToChannel,
  voiceProblem,
  command,
  commandsHere,
  sendWith,
}: {
  thread?: { locked: boolean };
  gate: SendGate;
  channel: Channel;
  alsoToChannel: boolean;
  onAlsoToChannel: (on: boolean) => void;
  voiceProblem: string | null;
  command?: string;
  commandsHere: boolean;
  sendWith: SendWith;
}) {
  const { t } = useI18n();
  return (
    <div className="mt-1 flex items-center gap-3 px-1 text-[0.7rem] text-muted-foreground">
      {thread && gate.canSend && !thread.locked && (
        <label className="flex shrink-0 cursor-pointer items-center gap-1.5 font-bold select-none">
          <input
            type="checkbox"
            checked={alsoToChannel}
            onChange={(e) => onAlsoToChannel(e.target.checked)}
            className="size-3.5 accent-[var(--primary)]"
          />
          {t("chat.composer.alsoSend", { channel: channel.name })}
        </label>
      )}
      {voiceProblem ? (
        <p role="alert" className="min-w-0 flex-1 truncate font-bold text-destructive">
          {voiceProblem}
        </p>
      ) : (
        <ComposerHint command={command} commandsHere={commandsHere} inThread={!!thread} sendWith={sendWith} />
      )}
      <SlowModeNote gate={gate} shown={(gate.slowmode > 0 || gate.exempt) && gate.timedOutUntil <= 0 && gate.canSend && !gate.pending} slowmodeSeconds={channel.slowmodeSeconds} />
    </div>
  );
}

/** How many characters are left, once you get near the limit. */
function CharsLeft({ length }: { length: number }) {
  const lang = useI18n();
  return (
    <AnimatePresence>
      {length > MAX - 500 && (
        <motion.span
          initial={{ opacity: 0, scale: 0.8 }}
          animate={{ opacity: 1, scale: 1 }}
          exit={{ opacity: 0, scale: 0.8 }}
          className={cn("mb-2 text-xs tabular-nums", length > MAX ? "font-bold text-destructive" : "text-muted-foreground")}
        >
          {lang.number(MAX - length)}
        </motion.span>
      )}
    </AnimatePresence>
  );
}

/** The buttons beside the box: timestamps, emoji, GIFs and polls. */
function ComposerTools({
  instanceKey,
  serverId,
  channelId,
  catalog,
  onInsert,
  canPoll,
  polling,
  onPoll,
}: {
  instanceKey: string;
  serverId: string;
  channelId: string;
  catalog: ReturnType<typeof useCatalog>;
  onInsert: (piece: string) => void;
  canPoll: boolean;
  polling: boolean;
  onPoll: () => void;
}) {
  const { t } = useI18n();
  return (
    <>
      <TimestampPicker onPick={onInsert} />
      <EmojiPicker
        catalog={catalog}
        closeOnPick={false}
        onPick={(emoji) => onInsert(emoji.text.startsWith("<") ? `:${emoji.name}:` : emoji.text)}
      >
        {(open) => (
          <motion.button
            type="button"
            aria-label={t("chat.composer.emoji")}
            data-composer-emoji
            whileHover={{ scale: 1.12, rotate: -10 }}
            whileTap={{ scale: 0.85 }}
            className={cn("group mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:text-primary", open && "bg-primary/10 text-primary")}
          >
            <SmileIcon className="size-[18px]" />
          </motion.button>
        )}
      </EmojiPicker>
      <GifPicker instanceKey={instanceKey} serverId={serverId} channelId={channelId} />
      {canPoll && (
        <motion.button
          type="button"
          aria-label={t("chat.composer.makePoll")}
          title={t("chat.composer.makePoll")}
          onClick={onPoll}
          whileHover={{ scale: 1.12, y: -1 }}
          whileTap={{ scale: 0.85 }}
          className={cn("mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:text-primary", polling && "bg-primary/10 text-primary")}
        >
          <BarChart3Icon className="size-[18px]" />
        </motion.button>
      )}
    </>
  );
}

/** Sends (or runs the chosen command), winding down while slow mode holds you back. */
function SendButton({
  onSend,
  ready,
  command,
  uploading,
  uploaded,
  gate,
  plane,
}: {
  onSend: () => void;
  ready: boolean;
  /** The chosen command's name, when one is being filled in. */
  command?: string;
  uploading: boolean;
  /** How much of the files went up, 0 to 1. */
  uploaded: number;
  gate: SendGate;
  plane: Controls;
}) {
  const lang = useI18n();
  const { t } = lang;
  const cooling = gate.cooldownUntil > 0;
  const label =
    command !== undefined
      ? t("chat.composer.run", { command })
      : cooling
        ? t("chat.composer.slowSendAgain", { time: formatLeft(lang, gate.cooldownUntil - gate.now) })
        : uploading
          ? t("chat.composer.uploading", { percent: lang.number(uploaded, { style: "percent", maximumFractionDigits: 0 }) })
          : t("chat.composer.send");
  return (
    <motion.button
      type="button"
      onClick={onSend}
      disabled={!ready}
      aria-label={label}
      whileTap={{ scale: 0.85 }}
      initial={false}
      animate={{ scale: ready || cooling ? 1 : 0.9 }}
      transition={{ type: "spring", stiffness: 600, damping: 20 }}
      className={cn(
        "relative mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl transition-colors",
        ready ? "bg-primary text-primary-foreground shadow-[0_6px_18px_-8px_var(--primary)]" : "text-muted-foreground",
      )}
    >
      <AnimatePresence mode="popLayout" initial={false}>
        {cooling ? (
          <Cooldown key="cooldown" left={gate.cooldownUntil - gate.now} total={gate.slowmode * 1000} />
        ) : uploading ? (
          <UploadRing key="uploading" share={uploaded} />
        ) : (
          <motion.span
            key="plane"
            initial={{ scale: 0.4, rotate: -45, opacity: 0 }}
            animate={{ scale: 1, rotate: 0, opacity: 1 }}
            exit={{ scale: 0.4, opacity: 0 }}
            transition={{ type: "spring", stiffness: 600, damping: 18 }}
          >
            <motion.span animate={plane} className="block">
              <SendHorizontalIcon className="size-[18px]" />
            </motion.span>
          </motion.span>
        )}
      </AnimatePresence>
    </motion.button>
  );
}

/** Which keys send and which add a line, under the box on wider screens. */
function ComposerHint({
  command,
  commandsHere,
  inThread,
  sendWith,
}: {
  command?: string;
  commandsHere: boolean;
  inThread: boolean;
  sendWith: SendWith;
}) {
  return (
    <p className={cn("hidden min-w-0 flex-1 truncate", !inThread && "sm:block")}>
      {command !== undefined ? (
        <T k="chat.composer.hintCommand" values={{ keys: <b>{comboLabel("Enter")}</b>, command, esc: <b>Esc</b> }} />
      ) : (
        <T
          k={commandsHere ? "chat.composer.hintCommands" : "chat.composer.hint"}
          values={{
            send: <b>{sendWith === "enter" ? comboLabel("Enter") : comboLabel("Mod+Enter")}</b>,
            newLine: <b>{sendWith === "enter" ? comboLabel("Shift+Enter") : comboLabel("Enter")}</b>,
          }}
        />
      )}
    </p>
  );
}

/** Slow mode's pace, or how long until you can send again. */
function SlowModeNote({ gate, shown, slowmodeSeconds }: { gate: SendGate; shown: boolean; slowmodeSeconds: number }) {
  const lang = useI18n();
  const { t } = lang;
  const cooling = gate.cooldownUntil > 0;
  return (
    <AnimatePresence initial={false}>
      {shown && (
        <motion.p
          initial={{ opacity: 0, x: 8 }}
          animate={{ opacity: 1, x: 0 }}
          exit={{ opacity: 0, x: 8 }}
          transition={SPRING}
          className={cn("ml-auto flex shrink-0 items-center gap-1 font-bold tabular-nums", cooling && "text-amber-600 dark:text-amber-400")}
          title={gate.exempt ? t("chat.composer.slowExempt") : undefined}
        >
          <SnailIcon className={cn("size-3.5", cooling && "animate-[crawl_1.6s_ease-in-out_infinite]")} />
          {gate.exempt
            ? t("chat.composer.slowOthers", { duration: formatDuration(lang, slowmodeSeconds) })
            : cooling
              ? t("chat.composer.slowCooling", { time: formatLeft(lang, gate.cooldownUntil - gate.now) })
              : t("chat.composer.slowEvery", { duration: formatDuration(lang, gate.slowmode) })}
        </motion.p>
      )}
    </AnimatePresence>
  );
}

/** In place of the box until you agree to the server's rules: one button to read them. */
function AgreeFirst({ onRead }: { onRead: () => void }) {
  const { t } = useI18n();
  return (
    <motion.div
      initial={{ opacity: 0, y: 12, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: -8, scale: 0.98 }}
      transition={SPRING}
      role="status"
      className="flex flex-wrap items-center gap-3 rounded-2xl border border-primary/40 bg-primary/10 px-3 py-2.5"
    >
      <motion.span
        initial={{ rotate: -20, scale: 0.6 }}
        animate={{ rotate: [0, -12, 10, 0], scale: 1 }}
        transition={{ ...SPRING, rotate: { duration: 0.7, delay: 0.15 } }}
        className="grid size-9 shrink-0 place-items-center rounded-xl bg-primary/15 text-primary"
      >
        <ScrollTextIcon className="size-[18px]" />
      </motion.span>
      <div className="min-w-0 flex-1">
        <p className="text-sm font-bold">{t("chat.composer.agreeTitle")}</p>
        <p className="text-xs text-muted-foreground">{t("chat.composer.agreeAbout")}</p>
      </div>
      <Button size="sm" onClick={onRead} className="btn h-9 shrink-0 rounded-xl px-4 font-bold max-sm:w-full">
        {t("chat.composer.readRules")}
      </Button>
    </motion.div>
  );
}

/** A ring that winds down to the moment slow mode lets you send again. */
function Cooldown({ left, total }: { left: number; total: number }) {
  const r = 15;
  const length = 2 * Math.PI * r;
  const share = total > 0 ? Math.min(1, left / total) : 0;
  const seconds = Math.ceil(left / 1000);
  return (
    <motion.span
      initial={{ scale: 0.4, opacity: 0 }}
      animate={{ scale: 1, opacity: 1 }}
      exit={{ scale: 1.4, opacity: 0 }}
      transition={{ type: "spring", stiffness: 600, damping: 20 }}
      className="relative grid size-9 place-items-center text-amber-600 dark:text-amber-400"
    >
      <svg viewBox="0 0 36 36" className="absolute inset-0 -rotate-90" aria-hidden>
        <circle cx="18" cy="18" r={r} fill="none" strokeWidth="2.5" className="stroke-amber-500/15" />
        <circle
          cx="18"
          cy="18"
          r={r}
          fill="none"
          strokeWidth="2.5"
          strokeLinecap="round"
          strokeDasharray={length}
          strokeDashoffset={length * (1 - share)}
          className="stroke-amber-500 transition-[stroke-dashoffset] duration-300 ease-linear"
        />
      </svg>
      <AnimatePresence mode="popLayout" initial={false}>
        <motion.span
          key={seconds > 99 ? "long" : seconds}
          initial={{ y: 8, opacity: 0 }}
          animate={{ y: 0, opacity: 1 }}
          exit={{ y: -8, opacity: 0 }}
          transition={{ duration: 0.18 }}
          className="relative text-[0.65rem] font-extrabold tabular-nums"
        >
          {seconds > 99 ? <SnailIcon className="size-4" /> : seconds}
        </motion.span>
      </AnimatePresence>
    </motion.span>
  );
}

/** In place of the box where your roles don't let you write. */
function ReadOnly({ title, about }: { title: string; about: string }) {
  return (
    <motion.div
      initial={{ opacity: 0, y: 12, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: -8, scale: 0.98 }}
      transition={SPRING}
      role="status"
      className="flex items-center gap-3 rounded-2xl border border-dashed bg-muted/40 px-3 py-2.5"
    >
      <motion.span
        initial={{ rotate: -20, scale: 0.6 }}
        animate={{ rotate: [0, -10, 8, 0], scale: 1 }}
        transition={{ ...SPRING, rotate: { duration: 0.6, delay: 0.1 } }}
        className="grid size-9 shrink-0 place-items-center rounded-xl bg-muted text-muted-foreground"
      >
        <LockIcon className="size-[18px]" />
      </motion.span>
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm font-bold">{title}</p>
        <p className="text-xs text-muted-foreground">{about}</p>
      </div>
    </motion.div>
  );
}

/** In place of the box while you're timed out: how long until you can talk again. */
function TimedOut({ left }: { left: number }) {
  const lang = useI18n();
  const { t } = lang;
  return (
    <motion.div
      initial={{ opacity: 0, y: 12, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: -8, scale: 0.98 }}
      transition={SPRING}
      role="status"
      className="flex items-center gap-3 rounded-2xl border border-amber-500/40 bg-amber-500/10 px-3 py-2.5"
    >
      <span className="grid size-9 shrink-0 place-items-center rounded-xl bg-amber-500/15 text-amber-600 dark:text-amber-400">
        <HourglassIcon className="size-[18px] animate-[flip_3s_ease-in-out_infinite]" />
      </span>
      <div className="min-w-0 flex-1">
        <p className="text-sm font-bold">{t("chat.composer.timedOutTitle")}</p>
        <p className="text-xs text-muted-foreground">{t("chat.composer.timedOutAbout")}</p>
      </div>
      <span className="shrink-0 rounded-full bg-amber-500/15 px-2.5 py-1 text-sm font-extrabold text-amber-600 tabular-nums dark:text-amber-400">
        {formatLeft(lang, left)}
      </span>
    </motion.div>
  );
}
