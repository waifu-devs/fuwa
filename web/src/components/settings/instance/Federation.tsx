import { timestampDate } from "@bufbuild/protobuf/wkt";
import { BanIcon, CheckIcon, FingerprintIcon, KeyRoundIcon, LoaderCircleIcon, NetworkIcon, RadarIcon, RefreshCwIcon, ShieldAlertIcon, TriangleAlertIcon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState } from "react";
import type { CheckInstanceResponse, FederationPeer, GetFederationResponse, InstanceSettings } from "@/gen/fuwa/v1/admin_pb";
import { checkInstance, getFederation, rotateFederationKey, run } from "@/fuwa/actions";
import { Private, usePrivateField } from "@/components/Private";
import { ConfirmDialog } from "@/components/settings/server/SharedChannels";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { toast } from "@/lib/ui";
import { ago, formatBytes } from "@/lib/format";
import { type I18n, T, useI18n } from "@/i18n/react";
import { cn } from "@/lib/utils";
import { Cap, Setting, SPRING, Toggle } from "../controls";

type Reset = { changed: boolean; onReset: () => void; resetting: boolean };

/** The instance settings the Federation page reads. */
export const FEDERATION_FIELDS: { path: string; get: (s: InstanceSettings) => unknown; copy: (into: InstanceSettings, from: InstanceSettings) => void }[] = [
  { path: "federation", get: (s) => s.federation, copy: (into, from) => (into.federation = from.federation) },
  {
    path: "federation_blocked_hosts",
    get: (s) => hosts(s.federationBlockedHosts).join("\n"),
    copy: (into, from) => (into.federationBlockedHosts = [...from.federationBlockedHosts]),
  },
  {
    path: "shared_remote_sends_per_minute",
    get: (s) => s.sharedRemoteSendsPerMinute,
    copy: (into, from) => (into.sharedRemoteSendsPerMinute = from.sharedRemoteSendsPerMinute),
  },
  { path: "shared_remote_people", get: (s) => s.sharedRemotePeople, copy: (into, from) => (into.sharedRemotePeople = from.sharedRemotePeople) },
  {
    path: "shared_remote_file_bytes_per_day",
    get: (s) => s.sharedRemoteFileBytesPerDay,
    copy: (into, from) => (into.sharedRemoteFileBytesPerDay = from.sharedRemoteFileBytesPerDay),
  },
  {
    path: "shared_file_fetches_in_flight",
    get: (s) => s.sharedFileFetchesInFlight,
    copy: (into, from) => (into.sharedFileFetchesInFlight = from.sharedFileFetchesInFlight),
  },
];

/** The Other instances page in the settings menu, in the app's language. */
export const federationSection = (t: I18n["t"]) => ({
  id: "federation",
  label: t("instancesettings.nav.federation"),
  icon: NetworkIcon,
  description: t("instancesettings.nav.federationAbout"),
  keywords: "federation federate instances share channels across key fingerprint block",
  settings: [
    { id: "federation", label: t("instancesettings.nav.federationOn"), keywords: "federation on off" },
    { id: "federation-identity", label: t("instancesettings.nav.federationIdentity"), keywords: "fingerprint key address rotate replace" },
    { id: "federation-check", label: t("instancesettings.nav.federationCheck"), keywords: "test reach ping" },
    { id: "federation-peers", label: t("instancesettings.nav.federationPeers"), keywords: "pinned peers" },
    { id: "federation-blocked", label: t("instancesettings.nav.federationBlocked"), keywords: "block list deny" },
    { id: "federation-sends", label: t("instancesettings.nav.federationSends"), keywords: "limit cap rate flood shared remote" },
    { id: "federation-people", label: t("instancesettings.nav.federationPeople"), keywords: "limit cap shared remote guests" },
    { id: "federation-files", label: t("instancesettings.nav.federationFiles"), keywords: "limit cap shared remote attachments bytes" },
    { id: "federation-fetches", label: t("instancesettings.nav.federationFetches"), keywords: "limit cap shared remote attachments busy" },
  ],
});

