/**
 * Single sign-on through an organization's identity provider (OpenID Connect
 * or SAML 2.0), for an instance's own sign-in or one community server's. It
 * works like signing in with waifu.dev (`linked.ts`): the app keeps a secret
 * while the browser visits the provider, the instance checks who signed in
 * and sends the browser on to /auth/sso/done with a one-time code in the
 * fragment, and that page finishes (or hands the code back to the fuwa app on
 * another address that started it).
 */
import { SsoProtocol, type IdentityProvider } from "@/gen/fuwa/v1/sso_pb";

/** Where an instance sends the browser once the provider answers, on every fuwa app. */
export const DONE = "/auth/sso/done";

/** A sign-in waiting for the browser to come back. */
export type PendingSso = {
  /** The instance. */
  url: string;
  /** Proves this app started it. */
  secret: string;
  /** Where to go once done, or the instance's (or server's) home. */
  next: string | null;
  /** Set for a server's sign-in, unset for the instance's. */
  serverId?: string;
  /** For a server: join it once signed in. */
  join?: boolean;
  /** For a server: the account that started it (the instance only lets that account finish). */
  userId?: string;
  /** For a server out of Browse: the invite joining takes. */
  inviteCode?: string;
  /** An admin checking the instance's provider: nobody gets signed in. */
  test?: boolean;
  startedAt: number;
};

const KEY = "fuwa.sso.";
/** Sign-ins run out on the instance after ten minutes. */
const TTL = 10 * 60 * 1000;

export function savePendingSso(state: string, pending: PendingSso) {
  try {
    sessionStorage.setItem(KEY + state, JSON.stringify(pending));
    return true;
  } catch {
    return false;
  }
}

/** The sign-in this tab started with `state`, taken so it finishes once. */
export function takePendingSso(state: string): PendingSso | null {
  try {
    const raw = sessionStorage.getItem(KEY + state);
    sessionStorage.removeItem(KEY + state);
    const pending = raw ? (JSON.parse(raw) as PendingSso) : null;
    return pending && Date.now() - pending.startedAt < TTL ? pending : null;
  } catch {
    return null;
  }
}

/** Whether a provider is filled in enough to try, as the instance checks it. */
export function providerReady(p: IdentityProvider | undefined) {
  if (!p || !p.name.trim()) return false;
  if (p.protocol === SsoProtocol.OIDC) {
    return !!p.oidc?.issuer.trim() && !!p.oidc.clientId.trim() && (!!p.oidc.clientSecret || p.oidc.clientSecretSet);
  }
  if (p.protocol === SsoProtocol.SAML) {
    return !!p.saml?.entityId.trim() && !!p.saml.ssoUrl.trim() && p.saml.certificates.includes("BEGIN CERTIFICATE");
  }
  return false;
}

/** "Acme", or the provider's host when it has no name yet. */
export function providerName(p: IdentityProvider | undefined) {
  return p?.name.trim() || "your organization";
}

/**
 * What a SAML metadata document says about its identity provider: entity ID,
 * HTTP-Redirect sign-in URL and signing certificates. Read in the browser
 * only to fill the form; the instance checks every field again.
 */
export function readSamlMetadata(xml: string): { entityId: string; ssoUrl: string; certificates: string } | null {
  const doc = new DOMParser().parseFromString(xml, "application/xml");
  if (doc.getElementsByTagName("parsererror").length) return null;
  const byLocal = (root: Element | Document, name: string) =>
    [...root.getElementsByTagName("*")].filter((e) => e.localName === name);
  const descriptor = byLocal(doc, "EntityDescriptor")[0];
  const idp = byLocal(doc, "IDPSSODescriptor")[0];
  if (!descriptor || !idp) return null;
  const sso = byLocal(idp, "SingleSignOnService").find((e) => e.getAttribute("Binding")?.endsWith(":HTTP-Redirect"));
  const certs = byLocal(idp, "KeyDescriptor")
    .filter((k) => (k.getAttribute("use") ?? "signing") === "signing")
    .flatMap((k) => byLocal(k, "X509Certificate"))
    .map((c) => (c.textContent ?? "").replace(/\s+/g, ""))
    .filter(Boolean);
  const pem = [...new Set(certs)]
    .slice(0, 4)
    .map((b64) => `-----BEGIN CERTIFICATE-----\n${b64.match(/.{1,64}/g)!.join("\n")}\n-----END CERTIFICATE-----`)
    .join("\n");
  return { entityId: descriptor.getAttribute("entityID") ?? "", ssoUrl: sso?.getAttribute("Location") ?? "", certificates: pem };
}

/** "acme.com" from whatever someone typed ("@Acme.com ", "https://acme.com"). */
export function cleanDomain(text: string) {
  return text
    .trim()
    .toLowerCase()
    .replace(/^https?:\/\//, "")
    .replace(/^@/, "")
    .replace(/\/.*$/, "");
}

const DAY = 86_400_000;

/**
 * Whether you're locked out of a server's channels until you sign in through
 * its provider: it requires one, you aren't its owner or an agent, and your
 * sign-in is missing or older than its recheck. The instance decides; this
 * only says why the channels went away.
 */
export function ssoLocked(
  server: { ssoRequired: boolean; ssoRecheckDays: number; ownerId: string } | undefined,
  member: { user?: { id: string; kind: number }; ssoSignedInAt?: { seconds: bigint } } | undefined,
  agentKind: number,
  now = Date.now(),
) {
  if (!server?.ssoRequired || !member?.user) return false;
  if (member.user.id === server.ownerId || member.user.kind === agentKind) return false;
  const at = member.ssoSignedInAt ? Number(member.ssoSignedInAt.seconds) * 1000 : 0;
  if (!at) return true;
  return server.ssoRecheckDays > 0 && now - at >= server.ssoRecheckDays * DAY;
}

const SIGNED_IN = "fuwa.sso.signed-in.";

/** Remembers, for this tab and account ("<instance>|<user id>"), that you signed in for a server you haven't joined, so it offers to apply next. */
export function rememberServerSignIn(account: string, serverId: string) {
  try {
    sessionStorage.setItem(`${SIGNED_IN}${account}/${serverId}`, String(Date.now()));
  } catch {
    // The instance still knows; the button just asks to sign in again.
  }
}

export function signedInForServer(account: string, serverId: string) {
  try {
    const at = Number(sessionStorage.getItem(`${SIGNED_IN}${account}/${serverId}`) ?? 0);
    return Date.now() - at < DAY;
  } catch {
    return false;
  }
}
