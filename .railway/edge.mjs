// Railway's CDN and edge rules for the public services, which railway.ts can't declare
// yet, set through Railway's API by the config workflow (.github/workflows/railway-config.yml).
//
//   node .railway/edge.mjs check fuwa   what apply would change; changes nothing (pull requests)
//   node .railway/edge.mjs apply fuwa   turns the CDN on and sets edge-rules.json (on merge, after the config apply)
//
// The CDN gets Railway's defaults: HTML cached only when a page says so, a 2 hour
// fallback for assets that don't, cached HTML dropped on each deploy. Neither change
// deploys or restarts anything. RAILWAY_TOKEN is the project token the workflow already uses.
import { readFile } from "node:fs/promises";

const API = "https://backboard.railway.com/graphql/v2";
const [mode, list = ""] = process.argv.slice(2);
if (mode !== "check" && mode !== "apply") throw new Error("usage: edge.mjs check|apply <service,service...>");
const token = process.env.RAILWAY_TOKEN;
if (!token) throw new Error("RAILWAY_TOKEN is not set");
const wanted = list.split(",").filter(Boolean);
const rules = JSON.parse(await readFile(new URL("./edge-rules.json", import.meta.url), "utf8"));

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

/** JSON with every object's keys sorted: Railway hands the ruleset back in its own key order. */
const canonical = (value) =>
  Array.isArray(value)
    ? `[${value.map(canonical).join(",")}]`
    : value && typeof value === "object"
      ? `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`
      : JSON.stringify(value);

/** A ruleset without the ids Railway gives each rule, to compare with ours. */
const comparable = (ruleset) =>
  ruleset && canonical({ ...ruleset, rules: (ruleset.rules ?? []).map(({ id: _, ...rule }) => rule) });

const EDGE = `edgeConfig { enabled edgeRules caching { mode htmlCaching defaultTtlSeconds purgeOnDeploy } }`;

const { projectToken } = await gql(`{ projectToken { projectId environmentId } }`);
const environmentId = projectToken.environmentId;
const { project } = await gql(`query($id: String!) { project(id: $id) { services { edges { node { id name } } } } }`, {
  id: projectToken.projectId,
});
const services = project.services.edges.map(({ node }) => node);
const missing = wanted.filter((name) => !services.some((s) => s.name === name));
if (missing.length) throw new Error(`no service named ${missing.join(", ")} in this project`);

for (const name of wanted) {
  const serviceId = services.find((s) => s.name === name).id;
  const read = async () =>
    (await gql(`query($e: String!, $s: String!) { serviceInstance(environmentId: $e, serviceId: $s) { ${EDGE} } }`, { e: environmentId, s: serviceId }))
      .serviceInstance.edgeConfig;
  const edge = await read();
  const cdnOff = !edge?.caching;
  const rulesDiffer = comparable(edge?.edgeRules) !== comparable(rules);
  console.log(`${name}: CDN ${cdnOff ? "off" : `on (${JSON.stringify(edge.caching)})`}, edge rules ${rulesDiffer ? "differ from edge-rules.json" : "match edge-rules.json"}`);

  if (mode === "check") {
    if (cdnOff) console.log(`${name}: merging turns the CDN on with Railway's defaults`);
    if (rulesDiffer) console.log(`${name}: merging sets the edge rules to edge-rules.json`);
    continue;
  }
  if (cdnOff) {
    await gql(`mutation($input: EnableServiceCdnInput!) { enableServiceCdn(input: $input) { enabled } }`, {
      input: { environmentId, serviceId },
    });
  }
  if (rulesDiffer) {
    await gql(`mutation($input: UpdateServiceEdgeRulesInput!) { updateServiceEdgeRules(input: $input) { enabled } }`, {
      input: { environmentId, serviceId, edgeRules: rules },
    });
  }
  const after = await read();
  if (!after?.caching || comparable(after.edgeRules) !== comparable(rules)) {
    throw new Error(`${name}: Railway didn't keep the CDN or edge rules: ${JSON.stringify(after)}`);
  }
  console.log(`${name}: CDN on (${JSON.stringify(after.caching)}), edge rules set`);
}
