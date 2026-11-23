import { timingSafeEqual } from "node:crypto";
import rateLimit from "@fastify/rate-limit";
import Fastify from "fastify";
import { find, migrate, pool, replace } from "./db.ts";
import { invalid, isKey } from "./rules.ts";

const ADMIN_TOKEN = process.env.ADMIN_TOKEN ?? "";
if (ADMIN_TOKEN.length < 24) throw new Error("ADMIN_TOKEN must be set to at least 24 characters");

// trustProxy: rate limits count the real client address when running behind a reverse proxy.
const app = Fastify({ logger: true, trustProxy: true, bodyLimit: 256 * 1024 });
await app.register(rateLimit, { max: 120, timeWindow: "1 minute" });

function isAdmin(header: string | undefined): boolean {
  const given = Buffer.from(header ?? "");
  const wanted = Buffer.from(`Bearer ${ADMIN_TOKEN}`);
  return given.length === wanted.length && timingSafeEqual(given, wanted);
}

app.get("/healthz", async () => {
  await pool.query("SELECT 1");
  return { ok: true };
});

app.get<{ Params: { key: string } }>("/v1/rules/:key", async (request, reply) => {
  const { key } = request.params;
  if (!isKey(key)) return reply.code(400).send({ error: "not a program key" });
  const found = await find(key);
  // "No rules" is cacheable too: most programs have none and clients should not keep asking.
  reply.header("Cache-Control", "public, max-age=3600");
  if (!found) return reply.code(404).send({ error: "no rules for this program" });
  const etag = `"${found.revision}"`;
  reply.header("ETag", etag);
  if (request.headers["if-none-match"] === etag) return reply.code(304).send();
  return found;
});

app.put<{ Params: { key: string }; Body: { name?: unknown; rules?: unknown } }>("/v1/rules/:key", async (request, reply) => {
  if (!isAdmin(request.headers.authorization)) return reply.code(401).send({ error: "administrator token required" });
  const { key } = request.params;
  const { name, rules } = request.body ?? {};
  if (!isKey(key)) return reply.code(400).send({ error: "not a program key" });
  if (typeof name !== "string" || !name.trim() || name.length > 200) return reply.code(400).send({ error: "name is required" });
  if (!Array.isArray(rules) || rules.length > 500) return reply.code(400).send({ error: "rules must be a list of at most 500 entries" });
  const problem = rules.map(invalid).find(Boolean);
  if (problem) return reply.code(400).send({ error: problem });
  return { key, revision: await replace(key, name.trim(), rules) };
});

await migrate();
await app.listen({ host: "0.0.0.0", port: Number(process.env.PORT ?? 8080) });

for (const signal of ["SIGTERM", "SIGINT"] as const) {
  process.once(signal, async () => {
    await app.close();
    await pool.end();
  });
}
