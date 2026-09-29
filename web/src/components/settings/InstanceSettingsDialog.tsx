import { clone, create } from "@bufbuild/protobuf";
import {
  CrownIcon,
  DoorClosedIcon,
  DoorOpenIcon,
  GlobeIcon,
  LinkIcon,
  LockIcon,
  ShieldCheckIcon,
  UsersIcon,
  BanIcon,
  GaugeIcon,
  SlidersHorizontalIcon,
} from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import {
  InstanceSettingsSchema,
  LocalAccounts,
  type InstanceConfig,
  type InstanceSettings,
} from "@/gen/fuwa/v1/admin_pb";
import { ServerCreation, ServerLimitsSchema } from "@/gen/fuwa/v1/types_pb";
import { getSettings, run, updateSettings } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { formatBytes } from "@/lib/format";
import { cn } from "@/lib/utils";
import { Cap, Choice, SaveBar, Setting, SPRING, Toggle } from "./controls";
import { SettingsScreen } from "./SettingsScreen";

/** Every setting, as the API names it, and how to read it for comparing. */
const FIELDS: { path: string; get: (s: InstanceSettings) => unknown }[] = [
  { path: "name", get: (s) => s.name.trim() },
  { path: "public_url", get: (s) => s.publicUrl.trim() },
  { path: "allowed_origins", get: (s) => s.allowedOrigins.map((o) => o.trim()).filter(Boolean).join("\n") },
  { path: "local_accounts", get: (s) => s.localAccounts },
  { path: "server_creation", get: (s) => s.serverCreation },
  { path: "servers_per_account", get: (s) => s.serversPerAccount },
  { path: "default_limits.members", get: (s) => s.defaultLimits?.members },
  { path: "default_limits.channels", get: (s) => s.defaultLimits?.channels },
  { path: "default_limits.storage_bytes", get: (s) => s.defaultLimits?.storageBytes },
  { path: "default_limits.attachment_bytes", get: (s) => s.defaultLimits?.attachmentBytes },
  { path: "telemetry", get: (s) => s.telemetry },
  { path: "web", get: (s) => s.web },
];

const changedPaths = (draft: InstanceSettings, saved: InstanceSettings) =>
  FIELDS.filter((f) => f.get(draft) !== f.get(saved)).map((f) => f.path);

const count = (n: bigint | undefined) => (n === undefined ? "no limit" : Number(n).toLocaleString());
const size = (n: bigint | undefined) => (n === undefined ? "no limit" : formatBytes(Number(n)));

/**
 * Everything an admin can change about a fuwa instance while it runs. Each
 * setting starts from the operator's environment; changes are stored on the
 * instance, and Reset puts one back.
 */
