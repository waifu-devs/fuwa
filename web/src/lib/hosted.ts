/**
 * Addresses Waifu Devs runs fuwa on. An instance only counts as hosted by us
 * when the app reached it at one of these over https: the browser checked the
 * address's certificate, which only we hold. What an instance says about
 * itself (its name, or FUWA_HOSTING) never counts, since any server can say
 * anything.
 */
const HOSTED_DOMAINS = ["fuwa.chat"];

/** Whether the instance at this base URL is one Waifu Devs runs. */
export function hostedByUs(url: string | undefined): boolean {
  if (!url) return false;
  try {
    const u = new URL(url);
    return u.protocol === "https:" && HOSTED_DOMAINS.some((d) => u.hostname === d || u.hostname.endsWith(`.${d}`));
  } catch {
    return false;
  }
}

/** Waifu Devs' terms and privacy policy, served at /terms and /privacy on its instances alone. */
export type LegalPage = "terms" | "privacy";

/** Where people write to Waifu Devs about the terms and their privacy. */
export const LEGAL_CONTACT = "veryimportantemail@fuwa.chat";

/** The page on the hosted instance at this base URL. */
export function legalLink(url: string, page: LegalPage): string {
  return new URL(`/${page}`, url).href;
}
