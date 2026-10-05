import { timestampDate } from "@bufbuild/protobuf/wkt";
import { BuildingIcon, Flower2Icon, KeyRoundIcon, LinkIcon, LoaderCircleIcon, Unlink2Icon } from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useCallback, useEffect, useState, type FormEvent, type ReactNode } from "react";
import type { ListSignInMethodsResponse } from "@/gen/fuwa/v1/account_pb";
import type { SignInMethod } from "@/gen/fuwa/v1/types_pb";
import { listSignInMethods, run, startProviderLink, unlinkProvider } from "@/fuwa/actions";
import type { FuwaError } from "@/fuwa/errors";
import { useAction } from "@/fuwa/hooks";
import { SPRING } from "@/lib/motion";
import { Private } from "@/components/Private";
import { ProviderMark } from "@/components/ProviderMarks";
import { PASSWORD_MAX } from "@/components/settings/account/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useI18n } from "@/i18n/react";
import { toast } from "@/lib/ui";
import { cn } from "@/lib/utils";

const day = (d: Date) => d.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });

/** What a change to how you sign in is waiting on: linking a provider, or unlinking one. */
type Asking = { action: "link" | "unlink"; id: string; name: string };

function MethodIcon({ kind }: { kind: string }) {
  const className = "size-5";
  if (kind === "password") return <KeyRoundIcon className={className} />;
  if (kind === "waifu") return <Flower2Icon className={className} />;
  if (kind === "sso") return <BuildingIcon className={className} />;
  return <ProviderMark id={kind} className="size-[18px]" />;
}

/**
 * How your account signs in: its password, waifu.dev or your organization,
 * and Google, X or Twitch, which you can link and unlink here. The last way
 * in that works stays. Changes ask for proof beyond being signed in.
 */
export function SignInMethods({ instanceKey }: { instanceKey: string }) {
  const { t } = useI18n();
  const [listed, setListed] = useState<ListSignInMethodsResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [asking, setAsking] = useState<Asking | null>(null);

  const load = useCallback(() => {
    run(listSignInMethods(instanceKey)).then(
      (l) => {
        setListed(l);
        setError(null);
      },
      (err: FuwaError) => setError(err.message),
    );
  }, [instanceKey]);
  useEffect(load, [load]);

  if (error) return <p className="text-sm text-destructive first-letter:uppercase">{error}</p>;
  if (!listed) return <LoaderCircleIcon className="size-5 animate-spin text-muted-foreground" />;

  const working = listed.methods.filter((m) => m.works);
  const name = (m: SignInMethod) => (m.kind === "password" ? t("accountsettings.signIn.password") : m.name);

  return (
    <div className="flex flex-col gap-6" data-testid="sign-in-methods">
      <section data-setting="sign-in-methods" className="flex flex-col gap-2.5">
        <h3 className="font-extrabold">{t("accountsettings.signIn.yours")}</h3>
        <ul className="flex flex-col gap-2">
          <AnimatePresence initial={false}>
            {listed.methods.map((m, n) => {
              const only = m.works && working.length === 1;
              const provider = !["password", "waifu", "sso"].includes(m.kind);
              return (
                <motion.li
                  key={m.kind}
                  layout
                  initial={{ opacity: 0, y: 8 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, x: -16 }}
                  transition={{ ...SPRING, delay: n * 0.04 }}
                  className="flex items-center gap-3 rounded-2xl border bg-card p-3"
                  data-testid={`sign-in-method-${m.kind}`}
                >
                  <span className="grid size-10 shrink-0 place-items-center rounded-xl bg-foreground text-primary">
                    <MethodIcon kind={m.kind} />
                  </span>
                  <div className="min-w-0 flex-1">
                    <p className="flex items-center gap-2 font-bold">
                      {name(m)}
                      {!m.works && (
                        <span className="rounded-full bg-muted px-2 py-0.5 text-[11px] font-bold text-muted-foreground">
                          {t("accountsettings.signIn.off")}
                        </span>
                      )}
                    </p>
                    <p className="truncate text-xs text-muted-foreground">
                      {m.accountName && <Private text={m.accountName} />}
                      {m.accountName && m.linkedAt && " · "}
                      {m.linkedAt && t("accountsettings.signIn.linkedOn", { date: day(timestampDate(m.linkedAt)) })}
                    </p>
                  </div>
                  {provider && (
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={only}
                      title={only ? t("accountsettings.signIn.onlyWay") : undefined}
                      onClick={() => setAsking({ action: "unlink", id: m.kind, name: m.name })}
                      className="rounded-xl font-bold text-destructive hover:text-destructive"
                    >
                      <Unlink2Icon className="size-4" /> {t("accountsettings.signIn.unlink")}
                    </Button>
                  )}
                </motion.li>
              );
            })}
          </AnimatePresence>
        </ul>
        {working.length === 1 && (
          <p className="text-xs text-muted-foreground">{t("accountsettings.signIn.onlyWay")}</p>
        )}
      </section>

      {listed.canLink && listed.available.length > 0 && (
        <section data-setting="link-provider" className="flex flex-col gap-2.5">
          <h3 className="font-extrabold">{t("accountsettings.signIn.add")}</h3>
          <div className="flex flex-col gap-2 sm:flex-row sm:flex-wrap">
            {listed.available.map((p) => (
              <motion.button
                key={p.id}
                type="button"
                whileHover={{ y: -2 }}
                whileTap={{ scale: 0.97 }}
                transition={SPRING}
                onClick={() => setAsking({ action: "link", id: p.id, name: p.name })}
                className="group flex items-center gap-2.5 rounded-xl border bg-card px-3.5 py-2.5 font-bold transition-colors hover:border-primary/60"
                data-testid={`link-${p.id}`}
              >
                <ProviderMark id={p.id} className="size-4 transition-transform group-hover:scale-110" />
                {t("accountsettings.signIn.link", { name: p.name })}
                <LinkIcon className="size-3.5 text-muted-foreground" />
              </motion.button>
            ))}
          </div>
          <p className="text-xs text-muted-foreground">{t("accountsettings.signIn.addNote")}</p>
        </section>
      )}

      <AnimatePresence>
        {asking && (
          <Proof
            key={`${asking.action}-${asking.id}`}
            instanceKey={instanceKey}
            asking={asking}
            listed={listed}
            onCancel={() => setAsking(null)}
            onUnlinked={() => {
              toast(t("accountsettings.signIn.unlinked", { name: asking.name }));
              setAsking(null);
              load();
            }}
          />
        )}
      </AnimatePresence>
    </div>
  );
}