export function InstanceSettingsDialog({
  open,
  onOpenChange,
  instanceKey,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  instanceKey: string;
}) {
  const inst = useInstance(instanceKey);
  const [config, setConfig] = useState<InstanceConfig | null>(null);
  const [draft, setDraft] = useState<InstanceSettings | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tab, setTab] = useState("general");
  const save = useAction(updateSettings);

  useEffect(() => {
    if (!open) return;
    setConfig(null);
    setLoadError(null);
    setTab("general");
    run(getSettings(instanceKey)).then(
      (c) => {
        setConfig(c);
        setDraft(clone(InstanceSettingsSchema, c.settings!));
      },
      (e) => setLoadError(e.message),
    );
  }, [open, instanceKey]);

  const saved = config?.settings;
  const changed = saved && draft ? changedPaths(draft, saved) : [];
  const overridden = new Set(config?.overridden ?? []);
  const defaults = config?.defaults;

  function patch(fn: (d: InstanceSettings) => void) {
    setDraft((d) => {
      if (!d) return d;
      const next = clone(InstanceSettingsSchema, d);
      next.defaultLimits ??= create(ServerLimitsSchema);
      fn(next);
      return next;
    });
    save.setError(null);
  }

  async function commit(update: string[], reset: string[]) {
    if (!draft) return;
    const next = await save.go(instanceKey, draft, update, reset);
    if (!next) return;
    setConfig(next);
    // Keep unsaved edits to other settings; take the stored value for the rest.
    setDraft((d) => {
      const fresh = clone(InstanceSettingsSchema, next.settings!);
      if (!d || !saved) return fresh;
      const pending = changedPaths(d, saved).filter((p) => !update.includes(p) && !reset.includes(p));
      return pending.length ? mergeFields(fresh, d, pending) : fresh;
    });
  }

  const resetter = (...paths: string[]) => ({
    changed: paths.some((p) => overridden.has(p)),
    onReset: () => commit([], paths),
    resetting: save.pending,
  });

  const name = inst?.node?.name ?? instanceKey;
  const loading = !config || !draft || !defaults;

  return (
    <SettingsScreen
      open={open}
      onOpenChange={onOpenChange}
      title={name}
      subtitle="Instance settings"
      section={tab}
      onSectionChange={setTab}
      groups={[
        {
          label: "Instance",
          sections: [
            { id: "general", label: "General", icon: SlidersHorizontalIcon, description: `What everyone on ${name} gets. Changes apply right away.` },
            { id: "accounts", label: "Accounts", icon: UsersIcon, description: "Who can join this instance and what they can make." },
            { id: "limits", label: "Limits", icon: GaugeIcon, description: "Caps every server starts with." },
            { id: "privacy", label: "Privacy", icon: ShieldCheckIcon, description: "What this instance tells Waifu Devs." },
          ],
        },
      ]}
      footer={
        <SaveBar
          scope="screen"
          count={changed.length}
          saving={save.pending}
          error={save.error}
          onSave={() => commit(changed, [])}
          onDiscard={() => {
            if (saved) setDraft(clone(InstanceSettingsSchema, saved));
            save.setError(null);
          }}
        />
      }
    >
      {loadError ? (
        <p className="text-sm text-muted-foreground first-letter:uppercase">{loadError}</p>
      ) : loading ? (
        <div className="flex flex-col gap-3">
          {[0, 1, 2].map((n) => (
            <div key={n} className="shimmer h-28 rounded-2xl" />
          ))}
        </div>
      ) : (
        <div className="flex flex-col gap-3">
          {tab === "general" && (
            <>
              <Setting title="Name" hint="Shown in the app and when people add this instance." defaultLabel={defaults.name} {...resetter("name")}>
                <Input value={draft.name} maxLength={64} onChange={(e) => patch((d) => (d.name = e.target.value))} className="h-10 rounded-xl" />
              </Setting>
              <Setting
                title="Public address"
                hint="The URL people use to reach this instance."
                defaultLabel={defaults.publicUrl}
                delay={0.04}
                {...resetter("public_url")}
              >
                <div className="relative">
                  <LinkIcon className="pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2 text-muted-foreground" />
                  <Input
                    value={draft.publicUrl}
                    type="url"
                    onChange={(e) => patch((d) => (d.publicUrl = e.target.value))}
                    className="h-10 rounded-xl pl-9"
                  />
                </div>
              </Setting>
              <Setting title="Web app" delay={0.08} defaultLabel={defaults.web ? "on" : "off"} {...resetter("web")}>
                <Toggle
                  checked={draft.web}
                  disabled={!config.startup?.webBuiltIn}
                  onChange={(web) => patch((d) => (d.web = web))}
                  label="Open the app at this address"
                  hint={
                    config.startup?.webBuiltIn
                      ? "Turned off, people can still use this instance from the app on another fuwa instance."
                      : "This build of fuwa doesn't include the web app."
                  }
                />
              </Setting>
              <Origins
                draft={draft}
                patch={patch}
                defaultLabel={defaults.allowedOrigins.join(", ")}
                reset={resetter("allowed_origins")}
              />
              <Startup config={config} />
            </>
          )}
          {tab === "accounts" && (
            <>
              <Setting
                title="Standalone accounts"
                hint="A username and password kept on this instance only."
                defaultLabel={LOCAL_LABEL[defaults.localAccounts]}
                {...resetter("local_accounts")}
              >
                <Choice
                  value={draft.localAccounts}
                  onChange={(v) => patch((d) => (d.localAccounts = v))}
                  options={[
                    { value: LocalAccounts.OPEN, label: "Open", hint: "Anyone can sign up.", icon: <DoorOpenIcon className="size-4" /> },
                    { value: LocalAccounts.CLOSED, label: "Closed", hint: "Existing accounts only.", icon: <DoorClosedIcon className="size-4" /> },
                    {
                      value: LocalAccounts.OFF,
                      label: "Off",
                      hint: "No standalone accounts.",
                      icon: <LockIcon className="size-4" />,
                      disabled: draft.localAccounts === LocalAccounts.OFF ? undefined : "Needs another way to sign in first.",
                    },
                  ]}
                />
              </Setting>
              <Setting
                title="Who can create servers"
                defaultLabel={CREATION_LABEL[defaults.serverCreation]}
                delay={0.04}
                {...resetter("server_creation")}
              >
                <Choice
                  value={draft.serverCreation}
                  onChange={(v) => patch((d) => (d.serverCreation = v))}
                  options={[
                    { value: ServerCreation.EVERYONE, label: "Everyone", hint: "Any signed-in account.", icon: <UsersIcon className="size-4" /> },
                    { value: ServerCreation.ADMINS, label: "Admins", hint: "Instance admins only.", icon: <CrownIcon className="size-4" /> },
                    { value: ServerCreation.DISABLED, label: "Nobody", hint: "No new servers.", icon: <BanIcon className="size-4" /> },
                  ]}
                />
              </Setting>
              <Setting
                title="Servers per account"
                hint="How many servers one account may own."
                defaultLabel={count(defaults.serversPerAccount)}
                delay={0.08}
                {...resetter("servers_per_account")}
              >
                <Cap label="Up to" value={draft.serversPerAccount} onChange={(v) => patch((d) => (d.serversPerAccount = v))} />
              </Setting>
            </>
          )}
          {tab === "limits" && (
            <>
              <Setting
                title="Default caps for every server"
                hint="A server can get its own caps from its settings. With a cap off, it's unlimited."
                defaultLabel={[
                  `${count(defaults.defaultLimits?.members)} members`,
                  `${count(defaults.defaultLimits?.channels)} channels`,
                  `${size(defaults.defaultLimits?.storageBytes)} storage`,
                  `${size(defaults.defaultLimits?.attachmentBytes)} files`,
                ].join(", ")}
                {...resetter("default_limits.members", "default_limits.channels", "default_limits.storage_bytes", "default_limits.attachment_bytes")}
              >
                <div className="flex flex-col gap-3">
                  <Cap label="Members" value={draft.defaultLimits?.members} onChange={(v) => patch((d) => (d.defaultLimits!.members = v))} />
                  <Cap label="Channels" value={draft.defaultLimits?.channels} onChange={(v) => patch((d) => (d.defaultLimits!.channels = v))} />
                  <Cap label="Storage" bytes value={draft.defaultLimits?.storageBytes} onChange={(v) => patch((d) => (d.defaultLimits!.storageBytes = v))} />
                  <Cap label="Files" bytes value={draft.defaultLimits?.attachmentBytes} onChange={(v) => patch((d) => (d.defaultLimits!.attachmentBytes = v))} />
                </div>
              </Setting>
            </>
          )}
          {tab === "privacy" && (
            <>
              <Setting title="Anonymous usage signal" defaultLabel={defaults.telemetry ? "on" : "off"} {...resetter("telemetry")}>
                <Toggle
                  checked={draft.telemetry}
                  onChange={(telemetry) => patch((d) => (d.telemetry = telemetry))}
                  label="Send it once a day"
                  hint="Helps Waifu Devs see how fuwa is used. Counts only: no names, messages, ids or addresses."
                />
                <ul className="grid gap-1.5 text-xs text-muted-foreground sm:grid-cols-2">
                  {["How many accounts, servers, channels and messages", "Storage used, in bytes", "Which account and server options are on", "fuwa version, OS and a random install id"].map(
                    (line, n) => (
                      <motion.li
                        key={line}
                        initial={{ opacity: 0, x: -8 }}
                        animate={{ opacity: 1, x: 0 }}
                        transition={{ ...SPRING, delay: 0.1 + n * 0.05 }}
                        className="flex items-center gap-2"
                      >
                        <ShieldCheckIcon className="size-3.5 shrink-0 text-primary" /> {line}
                      </motion.li>
                    ),
                  )}
                </ul>
                <a
                  href="https://github.com/waifu-devs/fuwa#the-anonymous-usage-signal"
                  target="_blank"
                  rel="noreferrer"
                  className="text-xs font-bold text-primary underline-offset-4 hover:underline"
                >
                  Every field it sends
                </a>
              </Setting>
            </>
          )}
        </div>
      )}
    </SettingsScreen>
  );
}

