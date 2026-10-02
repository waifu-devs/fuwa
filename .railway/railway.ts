import { defineRailway, image, project, service, volume } from "railway/iac";

/**
 * fuwa.chat, the fuwa instance Waifu Devs hosts. It has a Railway project of its own,
 * "fuwa", apart from the site's. Pull requests that touch .railway/ get a plan comment;
 * merging applies it (.github/workflows/railway-config.yml).
 *
 * Set once by hand, not here: the shared variable FUWA_ENCRYPTION_KEY (64 hex
 * characters). Every database on the volume is encrypted with it and fuwa won't open
 * them with any other key, so it must never change. Keep a copy in a password manager.
 * The DNS records for fuwa.chat live with its registrar.
 */

/** Railway's US East (Virginia) region, where the site runs too. */
const REGION = "us-east4-eqdc4a";
const DOMAIN = "fuwa.chat";
/** fuwa listens on PORT; the domain sends traffic here. */
const PORT = 8080;
/**
 * The window Railway may apply an image update in: every day (0 is Sunday), all day,
 * the dashboard's "Anytime". Without it, updates wait for a maintenance window.
 */
const ANYTIME = [0, 1, 2, 3, 4, 5, 6].map((day) => ({ day, startHour: 0, endHour: 24 }));

export default defineRailway((ctx) => {
  // node.db (accounts, sessions, instance settings) and one file per community server.
  // 5 GB, the most the Hobby plan allows. A volume can grow but never shrink.
  const data = volume("fuwa-data", { region: REGION, sizeMB: 5000 });

  const fuwa = service("fuwa", {
    // The image every merge to master publishes; the publish workflow redeploys fuwa
    // onto it right away. Auto updates are the fallback: Railway takes a new image as
    // soon as it notices one, which can take a few hours.
    source: image("ghcr.io/waifu-devs/fuwa:latest", {
      autoUpdates: { type: "patch", schedule: ANYTIME },
    }),
    healthcheck: "/healthz",
    // One replica: every database is a file on the one volume.
    regions: { [REGION]: 1 },
    // Added in the dashboard first (Railway configuration can't register a new custom
    // domain), declared here so later applies keep it.
    domains: [{ domain: DOMAIN, port: PORT }],
    volumeMounts: { "/data": data },
    // Give open streams and the last writes time to finish on shutdown.
    deploy: { drainingSeconds: 30 },
    env: {
      PORT: String(PORT),
      FUWA_PUBLIC_URL: `https://${DOMAIN}`,
      FUWA_NODE_NAME: "fuwa",
      // Marks this instance's anonymous usage signal as ours; changes nothing else.
      FUWA_HOSTING: "hosted",
      FUWA_ENCRYPTION_KEY: ctx.shared.FUWA_ENCRYPTION_KEY,
    },
  });

  return project("fuwa", { resources: [data, fuwa] });
});
