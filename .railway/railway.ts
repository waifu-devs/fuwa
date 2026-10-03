import { bucket, defineRailway, image, project, ref, service, volume } from "railway/iac";

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
/** Buckets have their own region names; this is Virginia too. It can't change once created. */
const BUCKET_REGION = "iad";
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
/** The port calls' sound uses (FUWA_MEDIA_PORT), reached through a Railway TCP proxy. */
const MEDIA_PORT = 50000;
/**
 * The home region's media part when it runs on a host of its own, with real UDP and port
 * 443 (deploy/media-host, docs/self-hosting.md), such as "https://media.fuwa.chat:8443".
 * Empty: the `fuwa-media` service here carries calls, over the TCP proxy.
 */
const MEDIA_HOST_URL = "";
/**
 * How long a split part has to finish what it's doing once told to stop: calls a gateway
 * holds while another part restarts wait up to 30 seconds.
 */
const DRAIN = 45;

/**
 * How fuwa.chat runs (see "Scaling out" in the README). `null` is one `fuwa` process
 * doing everything, how it ran until 2026-10. Set, as it is now, it runs as separate
 * parts, each a service here:
 *
 * - `fuwa`, the gateways clients reach at fuwa.chat. They keep nothing, so `gateways`
 *   replicas of it share the connections (2 or more, so one going down isn't noticed).
 * - `fuwa-directory`, with accounts, sessions, settings and pictures. It takes over the
 *   `fuwa-data` volume the single process used. Always one.
 * - `fuwa-shard-1` to `fuwa-shard-<shards>`, the community servers, each shard on a
 *   volume of its own. New servers go to the shard holding the fewest.
 * - `fuwa-media`, which carries calls' sound (docs/calls.md). Always one: an app's
 *   connection to it comes in through its TCP proxy, which reaches one replica.
 *
 * The first time it's set, the first shard to start takes the single process's servers
 * from the directory, so nothing has to be copied by hand. Raise `shards` to spread new
 * servers wider, but never lower it: removing a shard here deletes its volume, and the
 * servers on it.
 *
 * Deploys don't interrupt anyone. Gateways have no volume, so Railway starts the new ones,
 * waits until they're healthy (reaching the directory), then stops the old ones; clients'
 * live streams move over and carry on from where they were. The directory and shards each
 * have a volume, so Railway has to stop the old process before starting the new one; for
 * those few seconds the gateways hold calls (up to 30 seconds) and keep live streams open,
 * then carry on once the part is back. See "Deploys" in the README.
 */
const SPLIT: { gateways: number; shards: number } | null = { gateways: 2, shards: 2 };

/**
 * The home region's label (docs/regions.md): where the directory, the gateways and the
 * parts above run, and so where accounts, settings and direct messages are kept.
 */
const HOME = { id: "us-east", name: "US East" };

/**
 * Regions beyond the home one, for communities whose servers must be kept elsewhere
 * (docs/regions.md). Each gets `fuwa-<id>-shard-1` to `-<shards>` on volumes of their
 * own, a `fuwa-<id>-media` with its own TCP proxy for calls in that region, and a
 * `fuwa-<id>-replica` bucket, all in `railway` / `bucket`. Their servers' messages,
 * recordings and calls stay there; requests reach them through the home gateways.
 *
 * Empty, fuwa.chat is one region and runs exactly as before. Adding one deploys new
 * services, so it waits for Juan to say so. Railway can't move a service to another
 * region: a region is added, never moved, and removing one deletes its volumes and
 * every server on them (move them home first, in Settings > Instance > Servers).
 *
 * For example: `{ id: "eu", name: "Europe", railway: "europe-west4-drams3a", bucket: "ams", shards: 1 }`.
 * Railway regions: us-west2, us-east4-eqdc4a, europe-west4-drams3a, asia-southeast1-eqsg3a.
 * Bucket regions: sjc, iad, ams, sin.
 */
