import {
  BuildingIcon,
  CheckIcon,
  ClipboardPasteIcon,
  CopyIcon,
  FileBadgeIcon,
  FlaskConicalIcon,
  GlobeLockIcon,
  KeyRoundIcon,
  LoaderCircleIcon,
  PowerOffIcon,
  XIcon,
} from "lucide-react";
import { AnimatePresence, m as motion } from "motion/react";
import { useEffect, useState, type ReactNode } from "react";
import { SsoProtocol, type IdentityProvider, type ServiceProvider } from "@/gen/fuwa/v1/sso_pb";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Button } from "@/components/ui/button";
import { useI18n } from "@/i18n/react";
import { copy } from "@/lib/ui";
import { cleanDomain, readSamlMetadata } from "@/lib/sso";
import { cn } from "@/lib/utils";
import { Choice, Setting, SPRING } from "./controls";

/**
 * The identity provider people sign in through, the same for an instance
 * and for one server: which protocol, what buttons call it, its details,
 * which email domains get in, and what to tell the provider about this side.
 * `test` checks the saved provider by signing in through it.
 */
export function IdentityProviderForm({
  value,
  onChange,
  serviceProvider,
  test,
  offHint,
}: {
  value: IdentityProvider;
  onChange: (fn: (p: IdentityProvider) => void) => void;
  serviceProvider: ServiceProvider | undefined;
  test?: { onTest: () => void; pending: boolean; error: string | null; blocked?: string };
  /** What "Off" means here. */
  offHint: string;
}) {
  const { t } = useI18n();
  const on = value.protocol !== SsoProtocol.UNSPECIFIED;
  return (
    <div className="flex flex-col">
      <Setting id="sso-protocol" title={t("serversettings.nav.ssoProtocol")} hint={t("instancesettings.provider.protocolHint")} badge={false}>
        <Choice
          value={value.protocol}
          onChange={(protocol) => onChange((p) => (p.protocol = protocol))}
          options={[
            { value: SsoProtocol.UNSPECIFIED, label: t("serversettings.shared.off"), hint: offHint, icon: <PowerOffIcon className="size-4" /> },
            { value: SsoProtocol.OIDC, label: "OpenID Connect", hint: t("instancesettings.provider.oidcHint"), icon: <KeyRoundIcon className="size-4" /> },
            { value: SsoProtocol.SAML, label: "SAML 2.0", hint: t("instancesettings.provider.samlHint"), icon: <FileBadgeIcon className="size-4" /> },
          ]}
        />
      </Setting>
      <AnimatePresence initial={false} mode="popLayout">
        {on && (
          <motion.div
            key={value.protocol}
            initial={{ opacity: 0, y: 16, filter: "blur(4px)" }}
            animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
            exit={{ opacity: 0, y: -10, filter: "blur(4px)" }}
            transition={SPRING}
            className="flex flex-col"
          >
            <Setting id="sso-name" title={t("instancesettings.nav.name")} hint={t("instancesettings.provider.nameHint")} badge={false} delay={0.03}>
              <Input
                value={value.name}
                maxLength={40}
                placeholder="Acme"
                onChange={(e) => onChange((p) => (p.name = e.target.value))}
                className="h-10 rounded-xl"
                data-testid="sso-name"
              />
              <ButtonPreview name={value.name.trim()} />
            </Setting>
            {value.protocol === SsoProtocol.OIDC ? <OidcFields value={value} onChange={onChange} /> : <SamlFields value={value} onChange={onChange} />}
            <Setting
              id="sso-domains"
              title={t("serversettings.nav.ssoDomains")}
              hint={t("instancesettings.provider.domainsHint")}
              badge={false}
              delay={0.12}
            >
              <Domains value={value.emailDomains} onChange={(domains) => onChange((p) => (p.emailDomains = domains))} />
            </Setting>
            <TellProvider protocol={value.protocol} sp={serviceProvider} />
            {test && <TestRow {...test} />}
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  );
}

/** A live "Continue with ..." as people will see it. */
function ButtonPreview({ name }: { name: string }) {
  const { t } = useI18n();
  return (
    <div className="flex items-center gap-3 text-xs text-muted-foreground">
      <span className="shrink-0">{t("instancesettings.provider.showsAs")}</span>
      <span className="flex h-9 min-w-0 items-center gap-2 overflow-hidden rounded-lg bg-foreground px-3 text-sm font-extrabold text-background">
        <BuildingIcon className="size-4 shrink-0 text-primary" />
        <AnimatePresence mode="popLayout" initial={false}>
          <motion.span
            key={name || "-"}
            initial={{ opacity: 0, y: 6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.16 }}
            className="truncate"
          >
            {t("connect.provider.continueWith", { name: name || "…" })}
          </motion.span>
        </AnimatePresence>
      </span>
    </div>
  );
}

function OidcFields({ value, onChange }: { value: IdentityProvider; onChange: (fn: (p: IdentityProvider) => void) => void }) {
  const { t } = useI18n();
  const o = value.oidc!;
  return (
    <>
      <Setting id="sso-issuer" title={t("system.sso.issuer")} hint={t("instancesettings.provider.issuerHint")} badge={false} delay={0.06}>
        <Field icon={<GlobeLockIcon className="size-4" />}>
          <Input
            value={o.issuer}
            type="url"
            placeholder="https://login.acme.com"
            onChange={(e) => onChange((p) => (p.oidc!.issuer = e.target.value))}
            className="h-10 rounded-xl pl-9"
            data-testid="sso-issuer"
          />
        </Field>
      </Setting>
      <Setting id="sso-client" title={t("system.sso.client")} hint={t("instancesettings.provider.clientHint")} badge={false} delay={0.09}>
        <div className="grid gap-2 sm:grid-cols-2">
          <Input value={o.clientId} placeholder={t("system.sso.clientId")} onChange={(e) => onChange((p) => (p.oidc!.clientId = e.target.value))} className="h-10 rounded-xl" />
          <Input
            value={o.clientSecret}
            type="password"
            autoComplete="off"
            placeholder={o.clientSecretSet ? t("instancesettings.provider.secretKept") : t("system.sso.clientSecret")}
            onChange={(e) => onChange((p) => (p.oidc!.clientSecret = e.target.value))}
            className="h-10 rounded-xl"
          />
        </div>
        <Input
          value={o.extraScopes}
          placeholder={t("instancesettings.provider.scopes")}
          onChange={(e) => onChange((p) => (p.oidc!.extraScopes = e.target.value))}
          className="h-10 rounded-xl text-sm"
        />
      </Setting>
    </>
  );
}

function SamlFields({ value, onChange }: { value: IdentityProvider; onChange: (fn: (p: IdentityProvider) => void) => void }) {
  const { t } = useI18n();
  const s = value.saml!;
  const [pasting, setPasting] = useState(false);
  const [xml, setXml] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [filled, setFilled] = useState(0);

  function fill() {
    const found = readSamlMetadata(xml);
    if (!found || (!found.entityId && !found.ssoUrl)) return setProblem(t("instancesettings.provider.notMetadata"));
    onChange((p) => {
      p.saml!.entityId = found.entityId || p.saml!.entityId;
      p.saml!.ssoUrl = found.ssoUrl || p.saml!.ssoUrl;
      p.saml!.certificates = found.certificates || p.saml!.certificates;
    });
    setProblem(found.ssoUrl ? null : t("instancesettings.provider.noRedirect"));
    setPasting(false);
    setXml("");
    setFilled((n) => n + 1);
  }

  return (
    <>
      <Setting id="sso-metadata" title={t("instancesettings.provider.metadata")} hint={t("instancesettings.provider.metadataHint")} badge={false} delay={0.06}>
        <AnimatePresence initial={false} mode="popLayout">
          {pasting ? (
            <motion.div key="paste" initial={{ opacity: 0, height: 0 }} animate={{ opacity: 1, height: "auto" }} exit={{ opacity: 0, height: 0 }} transition={SPRING} className="flex flex-col gap-2 overflow-hidden">
              <Textarea autoFocus rows={5} value={xml} onChange={(e) => setXml(e.target.value)} placeholder="<EntityDescriptor …>" className="rounded-xl font-mono text-xs" />
              <div className="flex gap-2">
                <Button type="button" onClick={fill} disabled={!xml.trim()} className="btn rounded-xl font-bold">
                  {t("instancesettings.provider.fillIn")}
                </Button>
                <Button type="button" variant="ghost" onClick={() => setPasting(false)} className="rounded-xl font-bold">
                  {t("common.cancel")}
                </Button>
              </div>
            </motion.div>
          ) : (
            <motion.div key="button" initial={{ opacity: 0 }} animate={{ opacity: 1 }} exit={{ opacity: 0 }} className="flex items-center gap-3">
              <Button type="button" variant="outline" onClick={() => setPasting(true)} className="group rounded-xl font-bold">
                <ClipboardPasteIcon className="transition-transform group-hover:-rotate-12" /> {t("instancesettings.provider.pasteMetadata")}
              </Button>
              <AnimatePresence>
                {filled > 0 && !problem && (
                  <motion.span key={filled} initial={{ opacity: 0, scale: 0.6 }} animate={{ opacity: 1, scale: 1 }} exit={{ opacity: 0 }} transition={SPRING} className="flex items-center gap-1 text-xs font-bold text-emerald-500">
                    <CheckIcon className="size-3.5" strokeWidth={3} /> {t("instancesettings.provider.filledIn")}
                  </motion.span>
                )}
              </AnimatePresence>
            </motion.div>
          )}
        </AnimatePresence>
        {problem && <p className="text-xs text-amber-600 dark:text-amber-400">{problem}</p>}
      </Setting>
      <Setting id="sso-entity" title={t("instancesettings.provider.entity")} hint={t("instancesettings.provider.entityHint")} badge={false} delay={0.09}>
        <Glow key={`e${filled}`} on={filled > 0}>
          <Input value={s.entityId} placeholder="https://idp.acme.com/metadata" onChange={(e) => onChange((p) => (p.saml!.entityId = e.target.value))} className="h-10 rounded-xl" data-testid="saml-entity" />
        </Glow>
        <Glow key={`u${filled}`} on={filled > 0} delay={0.06}>
          <Input value={s.ssoUrl} type="url" placeholder="https://idp.acme.com/sso/redirect" onChange={(e) => onChange((p) => (p.saml!.ssoUrl = e.target.value))} className="h-10 rounded-xl" />
        </Glow>
      </Setting>
      <Setting id="sso-certificates" title={t("instancesettings.provider.certificates")} hint={t("instancesettings.provider.certificatesHint")} badge={false} delay={0.1}>
        <Glow key={`c${filled}`} on={filled > 0} delay={0.12}>
          <Textarea
            rows={4}
            value={s.certificates}
            placeholder={"-----BEGIN CERTIFICATE-----\n…\n-----END CERTIFICATE-----"}
            onChange={(e) => onChange((p) => (p.saml!.certificates = e.target.value))}
            className="rounded-xl font-mono text-xs"
          />
        </Glow>
      </Setting>
    </>
  );
}

/** Lights a field up for a moment after metadata filled it. */
function Glow({ on, delay = 0, children }: { on: boolean; delay?: number; children: ReactNode }) {
  return (
    <motion.div
      initial={on ? { boxShadow: "0 0 0 0px color-mix(in srgb, var(--primary) 0%, transparent)" } : false}
      animate={on ? { boxShadow: ["0 0 0 0px color-mix(in srgb, var(--primary) 0%, transparent)", "0 0 0 4px color-mix(in srgb, var(--primary) 45%, transparent)", "0 0 0 0px color-mix(in srgb, var(--primary) 0%, transparent)"] } : undefined}
      transition={{ duration: 1, delay }}
      className="rounded-xl"
    >
      {children}
    </motion.div>
  );
}

function Field({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <div className="relative">
      <span className="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted-foreground">{icon}</span>
      {children}
    </div>
  );
}

/** Email domains as chips: type and press Enter (or a comma), click one to take it off. */
function Domains({ value, onChange }: { value: string[]; onChange: (domains: string[]) => void }) {
  const { t } = useI18n();
  const [text, setText] = useState("");
  function add() {
    const have = new Set(value);
    const fresh = text.split(/[\s,]+/).map(cleanDomain).filter((d) => d && !have.has(d));
    if (fresh.length) onChange([...value, ...fresh].slice(0, 20));
    setText("");
  }
  return (
    <div className="flex min-h-10 flex-wrap items-center gap-1.5 rounded-xl border bg-transparent p-1.5 focus-within:ring-2 focus-within:ring-ring/40">
      <AnimatePresence initial={false} mode="popLayout">
        {value.map((d) => (
          <motion.button
            layout
            key={d}
            type="button"
            initial={{ opacity: 0, scale: 0.6 }}
            animate={{ opacity: 1, scale: 1 }}
            exit={{ opacity: 0, scale: 0.6 }}
            transition={SPRING}
            onClick={() => onChange(value.filter((x) => x !== d))}
            className="group flex items-center gap-1 rounded-full bg-primary/15 py-1 pr-1.5 pl-2.5 text-xs font-bold text-primary"
            aria-label={t("instancesettings.provider.removeDomain", { domain: d })}
          >
            @{d}
            <XIcon className="size-3 transition-transform group-hover:rotate-90" />
          </motion.button>
        ))}
      </AnimatePresence>
      <input
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === ",") {
            e.preventDefault();
            add();
          } else if (e.key === "Backspace" && !text && value.length) {
            onChange(value.slice(0, -1));
          }
        }}
        onBlur={add}
        placeholder={value.length ? "" : "acme.com"}
        className="h-7 min-w-24 flex-1 bg-transparent px-1.5 text-sm outline-none"
      />
    </div>
  );
}

