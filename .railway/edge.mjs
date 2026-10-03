// Railway's CDN and edge rules for the public services, which railway.ts can't declare
// yet. `node .railway/edge.mjs check` (on pull requests) shows what's set; it changes
// nothing. RAILWAY_TOKEN is the project token the config workflow already uses.
import { readFile } from "node:fs/promises";

const API = "https://backboard.railway.com/graphql/v2";
const token = process.env.RAILWAY_TOKEN;
if (!token) throw new Error("RAILWAY_TOKEN is not set");
const SERVICES = (process.argv[3] ?? "").split(",").filter(Boolean);

async function gql(query, variables = {}) {
  const response = await fetch(API, {
    method: "POST",
    headers: { "content-type": "application/json", "project-access-token": token },
    body: JSON.stringify({ query, variables }),
  });
  const body = await response.json();
  if (body.errors) throw new Error(JSON.stringify(body.errors));
  return body.data;
}

const fields = async (name) =>
  (await gql(`query($n: String!) { __type(name: $n) { name kind inputFields { name type { name kind ofType { name kind ofType { name kind } } } } fields { name type { name kind ofType { name kind ofType { name kind } } } } enumValues { name } } }`, { n: name })).__type;

const { projectToken } = await gql(`{ projectToken { projectId environmentId } }`);
console.log("environment", projectToken.environmentId);

for (const type of ["UpdateServiceEdgeConfigInput", "EnableServiceCdnInput", "ServiceEdgeConfig", "EdgeConfig"]) {
  try {
    console.log(type, JSON.stringify(await fields(type)));
  } catch (err) {
    console.log(type, "unavailable:", String(err).slice(0, 300));
  }
}
try {
  const mutation = await fields("Mutation");
  console.log("mutations", JSON.stringify(mutation?.fields?.map((f) => f.name).filter((n) => /edge|cdn|waf|rule/i.test(n))));
} catch (err) {
  console.log("mutations unavailable:", String(err).slice(0, 300));
}

const { project } = await gql(`query($id: String!) { project(id: $id) { services { edges { node { id name } } } } }`, { id: projectToken.projectId });
for (const { node } of project.services.edges.filter(({ node }) => SERVICES.includes(node.name))) {
  try {
    const { serviceInstance } = await gql(
      `query($e: String!, $s: String!) { serviceInstance(environmentId: $e, serviceId: $s) { edgeConfig { id enabled caching { mode defaultTtlSeconds htmlCaching purgeOnDeploy } } } }`,
      { e: projectToken.environmentId, s: node.id },
    );
    console.log(node.name, JSON.stringify(serviceInstance.edgeConfig));
  } catch (err) {
    console.log(node.name, "edge config unavailable:", String(err).slice(0, 300));
  }
}
console.log("ruleset to apply:", JSON.parse(await readFile(new URL("./edge-rules.json", import.meta.url), "utf8")).rules.length, "rule(s)");
