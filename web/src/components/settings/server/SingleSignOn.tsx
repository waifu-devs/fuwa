import { BuildingIcon, CalendarClockIcon, CalendarDaysIcon, InfinityIcon, ShieldCheckIcon, UserCheckIcon } from "lucide-react";
import { m as motion } from "motion/react";
import { useEffect, useState } from "react";
import { SsoProtocol, type IdentityProvider, type ServerSso, type SsoIdentity } from "@/gen/fuwa/v1/sso_pb";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { getServerSso, run, startServerSso, updateServerSso } from "@/fuwa/actions";
import { useAccess, useAction } from "@/fuwa/hooks";
import { ProviderButton } from "@/components/Connect";
import { CountUp, SPRING } from "@/components/motion";
import { IdentityCard } from "@/pages/SsoDone";
import { T, useI18n } from "@/i18n/react";
import { providerReady } from "@/lib/sso";
import { Choice, SaveBar, Setting, Toggle } from "../controls";
import { fullProvider, IdentityProviderForm, providerFingerprint } from "../IdentityProviderForm";

const RECHECKS = [7, 30, 90, 0] as const;

/**
 * A server's single sign-on: the identity provider its members sign in
 * through (the same form as the instance's), whether that's required to join
 * and to stay, and how often members sign in again. Managers sign in through
 * it themselves before requiring it, so nobody locks themselves out.
 */
export function SingleSignOn({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const { t } = useI18n();
  const access = useAccess(instanceKey, server.id);
  const [saved, setSaved] = useState<ServerSso | null>(null);
  const [mine, setMine] = useState<SsoIdentity | undefined>(undefined);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [provider, setProvider] = useState<IdentityProvider>(() => fullProvider(undefined));
  const [required, setRequired] = useState(false);
  const [recheck, setRecheck] = useState(30);
  const save = useAction(updateServerSso);
  const signIn = useAction(startServerSso);

  function take(sso: ServerSso) {
    setSaved(sso);
    setProvider(fullProvider(sso.provider));
    setRequired(sso.required);
    setRecheck(sso.recheckDays);
  }

  useEffect(() => {
    run(getServerSso(instanceKey, server.id)).then(
      (r) => {
        take(r.sso!);
        setMine(r.mine);
      },
      (e) => setLoadError(e.message),
    );
  }, [instanceKey, server.id]);

  if (loadError) return <p className="text-sm text-muted-foreground first-letter:uppercase">{loadError}</p>;
  if (!saved) {
    return (
      <div className="flex flex-col gap-3">
        {[0, 1, 2].map((n) => (
          <div key={n} className="shimmer h-28 rounded-2xl" />
        ))}
      </div>
    );
  }

  const providerDirty = providerFingerprint(provider) !== providerFingerprint(saved.provider);
  const changes = [providerDirty, required !== saved.required, recheck !== saved.recheckDays].filter(Boolean).length;
  const savedReady = providerReady(saved.provider);
  const name = saved.provider?.name || t("serversettings.sso.theProvider");
  const mayRequire = saved.required || access.owner || !!mine;

  const before = saved;
  async function commit() {
    const removing = provider.protocol === SsoProtocol.UNSPECIFIED;
    const next = await save.go(instanceKey, server.id, {
      ...(providerDirty ? (removing ? { removeProvider: true } : { provider }) : {}),
      // Only what changed: a new provider stops requiring itself unless asked again.
      required: removing || required === before.required ? undefined : required,
      recheckDays: recheck === before.recheckDays ? undefined : recheck,
    });
    if (!next) return;
    take(next);
    if (providerDirty) setMine(undefined);
  }

  return (
    <div className="flex flex-col">
      <SsoSummary saved={saved} server={server} name={name} />

      <IdentityProviderForm
        value={provider}
        onChange={(fn) =>
          setProvider((p) => {
            const next = fullProvider(p);
            fn(next);
            return next;
          })
        }
        serviceProvider={saved.serviceProvider}
        offHint={t("serversettings.sso.offHint")}
      />

      {provider.protocol !== SsoProtocol.UNSPECIFIED && (
        <>
          <YourSignIn
            identity={mine}
            providerDirty={providerDirty}
            savedReady={savedReady}
            name={name}
            onSignIn={() => signIn.go(instanceKey, server.id, { next: window.location.pathname })}
            error={signIn.error}
          />
          <RequireSetting
            required={required}
            onChange={setRequired}
            locked={!savedReady || providerDirty}
            mayRequire={mayRequire}
            name={name}
          />
          <RecheckSetting recheck={recheck} onChange={setRecheck} />
        </>
      )}

      <SaveBar
        count={changes}
        saving={save.pending}
        error={save.error}
        onSave={() => void commit()}
        onDiscard={() => {
          take(saved);
          save.setError(null);
        }}
      />
    </div>
  );
}

/** The headline: whether sign-on is set up and required, and how many members have signed in. */
function SsoSummary({ saved, server, name }: { saved: ServerSso; server: Server; name: string }) {
  const { t } = useI18n();
  const savedReady = providerReady(saved.provider);
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={SPRING}
      className="relative mb-6 overflow-hidden rounded-2xl border bg-gradient-to-br from-primary/10 via-transparent to-transparent p-4"
    >
      <div className="flex items-center gap-4">
        <motion.span
          initial={{ scale: 0, rotate: -15 }}
          animate={{ scale: 1, rotate: 0 }}
          transition={{ type: "spring", stiffness: 380, damping: 18, delay: 0.1 }}
          className="grid size-12 shrink-0 place-items-center rounded-2xl bg-primary/15 text-primary"
        >
          {saved.required ? <ShieldCheckIcon className="size-6" /> : <BuildingIcon className="size-6" />}
        </motion.span>
        <div className="min-w-0 flex-1">
          <p className="font-extrabold">
            {!savedReady ? t("serversettings.sso.notSetUp") : saved.required ? t("serversettings.sso.required", { name }) : t("serversettings.sso.notRequired", { name })}
          </p>
          <p className="text-sm text-muted-foreground">
            {savedReady ? (
              <T
                k="serversettings.sso.signedIn"
                values={{
                  signedIn: (
                    <b className="text-foreground">
                      <CountUp value={Number(saved.signedInMembers)} />
                    </b>
                  ),
                  count: <CountUp value={Number(server.memberCount)} />,
                }}
                count={Number(server.memberCount)}
              />
            ) : (
              t("serversettings.sso.usualWay")
            )}
          </p>
        </div>
      </div>
      {savedReady && (
        <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-muted">
          <motion.div
            className="h-full rounded-full bg-primary"
            initial={{ x: "-100%" }}
            animate={{ x: `${Math.min(100, (Number(saved.signedInMembers) / Math.max(1, Number(server.memberCount))) * 100) - 100}%` }}
            transition={{ type: "spring", stiffness: 120, damping: 20, delay: 0.2 }}
          />
        </div>
      )}
    </motion.section>
  );
}