/** One host per line, trimmed, lowercased, each once. */
const hosts = (list: string[]) => [...new Set(list.map((h) => h.trim().toLowerCase()).filter(Boolean))];
/** An origin as people read it: the host, and the port when there is one. */
const shown = (origin: string) => origin.replace(/^[a-z]+:\/\//, "");
/** How many known instances show before "Show more". */
const PAGE = 50;

/**
 * Federation: the switch, this instance's address and key fingerprint for
 * other admins to compare, a check that another instance can be reached, the
 * instances this one has pinned a key for, and the block list.
 */
export function FederationSettings({
  instanceKey,
  draft,
  saved,
  defaults,
  patch,
  resetter,
}: {
  instanceKey: string;
  draft: InstanceSettings;
  saved: InstanceSettings;
  defaults?: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  resetter: (...paths: string[]) => Reset;
}) {
  const lang = useI18n();
  const { t } = lang;
  const privateField = usePrivateField();
  const [info, setInfo] = useState<GetFederationResponse | null>(null);
  const [infoError, setInfoError] = useState<string | null>(null);
  const [blockText, setBlockText] = useState(() => saved.federationBlockedHosts.join("\n"));
  const [shownPeers, setShownPeers] = useState(PAGE);
  const [rotating, setRotating] = useState(false);
  const [reads, setReads] = useState(0);

  // Read again once a change to the switch, the address or the list is saved.
  const savedKey = `${saved.federation}|${saved.publicUrl}|${saved.federationBlockedHosts.join(",")}`;
  useEffect(() => {
    let live = true;
    run(getFederation(instanceKey)).then(
      (r) => live && (setInfo(r), setInfoError(null)),
      (e: Error) => live && setInfoError(e.message),
    );
    return () => {
      live = false;
    };
  }, [instanceKey, savedKey, reads]);

  return (
    <>
      <motion.div
        initial={{ opacity: 0, y: 8 }}
        animate={{ opacity: 1, y: 0 }}
        transition={SPRING}
        className="mb-4 flex items-start gap-3 rounded-2xl border border-sky-500/30 bg-sky-500/5 p-3 text-sm"
      >
        <motion.span
          animate={{ rotate: [0, 8, -4, 0] }}
          transition={{ duration: 1.6, repeat: Infinity, repeatDelay: 6 }}
          className="grid size-8 shrink-0 place-items-center rounded-xl bg-sky-500/15 text-sky-600 dark:text-sky-300"
        >
          <NetworkIcon className="size-4" />
        </motion.span>
        <p className="text-muted-foreground">{t("instancesettings.federation.intro")}</p>
      </motion.div>

      <Setting
        id="federation"
        title={t("instancesettings.nav.federationOn")}
        defaultLabel={t(defaults?.federation ? "instancesettings.shared.on" : "instancesettings.shared.off")}
        {...resetter("federation")}
      >
        <Toggle
          checked={draft.federation}
          onChange={(on) => patch((d) => (d.federation = on))}
          label={t("instancesettings.federation.label")}
          hint={t("instancesettings.federation.hint")}
        />
      </Setting>

      <Setting id="federation-identity" title={t("instancesettings.nav.federationIdentity")} delay={0.04} badge={false}>
        {infoError ? (
          <p className="text-xs text-muted-foreground first-letter:uppercase">{infoError}</p>
        ) : !info ? (
          <div className="shimmer h-16 rounded-xl" />
        ) : (
          <motion.div initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={SPRING} className="flex flex-col gap-3">
            {info.origin ? (
              <p className="text-sm">
                <T
                  k="instancesettings.federation.knownAs"
                  values={{
                    origin: (
                      <b className="font-mono text-xs">
                        <Private text={shown(info.origin)} />
                      </b>
                    ),
                  }}
                />
              </p>
            ) : (
              <p className="flex items-start gap-2 rounded-xl bg-amber-500/10 px-3 py-2 text-xs text-amber-700 dark:text-amber-300">
                <TriangleAlertIcon className="mt-0.5 size-3.5 shrink-0" />
                <span className="first-letter:uppercase">{info.originProblem}</span>
              </p>
            )}
            <div className="flex items-center gap-3 rounded-xl bg-muted/60 p-3">
              <FingerprintIcon className="size-5 shrink-0 text-primary" />
              <code className="grid grid-cols-4 gap-x-3 gap-y-1 font-mono text-xs tracking-wider sm:grid-cols-8">
                {info.fingerprint.split(" ").map((group, n) => (
                  <motion.span key={n} initial={{ opacity: 0, y: 4 }} animate={{ opacity: 1, y: 0 }} transition={{ ...SPRING, delay: n * 0.03 }}>
                    {group}
                  </motion.span>
                ))}
              </code>
            </div>
            <p className="text-xs text-muted-foreground">{t("instancesettings.federation.compare")}</p>
            <div className="flex flex-wrap items-center gap-3">
              <Button
                variant="outline"
                size="sm"
                className="group rounded-xl"
                disabled={!info.origin}
                onClick={() => setRotating(true)}
              >
                <RefreshCwIcon className="transition-transform duration-500 group-hover:rotate-180" /> {t("instancesettings.federation.rotate")}
              </Button>
              <span className="text-xs text-muted-foreground">
                {info.rotatedAt ? t("instancesettings.federation.lastRotated", { when: ago(lang, timestampDate(info.rotatedAt)) }) : t("instancesettings.federation.neverRotated")}
              </span>
            </div>
          </motion.div>
        )}
      </Setting>

      <ConfirmDialog
        open={rotating}
        onOpenChange={setRotating}
        title={t("instancesettings.federation.rotateAsk")}
        body={t("instancesettings.federation.rotateBody")}
        action={t("instancesettings.federation.rotateIt")}
        onConfirm={async () => {
          const r = await run(rotateFederationKey(instanceKey));
          setReads((n) => n + 1);
          toast(t("instancesettings.federation.newKey", { fingerprint: r.fingerprint.split(" ").slice(0, 2).join(" ") }));
        }}
      />

      <CheckCard instanceKey={instanceKey} enabled={saved.federation} onChecked={(r) => setInfo((i) => (i ? withPeer(i, r.peer) : i))} />

      <Setting id="federation-peers" title={t("instancesettings.nav.federationPeers")} delay={0.12} badge={false}>
        {!info || info.peers.length === 0 ? (
          <p className="text-xs text-muted-foreground">{t("instancesettings.federation.noPeers")}</p>
        ) : (
          <ul className="flex flex-col gap-2">
            <AnimatePresence initial={false}>
              {info.peers.slice(0, shownPeers).map((peer, n) => (
                <PeerRow key={peer.origin} peer={peer} delay={Math.min(n, 10) * 0.03} />
              ))}
            </AnimatePresence>
            {info.peers.length > shownPeers && (
              <Button variant="ghost" size="sm" className="self-start rounded-full" onClick={() => setShownPeers((n) => n + PAGE)}>
                {t("instancesettings.federation.showMore", { count: Math.min(PAGE, info.peers.length - shownPeers) })}
              </Button>
            )}
          </ul>
        )}
      </Setting>

      <Setting
        id="federation-blocked"
        title={t("instancesettings.nav.federationBlocked")}
        hint={t("instancesettings.federation.blockedHint")}
        defaultLabel={t("instancesettings.shared.none")}
        delay={0.16}
        {...resetter("federation_blocked_hosts")}
      >
        <Textarea
          rows={3}
          value={blockText}
          placeholder={"spam.example.com\nchat.example.org"}
          onChange={(e) => {
            setBlockText(e.target.value);
            patch((d) => (d.federationBlockedHosts = hosts(e.target.value.split("\n"))));
          }}
          className={cn("rounded-xl font-mono text-xs", privateField)}
        />
      </Setting>

      <Setting
        id="federation-sends"
        title={t("instancesettings.nav.federationSends")}
        hint={t("instancesettings.federation.sendsHint")}
        defaultLabel={
          defaults?.sharedRemoteSendsPerMinute === undefined
            ? t("instancesettings.shared.noLimit")
            : t("instancesettings.shared.perMinute", { count: Number(defaults.sharedRemoteSendsPerMinute) })
        }
        delay={0.2}
        {...resetter("shared_remote_sends_per_minute")}
      >
        <Cap label={t("instancesettings.shared.upTo")} placeholder="120" value={draft.sharedRemoteSendsPerMinute} onChange={(v) => patch((d) => (d.sharedRemoteSendsPerMinute = v))} />
      </Setting>

      <Setting
        id="federation-people"
        title={t("instancesettings.nav.federationPeople")}
        hint={t("instancesettings.federation.peopleHint")}
        defaultLabel={defaults?.sharedRemotePeople === undefined ? t("instancesettings.shared.noLimit") : lang.number(Number(defaults.sharedRemotePeople))}
        delay={0.24}
        {...resetter("shared_remote_people")}
      >
        <Cap label={t("instancesettings.shared.upTo")} placeholder="500" value={draft.sharedRemotePeople} onChange={(v) => patch((d) => (d.sharedRemotePeople = v))} />
      </Setting>

      <Setting
        id="federation-files"
        title={t("instancesettings.nav.federationFiles")}
        hint={t("instancesettings.federation.filesHint")}
        defaultLabel={
          defaults?.sharedRemoteFileBytesPerDay === undefined ? t("instancesettings.shared.noLimit") : formatBytes(lang, Number(defaults.sharedRemoteFileBytesPerDay))
        }
        delay={0.28}
        {...resetter("shared_remote_file_bytes_per_day")}
      >
        <Cap
          label={t("instancesettings.shared.upTo")}
          bytes
          value={draft.sharedRemoteFileBytesPerDay}
          onChange={(v) => patch((d) => (d.sharedRemoteFileBytesPerDay = v))}
        />
      </Setting>

      <Setting
        id="federation-fetches"
        title={t("instancesettings.nav.federationFetches")}
        hint={t("instancesettings.federation.fetchesHint")}
        defaultLabel={defaults?.sharedFileFetchesInFlight === undefined ? t("instancesettings.shared.noLimit") : lang.number(Number(defaults.sharedFileFetchesInFlight))}
        delay={0.32}
        {...resetter("shared_file_fetches_in_flight")}
      >
        <Cap
          label={t("instancesettings.shared.upTo")}
          placeholder="8"
          value={draft.sharedFileFetchesInFlight}
          onChange={(v) => patch((d) => (d.sharedFileFetchesInFlight = v))}
        />
      </Setting>
    </>
  );
}

/** The response with a just-checked instance first in the list. */
function withPeer(info: GetFederationResponse, peer: FederationPeer | undefined): GetFederationResponse {
  if (!peer) return info;
  return { ...info, peers: [peer, ...info.peers.filter((p) => p.origin !== peer.origin)] };
}

function PeerRow({ peer, delay }: { peer: FederationPeer; delay: number }) {
  const lang = useI18n();
  const { t } = lang;
  const heard = peer.lastHeard ? t("instancesettings.federation.heard", { when: ago(lang, timestampDate(peer.lastHeard)) }) : t("instancesettings.federation.heardNever");
  const lastMove = peer.moves.at(-1);
  return (
    <motion.li
      layout="position"
      initial={{ opacity: 0, y: 6 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.97 }}
      transition={{ ...SPRING, delay }}
      className={cn(
        "flex flex-col gap-1 rounded-xl border p-3 sm:flex-row sm:items-center sm:gap-3",
        peer.blocked && "border-destructive/40 bg-destructive/5",
        peer.needsCheck && !peer.blocked && "border-amber-500/40 bg-amber-500/5",
      )}
    >
      <span className="min-w-0 flex-1">
        <span className="flex items-center gap-2 text-sm font-bold">
          <Private text={shown(peer.origin)} />
          {peer.blocked && (
            <span className="flex items-center gap-1 rounded-full bg-destructive/15 px-2 py-0.5 text-[10px] font-bold text-destructive">
              <BanIcon className="size-3" /> {t("instancesettings.federation.blocked")}
            </span>
          )}
          {peer.needsCheck && (
            <motion.span
              initial={{ opacity: 0, scale: 0.9 }}
              animate={{ opacity: 1, scale: 1 }}
              transition={SPRING}
              className="flex items-center gap-1 rounded-full bg-amber-500/15 px-2 py-0.5 text-[10px] font-bold text-amber-700 dark:text-amber-300"
            >
              <ShieldAlertIcon className="size-3" /> {t("instancesettings.federation.checkAgain")}
            </motion.span>
          )}
        </span>
        <code className="block truncate font-mono text-[11px] text-muted-foreground">{peer.fingerprint}</code>
        {peer.needsCheck && (
          <p className="mt-1 text-xs text-amber-700 dark:text-amber-300">{t("instancesettings.federation.needsCheck")}</p>
        )}
        {lastMove && (
          <p className="mt-1 flex items-center gap-1 text-[11px] text-muted-foreground">
            <KeyRoundIcon className="size-3 shrink-0" />
            <span className="truncate">
              <T
                k={peer.moves.length > 1 ? "instancesettings.federation.movedMany" : "instancesettings.federation.moved"}
                values={{
                  when: lastMove.movedAt ? ago(lang, timestampDate(lastMove.movedAt)) : "",
                  fingerprint: <code className="font-mono">{lastMove.previousFingerprint.split(" ").slice(0, 2).join(" ")}…</code>,
                  count: peer.moves.length,
                }}
              />
            </span>
          </p>
        )}
      </span>
      <span className="shrink-0 text-xs text-muted-foreground">{heard}</span>
    </motion.li>
  );
}

/** Reaches another instance and back with a signed call. */
function CheckCard({ instanceKey, enabled, onChecked }: { instanceKey: string; enabled: boolean; onChecked: (r: CheckInstanceResponse) => void }) {
  const { t } = useI18n();
  const privateField = usePrivateField();
  const [address, setAddress] = useState("");
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<{ ok: CheckInstanceResponse } | { error: string } | null>(null);

  async function check() {
    setPending(true);
    setResult(null);
    try {
      const r = await run(checkInstance(instanceKey, address.trim()));
      setResult({ ok: r });
      onChecked(r);
    } catch (e) {
      setResult({ error: (e as Error).message });
    } finally {
      setPending(false);
    }
  }

  return (
    <Setting
      id="federation-check"
      title={t("instancesettings.nav.federationCheck")}
      hint={t("instancesettings.federation.checkHint")}
      delay={0.08}
      badge={false}
    >
      <form
        className="flex flex-col gap-2 sm:flex-row"
        onSubmit={(e) => {
          e.preventDefault();
          if (address.trim() && !pending) void check();
        }}
      >
        <Input
          value={address}
          onChange={(e) => setAddress(e.target.value)}
          placeholder="chat.example.com"
          disabled={!enabled}
          className={cn("rounded-xl font-mono text-xs", privateField)}
        />
        <Button type="submit" className="shrink-0 rounded-xl" disabled={!enabled || !address.trim() || pending}>
          {pending ? <LoaderCircleIcon className="size-4 animate-spin" /> : <RadarIcon className="size-4" />}
          {t("instancesettings.federation.check")}
        </Button>
      </form>
      {!enabled && <p className="text-xs text-muted-foreground">{t("instancesettings.federation.turnOnFirst", { setting: t("instancesettings.federation.label") })}</p>}
      <AnimatePresence mode="popLayout" initial={false}>
        {result && (
          <motion.div
            key={"ok" in result ? "ok" : "error"}
            initial={{ opacity: 0, y: 6, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, scale: 0.98 }}
            transition={SPRING}
            className={cn(
              "flex items-start gap-2 rounded-xl px-3 py-2 text-xs",
              "ok" in result ? "bg-emerald-500/10 text-emerald-700 dark:text-emerald-300" : "bg-destructive/10 text-destructive",
            )}
          >
            {"ok" in result ? <CheckIcon className="mt-0.5 size-3.5 shrink-0" /> : <TriangleAlertIcon className="mt-0.5 size-3.5 shrink-0" />}
            {"ok" in result ? (
              <span>
                <T
                  k="instancesettings.federation.reached"
                  values={{
                    origin: (
                      <b>
                        <Private text={shown(result.ok.peer?.origin ?? "")} />
                      </b>
                    ),
                    ms: result.ok.roundTripMs.toString(),
                    fingerprint: <code className="font-mono">{result.ok.peer?.fingerprint}</code>,
                  }}
                />{" "}
                {t(result.ok.knownThere ? "instancesettings.federation.knownThere" : "instancesettings.federation.notKnownThere")}
              </span>
            ) : (
              <span className="first-letter:uppercase">{result.error}</span>
            )}
          </motion.div>
        )}
      </AnimatePresence>
    </Setting>
  );
}
