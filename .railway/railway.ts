import { defineRailway, image, project, service, volume } from "railway/iac";

/**
 * fuwa.chat, the fuwa instance Waifu Devs hosts. It has a Railway project of its own,
 * "fuwa", apart from the site's. Pull requests that touch .railway/ get a plan comment;
 * merging applies it (.github/workflows/railway-config.yml).
 *
 * Set once by hand, not here: the shared variable FUWA_ENCRYPTION_KEY (64 hex
 * characters). Every database on the volumes is encrypted with it and fuwa won't open
 * them with any other key, so it must never change. Keep a copy in a password manager.
 * Before turning SPLIT on, also FUWA_CLUSTER_KEY (`openssl rand -hex 32`), the secret the
 * parts send each other; that one can change later, and every part restarts with it.
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
/** 5 GB, the most a Hobby plan volume holds. A volume can grow but never shrink. */
const VOLUME_MB = 5000;
/** Volumes a Hobby plan project can have: the directory's, and one per shard. */
const MAX_VOLUMES = 10;

/**
 * How fuwa.chat runs (see "Scaling out" in the README). `null` is one `fuwa` process
 * doing everything, which is how it runs now. Set, it runs as separate parts, each a
 * service here:
 *
 * - `fuwa`, the gateways clients reach at fuwa.chat. They keep nothing, so `gateways`
 *   replicas of it share the connections.
 * - `fuwa-directory`, with accounts, sessions, settings and pictures. It takes over the
 *   `fuwa-data` volume the single process used. Always one.
 * - `fuwa-shard-1` to `fuwa-shard-<shards>`, the community servers, each shard on a
 *   volume of its own. New servers go to the shard holding the fewest.
 *
 * The first time it's set, the first shard to start takes the single process's servers
 * from the directory, so nothing has to be copied by hand. Raise `shards` to spread new
 * servers wider, but never lower it: removing a shard here deletes its volume, and the
 * servers on it.
 */
const SPLIT: { gateways: number; shards: number } | null = null;

export default defineRailway((ctx) => {
  // The image every merge to master publishes; the publish workflow redeploys every
  // part onto it right away. Auto updates are the fallback: Railway takes a new image
  // as soon as it notices one, which can take a few hours.
  const fuwaImage = () =>
    image("ghcr.io/waifu-devs/fuwa:latest", {
      autoUpdates: { type: "patch", schedule: ANYTIME },
    });

  // node.db (accounts, sessions, instance settings) and, while it's one process, one
  // file per community server.
  const data = volume("fuwa-data", { region: REGION, sizeMB: VOLUME_MB });

  // What fuwa.chat is, whichever part reads it.
  const instance = {
    FUWA_PUBLIC_URL: `https://${DOMAIN}`,
    FUWA_NODE_NAME: "fuwa",
  };

  if (!SPLIT) {
    const fuwa = service("fuwa", {
      source: fuwaImage(),
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
        ...instance,
        // Marks this instance's anonymous usage signal as ours; changes nothing else.
        FUWA_HOSTING: "hosted",
        FUWA_ENCRYPTION_KEY: ctx.shared.FUWA_ENCRYPTION_KEY,
      },
    });

    return project("fuwa", { resources: [data, fuwa] });
  }

  const { gateways, shards } = SPLIT;
  if (!Number.isInteger(gateways) || gateways < 1) {
    throw new Error("SPLIT.gateways must be a whole number, at least 1");
  }
  if (!Number.isInteger(shards) || shards < 1 || shards > MAX_VOLUMES - 1) {
    throw new Error(`SPLIT.shards must be a whole number from 1 to ${MAX_VOLUMES - 1}`);
  }

  // Every part listens on PORT on both IPv4 and IPv6, and the parts reach each other over
  // Railway's private network, sending the cluster key with every call.
  const part = (role: string) => ({
    PORT: String(PORT),
    FUWA_HOST: "::",
    FUWA_ROLE: role,
    FUWA_CLUSTER_KEY: ctx.shared.FUWA_CLUSTER_KEY,
  });
  const internalUrl = (name: string) => `http://\${{${name}.RAILWAY_PRIVATE_DOMAIN}}:${PORT}`;

  const directory = service("fuwa-directory", {
    source: fuwaImage(),
    healthcheck: "/healthz",
    // One replica: node.db is a file on its volume.
    regions: { [REGION]: 1 },
    volumeMounts: { "/data": data },
    deploy: { drainingSeconds: 30 },
    env: {
      ...part("directory"),
      // The instance's own settings live with the directory, which hands them to every
      // other part, and it sends the usage signal.
      ...instance,
      FUWA_HOSTING: "hosted",
      FUWA_ENCRYPTION_KEY: ctx.shared.FUWA_ENCRYPTION_KEY,
    },
  });

  const shardParts = Array.from({ length: shards }, (_, i) => {
    const name = `fuwa-shard-${i + 1}`;
    const shardData = volume(`${name}-data`, { region: REGION, sizeMB: VOLUME_MB });
    const shard = service(name, {
      source: fuwaImage(),
      healthcheck: "/healthz",
      regions: { [REGION]: 1 },
      volumeMounts: { "/data": shardData },
      deploy: { drainingSeconds: 30 },
      env: {
        ...part("shard"),
        // Never renamed: the directory knows which servers are on which shard by it.
        FUWA_SHARD_ID: `shard-${i + 1}`,
        FUWA_DIRECTORY_URL: internalUrl(directory.name),
        FUWA_INTERNAL_URL: internalUrl(name),
        // The same key as the directory's, so server files can move between shards.
        FUWA_ENCRYPTION_KEY: ctx.shared.FUWA_ENCRYPTION_KEY,
      },
    });
    return [shardData, shard];
  });

  const gateway = service("fuwa", {
    source: fuwaImage(),
    healthcheck: "/healthz",
    // Gateways keep nothing, so any number of replicas share fuwa.chat's traffic.
    regions: { [REGION]: gateways },
    domains: [{ domain: DOMAIN, port: PORT }],
    // Give open streams time to finish on shutdown; clients follow again elsewhere.
    deploy: { drainingSeconds: 30 },
    env: {
      ...part("gateway"),
      ...instance,
      FUWA_DIRECTORY_URL: internalUrl(directory.name),
    },
  });

  return project("fuwa", { resources: [data, directory, ...shardParts.flat(), gateway] });
});