const LOCAL_LABEL: Record<number, string> = {
  [LocalAccounts.OPEN]: "open",
  [LocalAccounts.CLOSED]: "closed",
  [LocalAccounts.OFF]: "off",
};

const CREATION_LABEL: Record<number, string> = {
  [ServerCreation.EVERYONE]: "everyone",
  [ServerCreation.ADMINS]: "admins",
  [ServerCreation.DISABLED]: "nobody",
};

/** Copies the named settings from one draft into another. */
function mergeFields(into: InstanceSettings, from: InstanceSettings, paths: string[]) {
  for (const path of paths) {
    switch (path) {
      case "name":
        into.name = from.name;
        break;
      case "public_url":
        into.publicUrl = from.publicUrl;
        break;
      case "allowed_origins":
        into.allowedOrigins = [...from.allowedOrigins];
        break;
      case "local_accounts":
        into.localAccounts = from.localAccounts;
        break;
      case "server_creation":
        into.serverCreation = from.serverCreation;
        break;
      case "servers_per_account":
        into.serversPerAccount = from.serversPerAccount;
        break;
      case "telemetry":
        into.telemetry = from.telemetry;
        break;
      case "web":
        into.web = from.web;
        break;
      default: {
        const key = path.replace("default_limits.", "") as "members" | "channels" | "storage_bytes" | "attachment_bytes";
        const field = key === "storage_bytes" ? "storageBytes" : key === "attachment_bytes" ? "attachmentBytes" : key;
        into.defaultLimits ??= create(ServerLimitsSchema);
        into.defaultLimits[field] = from.defaultLimits?.[field];
      }
    }
  }
  return into;
}

