import { BarChart3Icon, HourglassIcon, LockIcon, ScrollTextIcon, SendHorizontalIcon, SmileIcon, SnailIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { MessageKind, Permission, type Channel } from "@/gen/fuwa/v1/types_pb";
import { run, sendMessage, uploadVoice } from "@/fuwa/actions";
import { voiceLimits } from "@/fuwa/dms";
import { instanceHas } from "@/lib/compat";
import { VoiceRecorder } from "@/components/voice/VoiceRecorder";
import type { Clip } from "@/voice/recorder";
import { useAccess } from "@/fuwa/hooks";
import { AttachButton, DropOverlay, StagedTray, UploadRing } from "@/components/chat/ComposerFiles";
import { addFiles, takeFiles, useStaged } from "@/components/chat/staged";
import { useFuwa } from "@/fuwa/store";
import { MentionPicker, useMentionPicker } from "@/components/chat/MentionPicker";
import { CommandForm, CommandPicker, useCommandPicker } from "@/components/chat/Commands";
import { TimestampPicker } from "@/components/chat/TimestampPicker";
import { EmojiPicker } from "@/components/EmojiPicker";
import { PollEditor } from "@/components/chat/PollEditor";
import { useContextMenu } from "@/components/ContextMenu";
import { composerMenu } from "@/components/menus/composer";
import { COMPOSER_INSERT } from "@/components/menus/member";
import { useCatalog } from "@/lib/emoji-catalog";
import { GifPicker } from "@/components/chat/GifPicker";
import { RulesDialog } from "@/components/join/Rules";
import { SPRING } from "@/components/motion";
import { Button } from "@/components/ui/button";
import { formatDuration, formatLeft, timedOutUntil, toDate } from "@/lib/format";
import { hasIn } from "@/lib/permissions";
import { comboLabel, isMac } from "@/lib/keybinds";
import { usePrefs, type SendWith } from "@/lib/prefs";
import { onCommand } from "@/lib/ui";
import { cn } from "@/lib/utils";

const MAX = 4000;
const drafts = new Map<string, string>();

/** Whether a key press sends, by the Chat setting: Enter, or Ctrl+Enter (Cmd+Return on a Mac). */
export function sendsMessage(e: KeyboardEvent<HTMLTextAreaElement>, sendWith: SendWith) {
  if (e.key !== "Enter" || e.nativeEvent.isComposing) return false;
  const mod = isMac ? e.metaKey : e.ctrlKey;
  return sendWith === "enter" ? !e.shiftKey && !mod : mod;
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
  // When you last sent here: your newest message, or one still on its way.
  const lastSent = useFuwa((s) => {
    const i = s.instances[instanceKey];
    const me = i?.me?.id;
    const items = i?.messages[channel.id]?.items ?? [];
    let last = 0;
    for (let n = items.length - 1; n >= 0; n--) {
      const m = items[n]!;
      if (m.authorId === me && m.kind === MessageKind.UNSPECIFIED) {
        last = toDate(m.createdAt).getTime();
        break;
      }
    }
    for (const p of i?.pending[channel.id] ?? []) if (!p.failed) last = Math.max(last, p.createdAt);
    return last;
  });
  const access = useAccess(instanceKey, serverId);
  // Files go into a channel shared from another instance once this one sends them there.
  const filesElsewhere = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "shared-files-elsewhere"));
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
    /**
     * May send files here. The channel's home takes and keeps them; one on
     * another instance only once this instance sends files there.
     */
    canAttach:
      !!member &&
      hasIn(access, channel.id, Permission.ATTACH_FILES) &&
      (filesElsewhere || !(channel.shared && !channel.shared.home && channel.shared.homeServer?.instance)),
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
  const channelId = channel.id;
  // Drafts are kept per channel, and per thread apart from their channel.
  const draftKey = thread ? `thread:${thread.id}` : channelId;
  const [text, setText] = useState(() => drafts.get(draftKey) ?? "");
  const [alsoToChannel, setAlsoToChannel] = useState(false);
  const [voiceProblem, setVoiceProblem] = useState<string | null>(null);
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
  // Agents' commands run in a channel, not a thread, and stay with the server
  // a channel lives in, so none in channels shared between servers.
  const commandsHere = !thread && !channel.shared;
  const commands = useCommandPicker(instanceKey, serverId, channelId, commandsHere ? text : "", setText);
  const chosen = commandsHere ? commands.chosen : null;
  const server = useFuwa((s) => s.instances[instanceKey]?.servers.find((x) => x.id === serverId));
  const [rules, setRules] = useState(false);
  const access = useAccess(instanceKey, serverId);
  // Polls are counted where a channel lives: in a shared one, once this instance takes them there.
  const pollsShared = useFuwa((s) => instanceHas(s.instances[instanceKey]?.node?.versions, "shared-polls"));
  const canPoll = hasIn(access, channelId, Permission.CREATE_POLLS) && (!channel.shared || pollsShared);
  const [polling, setPolling] = useState(false);
  const catalog = useCatalog(instanceKey, serverId, channel);
  const staged = useStaged(channelId);
  const where = { instanceKey, serverId, channelId };

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

  const menu = useContextMenu("composer", () =>
    box.current
      ? composerMenu(
          { instanceKey, serverId, channel, insert },
          {
            box: box.current,
            replaceSelection,
            openEmoji: () => box.current?.parentElement?.querySelector<HTMLElement>("[data-composer-emoji]")?.click(),
          },
        )
      : null,
    { touch: false },
  );

  // "Mention" in someone's menu types their name here.
  const typing = gate.canSend && !gate.pending && !timedOut;
  const inserts = useRef(insert);
  useEffect(() => {
    inserts.current = insert;
  });
  useEffect(() => (typing ? onCommand(COMPOSER_INSERT, (piece) => piece && inserts.current(piece)) : undefined), [typing]);

  useEffect(() => {
    setText(drafts.get(draftKey) ?? "");
    setVoiceProblem(null);
    if (window.matchMedia("(pointer: fine)").matches) box.current?.focus();
  }, [draftKey]);
  useEffect(() => {
    drafts.set(draftKey, text);
  }, [draftKey, text]);

  // Grow with the text, up to a point.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [text]);

  const content = text.trim();
  const tooLong = text.length > MAX;
  const uploading = staged.some((s) => !s.done && !s.failed);
  const brokenFile = staged.some((s) => s.failed);
  const uploaded = staged.length ? staged.reduce((sum, s) => sum + (s.failed ? 0 : s.sent), 0) / staged.length : 0;

  function send() {
    if (chosen) {
      if (timedOut) return;
      if (commands.missing.length || commands.running) {
        void nudge.start({ x: [0, -5, 5, -3, 3, 0], transition: { duration: 0.4 } });
        return;
      }
      void commands.send().then((ran) => ran && box.current?.focus());
      return;
    }
    if ((!content && !staged.length) || tooLong || timedOut) return;
    if (cooling || uploading || brokenFile) {
      void nudge.start({ x: [0, -5, 5, -3, 3, 0], transition: { duration: 0.4 } });
      return;
    }
    const files = takeFiles(channelId);
    setText("");
    drafts.delete(draftKey);
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
      (err: Error) => setVoiceProblem(`Your voice message didn't upload: ${err.message || "try again"}`),
    );
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing) return;
    if (commandsHere && commands.onKeyDown(e)) return;
    if (picker.onKeyDown(e)) return;
    if (sendsMessage(e, sendWith)) {
      e.preventDefault();
      send();
    } else if (e.key === "ArrowUp" && !text) {
      e.preventDefault();
      onEditLast();
    }
  }

  const ready = chosen
    ? !commands.missing.length && !commands.running
    : (!!content || staged.length > 0) && !tooLong && !cooling && !uploading && !brokenFile;

  return (
    <div className="px-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] sm:px-4">
      <AnimatePresence mode="popLayout" initial={false}>
        {gate.pending ? (
          <AgreeFirst key="rules" onRead={() => setRules(true)} />
        ) : timedOut ? (
          <TimedOut key="timed-out" left={gate.timedOutUntil - gate.now} />
        ) : thread?.locked ? (
          <ReadOnly key="locked" title="This thread is locked" about="A moderator locked it: you can read along, not reply." />
        ) : !gate.canSend || (thread && !thread.started && !gate.canStartThreads) ? (
          <ReadOnly
            key="read-only"
            title={gate.canSend ? `You can't start threads in #${channel.name}` : `You can't send messages in #${channel.name}`}
            about="Your roles let you read along here, not write."
          />
        ) : (
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
        {gate.canAttach && !chosen && <AttachButton where={where} />}
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
            if (!e.defaultPrevented) onKeyDown(e);
          }}
          onContextMenu={menu.onContextMenu}
          data-context-menu=""
          onPaste={(e) => {
            const pasted = Array.from(e.clipboardData.files);
            if (!pasted.length || !gate.canAttach) return;
            e.preventDefault();
            addFiles(instanceKey, serverId, channelId, pasted);
          }}
          placeholder={placeholder}
          aria-label={placeholder}
          className="scroll-thin max-h-[40vh] min-h-6 flex-1 resize-none bg-transparent py-1.5 text-[0.95rem] leading-6 outline-none placeholder:text-muted-foreground"
        />
        )}
        <AnimatePresence>
          {text.length > MAX - 500 && (
            <motion.span
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              className={cn("mb-2 text-xs tabular-nums", tooLong ? "font-bold text-destructive" : "text-muted-foreground")}
            >
              {MAX - text.length}
            </motion.span>
          )}
        </AnimatePresence>
        {!chosen && (
        <>
        <TimestampPicker onPick={insert} />
        <EmojiPicker
          catalog={catalog}
          closeOnPick={false}
          onPick={(emoji) => insert(emoji.text.startsWith("<") ? `:${emoji.name}:` : emoji.text)}
        >
          {(open) => (
            <motion.button
              type="button"
              aria-label="Emoji"
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
            aria-label="Make a poll"
            title="Make a poll"
            onClick={() => setPolling(true)}
            whileHover={{ scale: 1.12, y: -1 }}
            whileTap={{ scale: 0.85 }}
            className={cn("mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl text-muted-foreground transition-colors hover:text-primary", polling && "bg-primary/10 text-primary")}
          >
            <BarChart3Icon className="size-[18px]" />
          </motion.button>
        )}
        </>
        )}
        {voiceHere && gate.canAttach && !chosen && !text && !staged.length && !cooling ? (
          <VoiceRecorder
            maxMs={() => voiceLimits(instanceKey).then((l) => l.maxMs)}
            onSend={sendVoice}
            onProblem={setVoiceProblem}
          />
        ) : (
        <motion.button
          type="button"
          onClick={send}
          disabled={!ready}
          aria-label={
            chosen ? `Run /${chosen.command.name}` : cooling ? `Slow mode: send again in ${formatLeft(gate.cooldownUntil - gate.now)}` : uploading ? `Uploading files: ${Math.round(uploaded * 100)}%` : "Send"
          }
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
        )}
        </div>
      </motion.div>
          </motion.div>
        )}
      </AnimatePresence>
      {gate.canAttach && !gate.pending && !timedOut && gate.canSend && <DropOverlay where={where} channelName={channel.name} />}
      <div className="mt-1 flex items-center gap-3 px-1 text-[0.7rem] text-muted-foreground">
        {thread && gate.canSend && !thread.locked && (
          <label className="flex shrink-0 cursor-pointer items-center gap-1.5 font-bold select-none">
            <input
              type="checkbox"
              checked={alsoToChannel}
              onChange={(e) => setAlsoToChannel(e.target.checked)}
              className="size-3.5 accent-[var(--primary)]"
            />
            Also send to #{channel.name}
          </label>
        )}
        {voiceProblem ? (
          <p role="alert" className="min-w-0 flex-1 truncate font-bold text-destructive">
            {voiceProblem}
          </p>
        ) : (
        <p className={cn("hidden min-w-0 flex-1 truncate", !thread && "sm:block")}>
          {chosen ? (
            <>
              <b>{comboLabel("Enter")}</b> to run /{chosen.command.name} · <b>Esc</b> to go back to typing
            </>
          ) : (
            <>
              <b>{sendWith === "enter" ? comboLabel("Enter") : comboLabel("Mod+Enter")}</b> to send ·{" "}
              <b>{sendWith === "enter" ? comboLabel("Shift+Enter") : comboLabel("Enter")}</b> for a new line · Markdown works
              {commandsHere && " · / for commands"}
            </>
          )}
        </p>
        )}
        <AnimatePresence initial={false}>
          {(gate.slowmode > 0 || gate.exempt) && !timedOut && gate.canSend && !gate.pending && (
            <motion.p
              initial={{ opacity: 0, x: 8 }}
              animate={{ opacity: 1, x: 0 }}
              exit={{ opacity: 0, x: 8 }}
              transition={SPRING}
              className={cn("ml-auto flex shrink-0 items-center gap-1 font-bold tabular-nums", cooling && "text-amber-600 dark:text-amber-400")}
              title={gate.exempt ? "You can manage messages here, so slow mode doesn't hold you back" : undefined}
            >
              <SnailIcon className={cn("size-3.5", cooling && "animate-[crawl_1.6s_ease-in-out_infinite]")} />
              {gate.exempt
                ? `Slow mode is on for others: ${formatDuration(channel.slowmodeSeconds)}`
                : cooling
                  ? `Slow mode · send again in ${formatLeft(gate.cooldownUntil - gate.now)}`
                  : `Slow mode · one message every ${formatDuration(gate.slowmode)}`}
            </motion.p>
          )}
        </AnimatePresence>
      </div>
      {canPoll && <PollEditor open={polling} onOpenChange={setPolling} instanceKey={instanceKey} serverId={serverId} channel={channel} />}
      {server && <RulesDialog open={rules} onOpenChange={setRules} instanceKey={instanceKey} server={server} agree={gate.pending} />}
    </div>
  );
}

/** In place of the box until you agree to the server's rules: one button to read them. */
function AgreeFirst({ onRead }: { onRead: () => void }) {
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
        <p className="text-sm font-bold">Agree to the rules to start talking</p>
        <p className="text-xs text-muted-foreground">You can read along until then.</p>
      </div>
      <Button size="sm" onClick={onRead} className="btn h-9 shrink-0 rounded-xl px-4 font-bold max-sm:w-full">
        Read the rules
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
        <p className="text-sm font-bold">You're timed out</p>
        <p className="text-xs text-muted-foreground">You can still read along. Messages and edits open up again when it ends.</p>
      </div>
      <span className="shrink-0 rounded-full bg-amber-500/15 px-2.5 py-1 text-sm font-extrabold text-amber-600 tabular-nums dark:text-amber-400">
        {formatLeft(left)}
      </span>
    </motion.div>
  );
}