/** What to enter at the provider, each with a copy button. */
function TellProvider({ protocol, sp }: { protocol: SsoProtocol; sp: ServiceProvider | undefined }) {
  const { t } = useI18n();
  if (!sp) return null;
  const rows =
    protocol === SsoProtocol.OIDC
      ? [{ label: "Redirect URI", value: sp.oidcRedirectUri }]
      : [
          { label: t("instancesettings.provider.entityAndMetadata"), value: sp.samlEntityId },
          { label: "Assertion Consumer Service (HTTP-POST)", value: sp.samlAcsUrl },
        ];
  return (
    <motion.section
      initial={{ opacity: 0, y: 12 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...SPRING, delay: 0.15 }}
      className="my-4 rounded-2xl border border-dashed p-4"
    >
      <h3 className="text-sm font-extrabold">{t("instancesettings.provider.tell")}</h3>
      <p className="mt-0.5 text-xs text-muted-foreground">
        {t(protocol === SsoProtocol.OIDC ? "instancesettings.provider.tellOidc" : "instancesettings.provider.tellSaml")}
      </p>
      <div className="mt-3 flex flex-col gap-2">
        {rows.map((r, n) => (
          <CopyRow key={r.label} label={r.label} value={r.value} delay={0.2 + n * 0.05} />
        ))}
      </div>
    </motion.section>
  );
}

