import { AudioLinesIcon, KeyRoundIcon, RadioTowerIcon, ServerIcon } from "lucide-react";
import { motion } from "motion/react";
import type { InstanceConfig, InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import { usePrivateField } from "@/components/Private";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import { Cap, Setting, SPRING, Toggle } from "../controls";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

/** The instance settings calls read. */
export const CALL_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  { path: "calls", get: (s) => s.calls, copy: (into, from) => (into.calls = from.calls) },
  { path: "call_recordings", get: (s) => s.callRecordings, copy: (into, from) => (into.callRecordings = from.callRecordings) },
  {
    path: "call_recordings_keep_days",
    get: (s) => s.callRecordingsKeepDays,
    copy: (into, from) => (into.callRecordingsKeepDays = from.callRecordingsKeepDays),
  },
  {
    path: "ice_urls",
    get: (s) => s.iceUrls.map((u) => u.trim()).filter(Boolean).join("\n"),
    copy: (into, from) => (into.iceUrls = [...from.iceUrls]),
  },
  // The server never sends the secret back, so an empty field keeps the saved one.
  {
    path: "turn_secret",
    get: (s) => s.turnSecret.trim(),
    copy: (into, from) => {
      into.turnSecret = from.turnSecret;
      into.turnSecretSet = from.turnSecretSet;
      into.turnSecretHint = from.turnSecretHint;
    },
  },
];

export const CALL_SECTION = {
  id: "calls",
  label: "Calls",
  icon: AudioLinesIcon,
  description: "Voice channels and calls in direct messages.",
  keywords: "voice webrtc stun turn ice media",
  settings: [
    { id: "calls-on", label: "Calls", keywords: "voice enable" },
    { id: "call-recordings", label: "Recording on the server", keywords: "record recordings tracks podcast" },
    { id: "call-recordings-keep", label: "Keep recordings for", keywords: "retention expire delete days old recordings" },
    { id: "ice-urls", label: "STUN and TURN servers", keywords: "ice nat relay firewall" },
    { id: "turn-secret", label: "TURN secret", keywords: "coturn relay password" },
  ],
};

/** Calls on an instance: whether they're on, and how apps get through firewalls to its media server. */
export function CallSettings({
  config,
  draft,
  defaults,
  patch,
  resetter,
}: {
  config: InstanceConfig;
  draft: InstanceSettings;
  defaults: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  resetter: (...paths: string[]) => Reset;
}) {
  const privateField = usePrivateField();
  const startup = config.startup;
  const media = startup?.mediaPort
    ? `port ${startup.mediaPort}${startup.mediaAddresses.length ? ` on ${startup.mediaAddresses.join(", ")}` : ""}`
    : startup?.mediaAddresses.length
      ? startup.mediaAddresses.join(", ")
      : null;
  return (
    <>
      <Setting id="calls-on" title="Calls" defaultLabel={defaults.calls ? "on" : "off"} {...resetter("calls")}>
        <Toggle
          checked={draft.calls}
          onChange={(calls) => patch((d) => (d.calls = calls))}
          label="Let people talk in voice channels and call in direct messages"
          hint="Turning it off hangs up every call within a few seconds."
        />
        <motion.div
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ ...SPRING, delay: 0.1 }}
          className={cn("flex items-start gap-2.5 rounded-2xl border border-dashed p-3 text-sm", !media && "border-amber-500/50")}
        >
          <ServerIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <span className="min-w-0">
            <b>Media server: </b>
            {media ? (
              <span className={cn("break-words text-muted-foreground", privateField)}>{media}</span>
            ) : (
              <span className="text-amber-600 dark:text-amber-400">not running, so calls can't connect. Whoever runs this instance sets it up (FUWA_MEDIA_PORT, or FUWA_MEDIA_URL for a split instance).</span>
            )}
            <span className="mt-0.5 block text-xs text-muted-foreground">Set when the instance starts. Calls in direct messages are end-to-end encrypted: it only ever forwards sound it can't read.</span>
          </span>
        </motion.div>
      </Setting>
      <Setting
        id="call-recordings"
        title="Recording on the server"
        delay={0.02}
        defaultLabel={defaults.callRecordings ? "on" : "off"}
        {...resetter("call_recordings")}
      >
        <Toggle
          checked={draft.callRecordings}
          onChange={(on) => patch((d) => (d.callRecordings = on))}
          label="Let people with Record keep voice channels' recordings on this instance"
          hint="One Ogg Opus track per person, kept with the server's files (sealed when the instance encrypts its files). Nobody has Record until a server's admins grant it. Turning this off stops recordings going on now; the ones kept stay. How much each server may keep is a cap under Limits."
        />
      </Setting>
      <Setting
        id="call-recordings-keep"
        title="Keep recordings for"
        hint="Finished recordings older than this delete themselves, files and copies included. Off keeps them until someone deletes them."
        delay={0.03}
        defaultLabel={defaults.callRecordingsKeepDays === undefined ? "until deleted" : `${defaults.callRecordingsKeepDays} days`}
        {...resetter("call_recordings_keep_days")}
      >
        <Cap label="Days" value={draft.callRecordingsKeepDays} onChange={(v) => patch((d) => (d.callRecordingsKeepDays = v))} />
      </Setting>
      <Setting
        id="ice-urls"
        title="STUN and TURN servers"
        hint="For people behind strict firewalls: a TURN server relays their sound when nothing else gets through. One per line, as stun:, turn: or turns: addresses."
        delay={0.04}
        defaultLabel={defaults.iceUrls.join(", ") || "none"}
        {...resetter("ice_urls")}
      >
        <div className="relative">
          <RadioTowerIcon className="pointer-events-none absolute top-3 left-3 size-4 text-muted-foreground" />
          <Textarea
            value={draft.iceUrls.join("\n")}
            onChange={(e) => patch((d) => (d.iceUrls = e.target.value.split("\n")))}
            rows={3}
            spellCheck={false}
            placeholder={"stun:stun.example.com:3478\nturn:turn.example.com:3478?transport=udp"}
            className={cn("rounded-xl pl-9 font-mono text-sm", privateField)}
          />
        </div>
      </Setting>
      <Setting
        id="turn-secret"
        title="TURN secret"
        hint="The shared secret your TURN server (such as coturn with use-auth-secret) checks. Apps get a new password from it for each call, good for an hour, that names no account."
        delay={0.08}
        defaultLabel={defaults.turnSecretSet ? "set" : "none"}
        {...resetter("turn_secret")}
      >
        <div className="relative">
          <KeyRoundIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="password"
            autoComplete="off"
            value={draft.turnSecret}
            onChange={(e) => patch((d) => (d.turnSecret = e.target.value))}
            placeholder={
              draft.turnSecretSet
                ? draft.turnSecretHint
                  ? `Saved, ending in ${draft.turnSecretHint}. Type to replace it`
                  : "Saved. Type to replace it"
                : "No TURN secret"
            }
            className="h-10 rounded-xl pl-9"
          />
        </div>
      </Setting>
    </>
  );
}
