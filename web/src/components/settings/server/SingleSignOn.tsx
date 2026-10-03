import { BuildingIcon, CalendarClockIcon, CalendarDaysIcon, InfinityIcon, ShieldCheckIcon, UserCheckIcon } from "lucide-react";
import { motion } from "motion/react";
import { useEffect, useState } from "react";
import { SsoProtocol, type IdentityProvider, type ServerSso, type SsoIdentity } from "@/gen/fuwa/v1/sso_pb";
import type { Server } from "@/gen/fuwa/v1/types_pb";
import { getServerSso, run, startServerSso, updateServerSso } from "@/fuwa/actions";
import { useAccess, useAction } from "@/fuwa/hooks";
import { ProviderButton } from "@/components/Connect";
import { CountUp, SPRING } from "@/components/motion";
import { IdentityCard } from "@/pages/SsoDone";
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
  const access = useAccess(instanceKey, server.id);
  const [saved, setSaved] = useState<ServerSso | null>(null);
  const [mine, setMine] = useState<SsoIdentity | undefined>(undefined);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [provider, setProvider] = useState<IdentityProvider>(fullProvider(undefined));
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
  const name = saved.provider?.name || "the provider";
  const mayRequire = access.owner || !!mine;

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
              {!savedReady ? "Not set up" : saved.required ? `Members sign in with ${name}` : `${name} is set up, not required`}
            </p>
            <p className="text-sm text-muted-foreground">
              {savedReady ? (
                <>
                  <b className="text-foreground">
                    <CountUp value={Number(saved.signedInMembers)} />
                  </b>{" "}
                  of <CountUp value={Number(server.memberCount)} /> members signed in
                </>
              ) : (
                "Anyone who can join gets in the usual way."
              )}
            </p>
          </div>
        </div>
        {savedReady && (
          <div className="mt-3 h-1.5 overflow-hidden rounded-full bg-muted">
            <motion.div
              className="h-full rounded-full bg-primary"
              initial={{ width: 0 }}
              animate={{ width: `${Math.min(100, (Number(saved.signedInMembers) / Math.max(1, Number(server.memberCount))) * 100)}%` }}
              transition={{ type: "spring", stiffness: 120, damping: 20, delay: 0.2 }}
            />
          </div>
        )}
      </motion.section>

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
        offHint="Join the usual way."
      />

      {provider.protocol !== SsoProtocol.UNSPECIFIED && (
        <>
          <Setting id="sso-yours" title="Your sign-in" hint="Sign in through the provider to see that it works. It's also how you show you can before requiring it." badge={false} delay={0.2}>
            {mine && !providerDirty ? (
              <IdentityCard identity={mine} />
            ) : (
              <p className="text-sm text-muted-foreground">{providerDirty ? "Save first; signing in uses the saved provider." : "You haven't signed in through it yet."}</p>
            )}
            {savedReady && !providerDirty && (
              <ProviderButton
                name={name}
                label={mine ? `Sign in with ${name} again` : `Sign in with ${name}`}
                icon={<UserCheckIcon className="size-5" />}
                onGo={() => signIn.go(instanceKey, server.id, { next: window.location.pathname })}
                error={signIn.error}
                testId="sso-server-sign-in"
              />
            )}
          </Setting>
          <Setting id="sso-required" title="Require it" badge={false} delay={0.24}>
            <Toggle
              checked={required}
              onChange={setRequired}
              disabled={!savedReady || providerDirty || (!saved.required && !mayRequire)}
              label={`Members sign in with ${name} to join and to stay`}
              hint={
                !savedReady || providerDirty
                  ? "Save the provider first."
                  : !saved.required && !mayRequire
                    ? "Sign in through it yourself first, so you don't lock yourself out."
                    : "Members who haven't still belong, but see no channels until they sign in. The owner and agents never need to."
              }
            />
          </Setting>
          <Setting id="sso-recheck" title="Sign in again" hint="How long a sign-in lasts. When it runs out, members sign in again to keep seeing the server." badge={false} delay={0.28}>
            <Choice
              value={RECHECKS.includes(recheck as (typeof RECHECKS)[number]) ? recheck : 30}
              onChange={setRecheck}
              options={RECHECKS.map((days) => ({
                value: days,
                label: days === 0 ? "Never" : days === 7 ? "Every week" : `Every ${days} days`,
                hint: days === 0 ? "Once is enough." : days === 7 ? "Tightest." : days === 30 ? "A good default." : "Lightest.",
                icon: days === 0 ? <InfinityIcon className="size-4" /> : days === 7 ? <CalendarClockIcon className="size-4" /> : <CalendarDaysIcon className="size-4" />,
              }))}
            />
          </Setting>
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