function CopyRow({ label, value, delay }: { label: string; value: string; delay: number }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1400);
    return () => clearTimeout(t);
  }, [copied]);
  return (
    <motion.div initial={{ opacity: 0, x: -8 }} animate={{ opacity: 1, x: 0 }} transition={{ ...SPRING, delay }} className="flex min-w-0 flex-col gap-1">
      <span className="text-[0.7rem] font-bold text-muted-foreground uppercase">{label}</span>
      <div className="flex min-w-0 items-center gap-2 rounded-xl bg-muted/60 py-1 pr-1 pl-3">
        <code className="min-w-0 flex-1 truncate font-mono text-xs">{value}</code>
        <motion.button
          type="button"
          whileTap={{ scale: 0.85 }}
          onClick={() => {
            copy(t, value, label);
            setCopied(true);
          }}
          aria-label={t("common.copyThing", { what: label })}
          className={cn("grid size-8 shrink-0 place-items-center rounded-lg transition-colors", copied ? "text-emerald-500" : "text-muted-foreground hover:bg-background hover:text-foreground")}
        >
          <AnimatePresence mode="popLayout" initial={false}>
            <motion.span key={String(copied)} initial={{ scale: 0.4, rotate: -30 }} animate={{ scale: 1, rotate: 0 }} exit={{ scale: 0.4, opacity: 0 }} transition={SPRING}>
              {copied ? <CheckIcon className="size-4" strokeWidth={3} /> : <CopyIcon className="size-4" />}
            </motion.span>
          </AnimatePresence>
        </motion.button>
      </div>
    </motion.div>
  );
}

/** Signs in through the saved provider to see that it works. */
function TestRow({ onTest, pending, error, blocked }: { onTest: () => void; pending: boolean; error: string | null; blocked?: string }) {
  const { t } = useI18n();
  return (
    <Setting id="sso-test" title={t("instancesettings.nav.ssoTest")} hint={t("instancesettings.provider.testHint")} badge={false} delay={0.18}>
      <div className="flex flex-wrap items-center gap-3">
        <Button type="button" variant="outline" onClick={onTest} disabled={pending || !!blocked} className="group rounded-xl font-bold" data-testid="sso-test">
          {pending ? <LoaderCircleIcon className="animate-spin" /> : <FlaskConicalIcon className="transition-transform group-hover:-rotate-12" />}
          {t("instancesettings.nav.ssoTest")}
        </Button>
        {blocked && <span className="text-xs text-muted-foreground">{blocked}</span>}
      </div>
      {error && <p className="text-sm text-destructive first-letter:uppercase">{error}</p>}
    </Setting>
  );
}
