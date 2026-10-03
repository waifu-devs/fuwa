import { BuildingIcon, ChevronLeftIcon, LockKeyholeIcon, GlobeIcon } from "lucide-react";
import { motion, useReducedMotion } from "motion/react";
import { AccountKind, type Server } from "@/gen/fuwa/v1/types_pb";
import { startServerSso } from "@/fuwa/actions";
import { useAction, useInstance } from "@/fuwa/hooks";
import { ProviderButton } from "@/components/Connect";
import { useLayout } from "@/components/Shell";
import { ssoLocked } from "@/lib/sso";

/** Whether you're locked out of a server until you sign in through its provider. */
export function useSsoLocked(instanceKey: string, serverId: string) {
  const inst = useInstance(instanceKey);
  const server = inst?.servers.find((s) => s.id === serverId);
  const me = inst?.members[serverId]?.find((m) => m.user?.id === inst?.me?.id);
  return ssoLocked(server, me, AccountKind.AGENT);
}

/**
 * Where a server's channels were, for a member whose single sign-on is
 * missing or ran out: a padlock swinging on its chain and the way back in.
 * They stay a member; the instance shows the channels again once they sign in.
 */
export function SsoGate({ instanceKey, server }: { instanceKey: string; server: Server }) {
  const start = useAction(startServerSso);
  const reduce = useReducedMotion();
  const { compact, setNavOpen } = useLayout();
  const name = server.ssoName || "your organization";
  const days = server.ssoRecheckDays;
  return (
    <div className="relative grid h-full place-items-center overflow-hidden p-6 text-center" data-testid="sso-gate">
      {compact && (
        <button type="button" onClick={() => setNavOpen(true)} className="absolute top-2 left-2 flex items-center gap-1 rounded-full px-3 py-2 text-sm font-bold text-muted-foreground hover:bg-muted">
          <ChevronLeftIcon className="size-4" /> Channels
        </button>
      )}
      <motion.div
        initial={{ opacity: 0, y: 20, scale: 0.96 }}
        animate={{ opacity: 1, y: 0, scale: 1 }}
        transition={{ type: "spring", stiffness: 260, damping: 24 }}
        className="flex w-full max-w-sm flex-col items-center gap-4"
      >
        <div className="relative grid size-24 place-items-center">
          {!reduce &&
            [0, 1].map((n) => (
              <motion.span
                key={n}
                aria-hidden
                className="absolute inset-0 rounded-full border-2 border-primary/40"
                initial={{ scale: 0.7, opacity: 0.7 }}
                animate={{ scale: 1.6, opacity: 0 }}
                transition={{ repeat: Infinity, duration: 2.4, delay: n * 1.2, ease: "easeOut" }}
              />
            ))}
          <motion.span
            style={{ originY: 0 }}
            animate={reduce ? undefined : { rotate: [0, -8, 6, -3, 0] }}
            transition={{ repeat: Infinity, repeatDelay: 2.2, duration: 1.4, ease: "easeInOut" }}
            className="grid size-20 place-items-center rounded-[1.75rem] bg-primary/15 text-primary shadow-[0_20px_40px_-24px_var(--primary)]"
          >
            <LockKeyholeIcon className="size-9" />
          </motion.span>
        </div>
        <div className="flex flex-col gap-1.5">
          <h2 className="text-xl font-extrabold tracking-tight">Sign in with {name}</h2>
          <p className="text-sm text-muted-foreground">
            <b className="text-foreground">{server.name}</b> asks members to sign in through {name}
            {days > 0 ? ` every ${days === 1 ? "day" : `${days} days`}` : ""}. You're still a member; the channels come back once you do.
          </p>
          {server.ssoHost && (
            <motion.p
              initial={{ opacity: 0, y: 6 }}
              animate={{ opacity: 1, y: 0 }}
              transition={{ type: "spring", stiffness: 300, damping: 26, delay: 0.15 }}
              className="mt-1 text-xs text-balance text-muted-foreground"
              data-testid="sso-gate-host"
            >
              <GlobeIcon className="mr-1 inline size-3.5 -translate-y-px align-middle" />
              Signs you in at <b className="text-foreground">{server.ssoHost}</b>, which sees your IP address.
            </motion.p>
          )}
        </div>
        <div className="w-full">
          <ProviderButton
            name={name}
            icon={<BuildingIcon className="size-5 transition-transform duration-500 group-hover:-translate-y-0.5 group-hover:scale-110" />}
            onGo={() => start.go(instanceKey, server.id, { next: window.location.pathname })}
            error={start.error}
            testId="sso-gate-sign-in"
          />
        </div>
      </motion.div>
    </div>
  );
}