/** Which sites' pages may call this instance: any (so every fuwa app works), or a list. */
function Origins({
  draft,
  patch,
  defaultLabel,
  reset,
}: {
  draft: InstanceSettings;
  patch: (fn: (d: InstanceSettings) => void) => void;
  defaultLabel: string;
  reset: { changed: boolean; onReset: () => void; resetting: boolean };
}) {
  const any = draft.allowedOrigins.includes("*");
  const [list, setList] = useState(any ? "" : draft.allowedOrigins.join("\n"));
  useEffect(() => {
    if (!any) setList((current) => (current.split("\n").map((o) => o.trim()).filter(Boolean).join("\n") === draft.allowedOrigins.join("\n") ? current : draft.allowedOrigins.join("\n")));
  }, [any, draft.allowedOrigins]);
  return (
    <Setting
      title="Sites that can connect"
      hint="Web pages on other sites, such as the fuwa app on another instance, that may use this one from a browser."
      defaultLabel={defaultLabel === "*" ? "any site" : defaultLabel}
      delay={0.12}
      {...reset}
    >
      <Choice
        value={any ? "any" : "list"}
        onChange={(v) =>
          patch((d) => {
            d.allowedOrigins = v === "any" ? ["*"] : list.split("\n").map((o) => o.trim()).filter(Boolean);
            if (v === "list" && d.allowedOrigins.length === 0) d.allowedOrigins = [window.location.origin];
          })
        }
        options={[
          { value: "any", label: "Any site", hint: "Every fuwa app can connect.", icon: <GlobeIcon className="size-4" /> },
          { value: "list", label: "Only these", hint: "Other apps are blocked.", icon: <LockIcon className="size-4" /> },
        ]}
      />
      <motion.div initial={false} animate={{ height: any ? 0 : "auto", opacity: any ? 0 : 1 }} transition={SPRING} className="overflow-hidden">
        <Textarea
          rows={3}
          value={list}
          placeholder={"https://fuwa.waifu.dev\nhttps://chat.example.com"}
          onChange={(e) => {
            setList(e.target.value);
            patch((d) => (d.allowedOrigins = e.target.value.split("\n").map((o) => o.trim()).filter(Boolean)));
          }}
          className="rounded-xl font-mono text-xs"
        />
        <p className="mt-1.5 text-xs text-muted-foreground">One per line, like https://chat.example.com. This page ({window.location.origin}) always works when served by this instance.</p>
      </motion.div>
    </Setting>
  );
}

/** How the process was started: read-only, set by whoever runs the instance. */
function Startup({ config }: { config: InstanceConfig }) {
  const s = config.startup;
  if (!s) return null;
  const facts: { label: string; value: ReactNode; good?: boolean }[] = [
    { label: "Version", value: s.version },
    { label: "Port", value: s.port },
    { label: "Encryption at rest", value: s.encryption ? "on" : "off", good: s.encryption },
    { label: "Admin token", value: s.adminToken ? "set" : "not set" },
    { label: "Hosting", value: s.hosted ? "Waifu Devs" : "self-hosted" },
  ];
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...SPRING, delay: 0.16 }}
      className="rounded-2xl border border-dashed p-4"
    >
      <h3 className="text-sm font-extrabold">Set when the instance starts</h3>
      <p className="mt-0.5 text-xs text-muted-foreground">Changed only by whoever runs it, through FUWA_* environment variables.</p>
      <div className="mt-3 flex flex-wrap gap-2">
        {facts.map((f, n) => (
          <motion.span
            key={f.label}
            initial={{ opacity: 0, scale: 0.9 }}
            animate={{ opacity: 1, scale: 1 }}
            transition={{ ...SPRING, delay: 0.2 + n * 0.04 }}
            className="rounded-full border bg-muted/50 px-3 py-1 text-xs"
          >
            <span className="text-muted-foreground">{f.label}</span>{" "}
            <b className={cn(f.good && "text-primary")}>{f.value}</b>
          </motion.span>
        ))}
      </div>
    </motion.section>
  );
}

