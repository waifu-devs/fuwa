import { SendHorizontalIcon } from "lucide-react";
import { AnimatePresence, motion, useAnimationControls } from "motion/react";
import { useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { run, sendMessage } from "@/fuwa/actions";
import { comboLabel, isMac } from "@/lib/keybinds";
import { usePrefs, type SendWith } from "@/lib/prefs";
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
 * Where you type. Enter sends (or Ctrl+Enter, by the Chat setting), the other
 * adds a line, Up in an empty box edits your last message. Drafts survive
 * switching channels.
 */
export function Composer({
  instanceKey,
  serverId,
  channelId,
  placeholder,
  onEditLast,
}: {
  instanceKey: string;
  serverId: string;
  channelId: string;
  placeholder: string;
  onEditLast: () => void;
}) {
  const [text, setText] = useState(() => drafts.get(channelId) ?? "");
  const box = useRef<HTMLTextAreaElement>(null);
  const plane = useAnimationControls();
  const sendWith = usePrefs((p) => p.sendWith);

  useEffect(() => {
    setText(drafts.get(channelId) ?? "");
    if (window.matchMedia("(pointer: fine)").matches) box.current?.focus();
  }, [channelId]);
  useEffect(() => {
    drafts.set(channelId, text);
  }, [channelId, text]);

  // Grow with the text, up to a point.
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    el.style.height = "0px";
    el.style.height = `${Math.min(el.scrollHeight, window.innerHeight * 0.4)}px`;
  }, [text]);

  const content = text.trim();
  const tooLong = text.length > MAX;

  function send() {
    if (!content || tooLong) return;
    setText("");
    drafts.delete(channelId);
    void plane.start({
      x: [0, 28, -18, 0],
      y: [0, -14, 8, 0],
      opacity: [1, 0, 0, 1],
      rotate: [0, -20, 0, 0],
      transition: { duration: 0.55, times: [0, 0.45, 0.5, 1], ease: "easeOut" },
    });
    run(sendMessage(instanceKey, serverId, channelId, content)).catch(() => {
      // The message stays in the list, marked as failed, with a retry.
    });
  }

  function onKeyDown(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.nativeEvent.isComposing) return;
    if (sendsMessage(e, sendWith)) {
      e.preventDefault();
      send();
    } else if (e.key === "ArrowUp" && !text) {
      e.preventDefault();
      onEditLast();
    }
  }

  return (
    <div className="px-3 pb-[max(0.75rem,env(safe-area-inset-bottom))] sm:px-4">
      <div className="composer flex items-end gap-2 rounded-2xl border bg-card px-3 py-2">
        <textarea
          ref={box}
          data-composer
          rows={1}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={placeholder}
          aria-label={placeholder}
          className="scroll-thin max-h-[40vh] min-h-6 flex-1 resize-none bg-transparent py-1.5 text-[0.95rem] leading-6 outline-none placeholder:text-muted-foreground"
        />
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
        <motion.button
          type="button"
          onClick={send}
          disabled={!content || tooLong}
          aria-label="Send"
          whileTap={{ scale: 0.85 }}
          initial={false}
          animate={{ scale: content && !tooLong ? 1 : 0.9 }}
          transition={{ type: "spring", stiffness: 600, damping: 20 }}
          className={cn(
            "mb-0.5 grid size-9 shrink-0 place-items-center rounded-xl transition-colors",
            content && !tooLong ? "bg-primary text-primary-foreground shadow-[0_6px_18px_-8px_var(--primary)]" : "text-muted-foreground",
          )}
        >
          <motion.span animate={plane}>
            <SendHorizontalIcon className="size-[18px]" />
          </motion.span>
        </motion.button>
      </div>
      <p className="mt-1 hidden px-1 text-[0.7rem] text-muted-foreground sm:block">
        <b>{sendWith === "enter" ? comboLabel("Enter") : comboLabel("Mod+Enter")}</b> to send ·{" "}
        <b>{sendWith === "enter" ? comboLabel("Shift+Enter") : comboLabel("Enter")}</b> for a new line · Markdown works
      </p>
    </div>
  );
}