/** How often members sign in again. */
function RecheckSetting({ recheck, onChange }: { recheck: number; onChange: (days: number) => void }) {
  const { t } = useI18n();
  return (
    <Setting id="sso-recheck" title={t("serversettings.nav.ssoRecheck")} hint={t("serversettings.sso.recheckHint")} badge={false} delay={0.28}>
      <Choice
        value={RECHECKS.includes(recheck as (typeof RECHECKS)[number]) ? recheck : 30}
        onChange={onChange}
        options={RECHECKS.map((days) => ({
          value: days,
          label: days === 0 ? t("serversettings.shared.never") : days === 7 ? t("serversettings.sso.everyWeek") : t("serversettings.sso.everyDays", { count: days }),
          hint:
            days === 0
              ? t("serversettings.sso.onceEnough")
              : days === 7
                ? t("serversettings.sso.tightest")
                : days === 30
                  ? t("serversettings.sso.goodDefault")
                  : t("serversettings.sso.lightest"),
          icon: days === 0 ? <InfinityIcon className="size-4" /> : days === 7 ? <CalendarClockIcon className="size-4" /> : <CalendarDaysIcon className="size-4" />,
        }))}
      />
    </Setting>
  );
}

/** Your own sign-in through the saved provider, so you can require it without locking yourself out. */
function YourSignIn({
  identity,
  providerDirty,
  savedReady,
  name,
  onSignIn,
  error,
}: {
  identity: SsoIdentity | undefined;
  providerDirty: boolean;
  savedReady: boolean;
  name: string;
  onSignIn: () => Promise<unknown>;
  error: string | null;
}) {
  const { t } = useI18n();
  return (
    <Setting id="sso-yours" title={t("serversettings.sso.yours")} hint={t("serversettings.sso.yoursHint")} badge={false} delay={0.2}>
      {identity && !providerDirty ? (
        <IdentityCard identity={identity} />
      ) : (
        <p className="text-sm text-muted-foreground">{providerDirty ? t("serversettings.sso.saveFirst") : t("serversettings.sso.notYet")}</p>
      )}
      {savedReady && !providerDirty && (
        <ProviderButton
          name={name}
          label={identity ? t("serversettings.sso.signInAgain", { name }) : t("join.sso.title", { name })}
          icon={<UserCheckIcon className="size-5" />}
          onGo={onSignIn}
          error={error}
          testId="sso-server-sign-in"
        />
      )}
    </Setting>
  );
}

/** Whether members must sign in to join and stay; held back until the provider is saved and you've signed in. */
function RequireSetting({
  required,
  onChange,
  locked,
  mayRequire,
  name,
}: {
  required: boolean;
  onChange: (on: boolean) => void;
  locked: boolean;
  mayRequire: boolean;
  name: string;
}) {
  const { t } = useI18n();
  return (
    <Setting id="sso-required" title={t("serversettings.sso.requireIt")} badge={false} delay={0.24}>
      <Toggle
        checked={required}
        onChange={onChange}
        disabled={locked || !mayRequire}
        label={t("serversettings.sso.requireLabel", { name })}
        hint={
          locked
            ? t("serversettings.sso.saveProviderFirst")
            : !mayRequire
              ? t("serversettings.sso.signInYourselfFirst")
              : t("serversettings.sso.requireHint")
        }
      />
    </Setting>
  );
}