/** The password, two-step code, or a fresh sign-in a change asks for, then the change. */
function Proof({
  instanceKey,
  asking,
  listed,
  onCancel,
  onUnlinked,
}: {
  instanceKey: string;
  asking: Asking;
  listed: ListSignInMethodsResponse;
  onCancel: () => void;
  onUnlinked: () => void;
}) {
  const { t } = useI18n();
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const link = useAction(startProviderLink);
  const unlink = useAction(unlinkProvider);
  const busy = link.pending || unlink.pending;
  const linking = asking.action === "link";
  const ready = (!listed.needsPassword || password.length > 0) && (!listed.needsCode || code.trim().length > 0);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const proof = { password, code: code.trim() };
    if (linking) {
      // On success the browser is already on its way to the provider.
      await link.go(instanceKey, asking.id, proof, window.location.pathname);
      return;
    }
    if (await unlink.go(instanceKey, asking.id, proof)) onUnlinked();
  }

  return (
    <motion.form
      initial={{ opacity: 0, y: 12, scale: 0.98 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, y: 8, scale: 0.98 }}
      transition={SPRING}
      onSubmit={submit}
      className="flex flex-col gap-3 rounded-2xl border bg-muted/40 p-4"
      data-testid="sign-in-proof"
    >
      <p className="font-extrabold">{t(linking ? "accountsettings.signIn.linkTitle" : "accountsettings.signIn.unlinkTitle", { name: asking.name })}</p>
      {listed.needsFreshSignIn && <p className="text-sm text-muted-foreground">{t("accountsettings.signIn.freshNote")}</p>}
      {listed.needsPassword && (
        <ProofField id="sign-in-proof-password" label={t("accountsettings.signIn.passwordLabel")}>
          <Input
            id="sign-in-proof-password"
            type="password"
            autoComplete="current-password"
            maxLength={PASSWORD_MAX}
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            className="h-11 rounded-xl"
          />
        </ProofField>
      )}
      {listed.needsCode && (
        <ProofField id="sign-in-proof-code" label={t("accountsettings.signIn.codeLabel")}>
          <Input
            id="sign-in-proof-code"
            autoComplete="one-time-code"
            inputMode="numeric"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            className="h-11 rounded-xl font-mono tracking-widest"
          />
        </ProofField>
      )}
      <Problem text={link.error ?? unlink.error} />
      <div className="flex flex-col gap-2 sm:flex-row-reverse">
        <Button
          type="submit"
          disabled={busy || !ready}
          className={cn("btn h-10 rounded-xl font-bold", !linking && "bg-destructive text-white hover:bg-destructive/90")}
        >
          <ProofIcon busy={busy} linking={linking} />
          {linking ? t("accountsettings.signIn.continueTo", { name: asking.name }) : t("accountsettings.signIn.unlink")}
        </Button>
        <Button type="button" variant="ghost" className="h-10 rounded-xl font-bold" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
      </div>
    </motion.form>
  );
}

function ProofField({ id, label, children }: { id: string; label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={id} className="font-bold">
        {label}
      </Label>
      {children}
    </div>
  );
}

function ProofIcon({ busy, linking }: { busy: boolean; linking: boolean }) {
  if (busy) return <LoaderCircleIcon className="animate-spin" />;
  return linking ? <LinkIcon /> : <Unlink2Icon />;
}

function Problem({ text }: { text: string | null | undefined }) {
  return (
    <AnimatePresence>
      {text && (
        <motion.p
          initial={{ opacity: 0, y: -4 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -4 }}
          className="text-sm text-destructive first-letter:uppercase"
        >
          {text}
        </motion.p>
      )}
    </AnimatePresence>
  );
}
