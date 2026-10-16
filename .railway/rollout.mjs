// Deploys fuwa.chat onto the image just published, one part at a time, so nobody notices
// (.github/workflows/publish.yml runs it after tagging :latest).
//
//   node --experimental-strip-types .railway/rollout.mjs plan   the order it would go in; deploys nothing
//   node --experimental-strip-types .railway/rollout.mjs        deploys
//
// Redeploying every part at once (what this replaced) stopped every shard together, and a
// shard with a volume can take minutes to come back while Railway moves its volume over;
// its servers' calls failed after the 30 seconds gateways wait. Now:
//
// 1. Media parts, every region at once. They keep nothing, so Railway starts the new one
//    before stopping the old, which tells every app in a call to join again on it.
// 2. Shards, one at a time in each region (the regions side by side). A shard told to stop
//    hands its servers to the other shards in its region first (FUWA_HANDOFF), their calls
//    and live streams carried on there, so its volume can take as long as it likes. The
//    shard deployed last time goes first: it ended up with the fewest servers, since the
//    others handed theirs to it, so every server moves about once per rollout.
// 3. The directory, which still restarts in place: gateways hold calls for it meanwhile,
//    and live streams and calls to servers don't need it.
// 4. Gateways last, so clients only reach the new version once everything behind it runs.
//
// Each step waits until Railway says the new deployment is up (healthy: a shard only once
// it has registered with the directory) before the next. A deployment that fails stops the
// rollout there; what's left keeps running the image before, and the next merge tries
// again. RAILWAY_TOKEN is the project token for the "fuwa" project's production
// environment.
import { execFileSync } from "node:child_process";

const API = "https://backboard.railway.com/graphql/v2";
/** The Railway CLI that triggers a redeploy, pinned like the config workflow's. */
const CLI = "@railway/cli@5.63.1";
/** How long one part may take to be up again: a shard's hand-off, its volume, its start. */
const PART_TIMEOUT_MS = 20 * 60 * 1000;
/** A shard that's just up has registered, but give its region a moment before the next. */
const SETTLE_MS = 10 * 1000;
const POLL_MS = 5 * 1000;

const mode = process.argv[2] ?? "deploy";
if (mode !== "plan" && mode !== "deploy") throw new Error("usage: rollout.mjs [plan]");
const token = process.env.RAILWAY_TOKEN;
if (!token) throw new Error("RAILWAY_TOKEN is not set");
const IMAGE = process.env.IMAGE ?? "ghcr.io/waifu-devs/fuwa";

async function gql(query, variables = {}) {
  const response = await fetch(API, {
    method: "POST",
    headers: { "content-type": "application/json", "project-access-token": token },
    body: JSON.stringify({ query, variables }),
  });
  const body = await response.json();
  if (!response.ok || body.errors) throw new Error(`Railway API: ${response.status} ${JSON.stringify(body.errors ?? body)}`);
  return body.data;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const log = (...args) => console.log(new Date().toISOString().slice(11, 19), ...args);

// Every service the config runs fuwa's image on, with its part and region.
const { createRailwayContext, project } = await import("railway/iac");
const { default: config } = await import("./railway.ts");
const { resources } = await config(createRailwayContext({ environment: "production" }), project);
const parts = resources
  .flat(Infinity)
  .filter((r) => r.type === "service" && r.source?.image?.startsWith(`${IMAGE}:`))
  .map((r) => ({
    name: r.name,
    role: r.variables?.FUWA_ROLE?.value ?? "all",
    region: r.variables?.FUWA_REGION?.value ?? "",
  }));

const { projectToken } = await gql(`{ projectToken { projectId environmentId } }`);
const environmentId = projectToken.environmentId;
const { project: railway } = await gql(`query($id: String!) { project(id: $id) { services { edges { node { id name } } } } }`, {
  id: projectToken.projectId,
});
const ids = new Map(railway.services.edges.map(({ node }) => [node.name, node.id]));
for (const part of parts) {
  part.id = ids.get(part.name);
  if (!part.id) throw new Error(`no service named ${part.name} in this project; apply the config first`);
}

/** The service's newest deployment: { id, status, createdAt }, or null. */
async function latest(part) {
  const data = await gql(
    `query($e: String!, $s: String!) {
      deployments(first: 1, input: { environmentId: $e, serviceId: $s }) { edges { node { id status createdAt } } }
    }`,
    { e: environmentId, s: part.id },
  );
  return data.deployments.edges[0]?.node ?? null;
}

for (const part of parts) part.deployedAt = (await latest(part))?.createdAt ?? "";

// The order: media, then each region's shards (the most recently deployed first), then the
// directory (or the one process), then gateways.
const by = (role) => parts.filter((p) => p.role === role);
const regions = new Map();
for (const shard of by("shard")) {
  if (!regions.has(shard.region)) regions.set(shard.region, []);
  regions.get(shard.region).push(shard);
}
for (const shards of regions.values()) {
  shards.sort((a, b) => b.deployedAt.localeCompare(a.deployedAt) || a.name.localeCompare(b.name));
}
const steps = [
  { title: "media parts", lanes: by("media").map((p) => [p]) },
  { title: "shards", lanes: [...regions.values()] },
  { title: "directory", lanes: [...by("directory"), ...by("all")].map((p) => [p]) },
  { title: "gateways", lanes: by("gateway").map((p) => [p]) },
].filter((step) => step.lanes.length);

for (const step of steps) {
  console.log(`${step.title}: ${step.lanes.map((lane) => lane.map((p) => p.name).join(" → ")).join(" | ")}`);
}
for (const [region, shards] of regions) {
  if (shards.length < 2) {
    console.log(`warning: ${region || "the home region"} has one shard, so its servers wait while it restarts`);
  }
}
if (mode === "plan") process.exit(0);

/** Redeploys one part onto the newest image and waits until the new deployment is up. */
async function deploy(part) {
  const before = (await latest(part))?.id;
  log(`${part.name}: deploying`);
  // --from-source pulls :latest again; a plain redeploy would rerun the old image.
  execFileSync("npx", ["--yes", CLI, "redeploy", "--service", part.name, "--from-source", "--yes"], {
    stdio: "inherit",
  });
  const until = Date.now() + PART_TIMEOUT_MS;
  let status = "";
  while (Date.now() < until) {
    await sleep(POLL_MS);
    const now = await latest(part);
    if (!now || now.id === before) continue;
    if (now.status !== status) log(`${part.name}: ${(status = now.status).toLowerCase()}`);
    if (now.status === "SUCCESS") return;
    if (["FAILED", "CRASHED", "REMOVED", "SKIPPED"].includes(now.status)) {
      throw new Error(`${part.name}'s deployment ${now.id} ended ${now.status}; stopping the rollout here`);
    }
  }
  throw new Error(`${part.name} wasn't up after ${PART_TIMEOUT_MS / 60000} minutes; stopping the rollout here`);
}

for (const step of steps) {
  log(`— ${step.title}`);
  // Lanes side by side, each one's parts in turn.
  await Promise.all(
    step.lanes.map(async (lane) => {
      for (const part of lane) {
        await deploy(part);
        if (part.role === "shard") await sleep(SETTLE_MS);
      }
    }),
  );
}
log("every part runs the new image");