const REGIONS: { id: string; name: string; railway: string; bucket: string; shards: number }[] = [];

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

  // The split parts' continuous backup (docs/storage.md): the directory copies node.db
  // and the pictures here as they change, each shard its servers. Declared either way,
  // so turning SPLIT off never deletes it; only the split parts use it, since one
  // process refuses these settings. A part that starts on an empty volume restores
  // itself from it, and one over a bucket that has its files never starts empty.
  const replica = bucket("fuwa-replica", { region: BUCKET_REGION });
  const replicated = {
    FUWA_S3_ENDPOINT: ref(replica, "ENDPOINT"),
    FUWA_S3_REGION: ref(replica, "REGION"),
    FUWA_S3_BUCKET: ref(replica, "BUCKET"),
    FUWA_S3_ACCESS_KEY_ID: ref(replica, "ACCESS_KEY_ID"),
    FUWA_S3_SECRET_ACCESS_KEY: ref(replica, "SECRET_ACCESS_KEY"),
    FUWA_RESTORE: "if-empty",
  };

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

    return project("fuwa", { resources: [data, replica, fuwa] });
  }

  const { gateways, shards } = SPLIT;
  if (!Number.isInteger(gateways) || gateways < 1) {
    throw new Error("SPLIT.gateways must be a whole number, at least 1");
  }
  if (!Number.isInteger(shards) || shards < 1 || shards > MAX_VOLUMES - 1) {
    throw new Error(`SPLIT.shards must be a whole number from 1 to ${MAX_VOLUMES - 1}`);
  }
  for (const r of REGIONS) {
    if (!/^[a-z0-9-]{1,32}$/.test(r.id) || r.id === HOME.id || REGIONS.filter((o) => o.id === r.id).length > 1) {
      throw new Error(`region ${JSON.stringify(r.id)} needs a label of its own: 1 to 32 of a-z, 0-9 and -`);
    }
    if (!Number.isInteger(r.shards) || r.shards < 1) throw new Error(`region ${r.id} needs at least one shard`);
  }
  if (1 + shards + REGIONS.reduce((n, r) => n + r.shards, 0) > MAX_VOLUMES) {
    throw new Error(`the directory and every region's shards need at most ${MAX_VOLUMES} volumes`);
  }
  // Labels go on the parts only once there's more than one region, so a single-region
  // fuwa.chat keeps the settings it has always had.
  const label = (region: { id: string; name: string }) =>
    REGIONS.length ? { FUWA_REGION: region.id, FUWA_REGION_NAME: region.name } : {};

  // Every part listens on PORT on both IPv4 and IPv6, and the parts reach each other over
  // Railway's private network, sending the cluster key with every call.
  const part = (role: string) => ({
    PORT: String(PORT),
    FUWA_HOST: "::",
    FUWA_ROLE: role,
    FUWA_CLUSTER_KEY: ctx.shared.FUWA_CLUSTER_KEY,
  });
  const internalUrl = (name: string) => `http://\${{${name}.RAILWAY_PRIVATE_DOMAIN}}:${PORT}`;
  const mediaEnv = (region: { id: string; name: string }) => ({
    ...part("media"),
    ...label(region),
    FUWA_MEDIA_PORT: String(MEDIA_PORT),
    FUWA_MEDIA_ADDRESSES: "tcp/${{RAILWAY_TCP_PROXY_DOMAIN}}:${{RAILWAY_TCP_PROXY_PORT}}",
  });

  // Calls' sound. Railway has no public UDP, so apps reach it over TCP (ICE-TCP)
  // through a TCP proxy, whose address it hands them. It keeps nothing: on a deploy or
  // restart it tells every app in a call, and they join again on the new one with their
  // places kept (docs/calls.md, "Restarts").
  const media = MEDIA_HOST_URL
    ? []
    : [
        service("fuwa-media", {
          source: fuwaImage(),
          healthcheck: "/healthz",
          regions: { [REGION]: 1 },
          tcp: [MEDIA_PORT],
          deploy: { drainingSeconds: 5 },
          env: mediaEnv(HOME),
        }),
      ];
  // Where the directory (calls in direct messages) and shards (voice channels) open calls.
  // A media host gets its own key (shared variable FUWA_MEDIA_KEY), never the cluster key.
  const mediaUrl = MEDIA_HOST_URL
    ? { FUWA_MEDIA_URL: MEDIA_HOST_URL, FUWA_MEDIA_KEY: ctx.shared.FUWA_MEDIA_KEY }
    : { FUWA_MEDIA_URL: internalUrl("fuwa-media") };

  const directory = service("fuwa-directory", {
    source: fuwaImage(),
    healthcheck: "/healthz",
    // One replica: node.db is a file on its volume.
    regions: { [REGION]: 1 },
    volumeMounts: { "/data": data },
    // Calls still being answered when it's told to stop get time to finish.
    deploy: { drainingSeconds: DRAIN },
    env: {
      ...part("directory"),
      ...label(HOME),
      // The instance's own settings live with the directory, which hands them to every
      // other part, and it sends the usage signal.
      ...instance,
      FUWA_HOSTING: "hosted",
      FUWA_ENCRYPTION_KEY: ctx.shared.FUWA_ENCRYPTION_KEY,
      ...replicated,
      ...mediaUrl,
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
      deploy: { drainingSeconds: DRAIN },
      env: {
        ...part("shard"),
        ...label(HOME),
        // Never renamed: the directory knows which servers are on which shard by it.
        FUWA_SHARD_ID: `shard-${i + 1}`,
        FUWA_DIRECTORY_URL: internalUrl(directory.name),
        FUWA_INTERNAL_URL: internalUrl(name),
        // The same key as the directory's, so server files can move between shards.
        FUWA_ENCRYPTION_KEY: ctx.shared.FUWA_ENCRYPTION_KEY,
        ...replicated,
        ...mediaUrl,
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
    // On a deploy the old gateways keep serving for a little while after the new ones
    // are up, then tell open streams to follow again (they do, on a new gateway) and
    // finish the calls they're holding.
    deploy: { overlapSeconds: 10, drainingSeconds: DRAIN },
    env: {
      ...part("gateway"),
      ...instance,
      FUWA_DIRECTORY_URL: internalUrl(directory.name),
    },
  });

  // Each other region: its own shards, media part and bucket, joined to the home
  // directory over the private network (it spans regions within a project).
  const regionParts = REGIONS.flatMap((region) => {
    const regionReplica = bucket(`fuwa-${region.id}-replica`, { region: region.bucket });
    const regionMedia = service(`fuwa-${region.id}-media`, {
      source: fuwaImage(),
      healthcheck: "/healthz",
      regions: { [region.railway]: 1 },
      tcp: [MEDIA_PORT],
      deploy: { drainingSeconds: 5 },
      env: mediaEnv(region),
    });
    const regionShards = Array.from({ length: region.shards }, (_, i) => {
      const name = `fuwa-${region.id}-shard-${i + 1}`;
      const shardData = volume(`${name}-data`, { region: region.railway, sizeMB: VOLUME_MB });
      const shard = service(name, {
        source: fuwaImage(),
        healthcheck: "/healthz",
        regions: { [region.railway]: 1 },
        volumeMounts: { "/data": shardData },
        deploy: { drainingSeconds: DRAIN },
        env: {
          ...part("shard"),
          ...label(region),
          FUWA_SHARD_ID: `${region.id}-shard-${i + 1}`,
          FUWA_DIRECTORY_URL: internalUrl(directory.name),
          FUWA_INTERNAL_URL: internalUrl(name),
          FUWA_ENCRYPTION_KEY: ctx.shared.FUWA_ENCRYPTION_KEY,
          // Its own region's bucket, so its servers' copies stay there too.
          FUWA_S3_ENDPOINT: ref(regionReplica, "ENDPOINT"),
          FUWA_S3_REGION: ref(regionReplica, "REGION"),
          FUWA_S3_BUCKET: ref(regionReplica, "BUCKET"),
          FUWA_S3_ACCESS_KEY_ID: ref(regionReplica, "ACCESS_KEY_ID"),
          FUWA_S3_SECRET_ACCESS_KEY: ref(regionReplica, "SECRET_ACCESS_KEY"),
          FUWA_RESTORE: "if-empty",
          FUWA_MEDIA_URL: internalUrl(regionMedia.name),
        },
      });
      return [shardData, shard];
    });
    return [regionReplica, regionMedia, ...regionShards.flat()];
  });

  return project("fuwa", {
    resources: [data, replica, directory, ...shardParts.flat(), gateway, ...media, ...regionParts],
  });
});
