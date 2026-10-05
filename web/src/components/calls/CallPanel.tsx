import { Link } from "@tanstack/react-router";
import { LockKeyholeIcon, TriangleAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { hangUp } from "@/calls/engine";
import { useQualityLevel } from "@/calls/quality";
import { useCalls, type ActiveCall } from "@/calls/state";
import { useFuwa } from "@/fuwa/store";
import { SPRING, SwapText } from "@/components/motion";
import { displayName } from "@/lib/format";
import { useNow } from "@/lib/notifications";
import { cn } from "@/lib/utils";
import { type Key, useI18n } from "@/i18n/react";
import { ConnectionDetails, PingText, Signal } from "./Connection";
import { HangUpButton } from "./parts";
import { CameraButton, RecordButton, ScreenButton } from "./Video";

/** How long a call has gone on: 4:07, or 1:02:33. */
export function clock(seconds: number) {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = String(seconds % 60).padStart(2, "0");
  return h ? `${h}:${String(m).padStart(2, "0")}:${s}` : `${m}:${s}`;
}

const STATUS: Record<ActiveCall["status"], Key> = {
  connecting: "dms-calls.calls.status.connecting",
  connected: "dms-calls.calls.status.connected",
  reconnecting: "dms-calls.calls.status.reconnecting",
};

/** Where you are in a call, above your user panel, with a way out. Shows wherever you go in the app. */
export function CallPanel() {
  const call = useCalls((s) => s.call);
  return (
    <AnimatePresence initial={false}>
      {call && (
        <motion.div
          key="call"
          initial={{ height: 0, opacity: 0 }}
          animate={{ height: "auto", opacity: 1 }}
          exit={{ height: 0, opacity: 0 }}
          transition={SPRING}
          className="shrink-0 overflow-hidden border-t bg-[color-mix(in_srgb,var(--background)_60%,transparent)]"
        >
          <CallPanelBody call={call} />
        </motion.div>
      )}
    </AnimatePresence>
  );
}

function CallPanelBody({ call }: { call: ActiveCall }) {
  const t = call.target;
  const now = useNow(1_000);
  const level = useQualityLevel();
  const { t: tr } = useI18n();
  const where = useFuwa((s) => {
    const inst = s.instances[t.instance];
    if (t.kind === "voice") {
      const channel = inst?.channels[t.serverId]?.find((c) => c.id === t.channelId)?.name ?? tr("dms-calls.calls.panel.voice");
      const server = inst?.servers.find((x) => x.id === t.serverId)?.name ?? "";
      return server ? tr("dms-calls.calls.panel.where", { channel, server }) : channel;
    }
    const conversation = inst?.dms.conversations.find((c) => c.id === t.conversationId);
    const other = conversation?.users.find((u) => u.id !== inst?.me?.id);
    return displayName(other);
  });
  const seconds = call.since ? Math.max(0, Math.floor((now - call.since) / 1000)) : 0;
  const link =
    t.kind === "voice"
      ? ({ to: "/$instance/$server/$channel", params: { instance: t.instance, server: t.serverId, channel: t.channelId } } as const)
      : ({ to: "/$instance/dm/$conversation", params: { instance: t.instance, conversation: t.conversationId } } as const);
  return (
    <div className="flex flex-col gap-1 p-2 pb-1.5">
      <div className="flex items-center gap-2">
        <div className="min-w-0 flex-1 px-1.5">
          <ConnectionDetails status={call.status}>
            <button
              type="button"
              title={tr("dms-calls.calls.panel.details")}
              className={cn(
                "-mx-1 flex w-[calc(100%+0.5rem)] items-center gap-1.5 rounded-md px-1 text-left text-sm font-extrabold transition hover:bg-muted/60 active:scale-[0.98]",
                call.status === "connected" ? "text-[#3ba55d]" : call.status === "reconnecting" ? "text-amber-500" : "text-muted-foreground",
              )}
            >
              <Signal status={call.status} level={level} />
              <SwapText className="truncate">{tr(STATUS[call.status])}</SwapText>
              {call.status === "connected" && <PingText colored className="ml-auto shrink-0 text-xs font-bold text-muted-foreground" />}
            </button>
          </ConnectionDetails>
          <Link {...link} className="flex min-w-0 items-center gap-1 text-xs text-muted-foreground transition hover:text-foreground hover:underline">
            {t.kind === "dm" && (
              <span title={tr("dms-calls.calls.panel.encrypted")} className="shrink-0">
                <LockKeyholeIcon className="size-3" />
              </span>
            )}
            <span className="truncate">{where}</span>
            {call.status === "connected" && <span className="ml-auto shrink-0 pl-2 tabular-nums">{clock(seconds)}</span>}
          </Link>
        </div>
        <HangUpButton onClick={() => void hangUp(null)} />
      </div>
      <div className="flex gap-1.5">
        <CameraButton className="h-8 flex-1" />
        <ScreenButton className="h-8 flex-1" />
        <RecordButton className="h-8 flex-1" />
      </div>
      <AnimatePresence initial={false}>
        {call.problem && (
          <motion.p
            initial={{ opacity: 0, height: 0 }}
            animate={{ opacity: 1, height: "auto" }}
            exit={{ opacity: 0, height: 0 }}
            className="flex items-start gap-1.5 overflow-hidden px-1.5 text-xs text-amber-600 dark:text-amber-400"
          >
            <TriangleAlertIcon className="mt-0.5 size-3 shrink-0" /> {call.problem}
          </motion.p>
        )}
      </AnimatePresence>
    </div>
  );
}
