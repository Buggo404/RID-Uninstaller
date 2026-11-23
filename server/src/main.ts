import { createHash, timingSafeEqual } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { extname, join } from "node:path";
import rateLimit from "@fastify/rate-limit";
import Fastify, { type FastifyReply, type FastifyRequest } from "fastify";
import { ask, config, factsText, invalidRelease, invalidSettings, isFeature } from "./ai.ts";
import { DEFAULT_PROMPTS, SAMPLES } from "./prompts.ts";
import { type Admin, captchaPasses, changeCredentials, closeSession, cookieOf, ensureAdmin, invalidCredentials, newCaptcha, openSession, sessionAdmin, signIn } from "./auth.ts";
import { answersToday, cachedAnswer, find, migrate, pool, programs, remove, replace, saveAnswer, setSetting, setting } from "./db.ts";
import { invalid, isKey } from "./rules.ts";

const ADMIN_TOKEN = process.env.ADMIN_TOKEN ?? "";
if (ADMIN_TOKEN.length < 24) throw new Error("ADMIN_TOKEN must be set to at least 24 characters");
/** The AI settings as they are now; read per request so that a change in the web interface applies at once. */
const aiConfig = async () => config(await setting("ai"));
const COOKIE = "rid_admin";
/** Questions sent on to the model per 24 hours, over all clients: a ceiling on what abuse can cost. */
const DAILY_LIMIT = Number(process.env.AI_DAILY_LIMIT) || 3000;

// trustProxy: rate limits count the real client address when running behind a reverse proxy.
const app = Fastify({ logger: true, trustProxy: true, bodyLimit: 256 * 1024 });
await app.register(rateLimit, { max: 120, timeWindow: "1 minute" });

app.addHook("onRequest", async (request, reply) => {
  reply.header("X-Content-Type-Options", "nosniff");
  reply.header("X-Frame-Options", "DENY");
  reply.header("Referrer-Policy", "no-referrer");
  // Nothing is kept by a CDN or proxy unless its route says so: one visitor's answer must never reach another.
  reply.header("Cache-Control", "no-store");
  // A page on another site must not be able to act with the administrator's session.
  const origin = request.headers.origin;
  if (origin && request.method !== "GET" && (!URL.canParse(origin) || new URL(origin).host !== request.headers.host)) {
    return reply.code(403).send({ error: "cross-site request refused" });
  }
});

function hasToken(header: string | undefined): boolean {
  const given = Buffer.from(header ?? "");
  const wanted = Buffer.from(`Bearer ${ADMIN_TOKEN}`);
  return given.length === wanted.length && timingSafeEqual(given, wanted);
}

const signedIn = (request: FastifyRequest) => sessionAdmin(cookieOf(request.headers.cookie, COOKIE));

/** The token, or a signed-in administrator who is past the forced change of the initial password. */
async function mayManage(request: FastifyRequest): Promise<boolean> {
  return hasToken(request.headers.authorization) || (await signedIn(request))?.must_change === false;
}

const refuse = (reply: FastifyReply) => reply.code(401).send({ error: "administrator sign-in required" });

function setCookie(request: FastifyRequest, reply: FastifyReply, token: string | null) {
  const secure = request.protocol === "https" ? "; Secure" : "";
  reply.header("Set-Cookie", `${COOKIE}=${token ?? ""}; Path=/; HttpOnly; SameSite=Strict; Max-Age=${token ? 12 * 3600 : 0}${secure}`);
}

app.get("/healthz", async () => {
  await pool.query("SELECT 1");
  const { upstream } = await aiConfig();
  return { ok: true, ai: upstream ? { protocol: upstream.protocol, model: upstream.model } : null };
});

// Public: what the client asks

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

/** The release clients are offered, in the shape the client's updater reads; 204 until one is published. */
app.get("/v1/update", async (_request, reply) => {
  reply.header("Cache-Control", "public, max-age=300");
  return (await setting("release")) ?? reply.code(204).send();
});

// The model costs far more per call than a database lookup, hence the tighter limit.
app.post<{ Body: { feature?: unknown; lang?: unknown; facts?: unknown } }>(
  "/v1/ai/ask",
  { config: { rateLimit: { max: 10, timeWindow: "1 minute" } } },
  async (request, reply) => {
    const { feature, lang, facts } = request.body ?? {};
    const text = factsText(facts);
    if (!isFeature(feature) || !text) return reply.code(400).send({ error: "not a question this server answers" });
    const { upstream, prompts } = await aiConfig();
    if (!upstream) return reply.code(503).send({ error: "no model is configured on this server" });
    const language = lang === "zh" ? "简体中文" : "English";
    // The same question about the same program is answered once: most people ask about the same programs.
    const key = createHash("sha256").update(JSON.stringify([feature, language, upstream.model, prompts[feature], text])).digest("hex");
    const known = await cachedAnswer(key);
    if (known) return known;
    if ((await answersToday()) >= DAILY_LIMIT) return reply.code(429).send({ error: "the daily limit is reached" });
    try {
      const answer = await ask(upstream, prompts[feature], language, text);
      await saveAnswer(key, answer);
      return answer;
    } catch (e) {
      // The upstream's own words stay in the log; they may quote the key or internal addresses.
      request.log.error(e, "model request failed");
      return reply.code(502).send({ error: "the model did not answer" });
    }
  },
);

// Management: the token (scripts) or a signed-in administrator (web interface)

app.put<{ Params: { key: string }; Body: { name?: unknown; rules?: unknown } }>("/v1/rules/:key", async (request, reply) => {
  if (!(await mayManage(request))) return refuse(reply);
  const { key } = request.params;
  const { name, rules } = request.body ?? {};
  if (!isKey(key)) return reply.code(400).send({ error: "not a program key" });
  if (typeof name !== "string" || !name.trim() || name.length > 200) return reply.code(400).send({ error: "name is required" });
  if (!Array.isArray(rules) || rules.length > 500) return reply.code(400).send({ error: "rules must be a list of at most 500 entries" });
  const problem = rules.map(invalid).find(Boolean);
  if (problem) return reply.code(400).send({ error: problem });
  return { key, revision: await replace(key, name.trim(), rules) };
});

app.delete<{ Params: { key: string } }>("/v1/rules/:key", async (request, reply) => {
  if (!(await mayManage(request))) return refuse(reply);
  return (await remove(request.params.key)) ? { ok: true } : reply.code(404).send({ error: "no rules for this program" });
});

app.put<{ Body: { version?: unknown; url?: unknown; notes?: unknown } }>("/v1/update", async (request, reply) => {
  if (!(await mayManage(request))) return refuse(reply);
  const problem = invalidRelease(request.body);
  if (problem) return reply.code(400).send({ error: problem });
  const { version, url, signature, notes = "" } = request.body as Record<string, string>;
  await setSetting("release", { version, url, signature: signature.trim(), notes });
  return { version };
});

// The web interface's own requests. Nothing here may be cached or kept by a proxy.

const strict = { config: { rateLimit: { max: 10, timeWindow: "1 minute" } } };
const describe = (admin: Admin) => ({ username: admin.username, mustChange: admin.must_change });

app.get("/admin/api/captcha", { config: { rateLimit: { max: 30, timeWindow: "1 minute" } } }, async () => newCaptcha());

app.post<{ Body: { username?: unknown; password?: unknown; captchaId?: unknown; captcha?: unknown } }>("/admin/api/login", strict, async (request, reply) => {
  const { username, password, captchaId, captcha } = request.body ?? {};
  if (!captchaPasses(captchaId, captcha)) return reply.code(400).send({ error: "验证码不正确" });
  if (typeof username !== "string" || typeof password !== "string" || password.length > 200) return reply.code(400).send({ error: "用户名或密码不正确" });
  const admin = await signIn(username, password);
  if (admin === "locked") return reply.code(429).send({ error: "尝试次数过多，请 15 分钟后再试" });
  if (!admin) return reply.code(401).send({ error: "用户名或密码不正确" });
  setCookie(request, reply, await openSession(admin.id));
  return describe(admin);
});

app.post("/admin/api/logout", async (request, reply) => {
  await closeSession(cookieOf(request.headers.cookie, COOKIE));
  setCookie(request, reply, null);
  return { ok: true };
});

app.get("/admin/api/me", async (request, reply) => {
  const admin = await signedIn(request);
  return admin ? describe(admin) : refuse(reply);
});

app.post<{ Body: { currentPassword?: unknown; username?: unknown; password?: unknown } }>("/admin/api/account", strict, async (request, reply) => {
  const admin = await signedIn(request);
  if (!admin) return refuse(reply);
  const { currentPassword, username, password } = request.body ?? {};
  const problem = invalidCredentials(username, password) ?? (typeof currentPassword === "string" ? null : "当前密码不正确");
  if (problem) return reply.code(400).send({ error: problem });
  const failure = await changeCredentials(admin, currentPassword as string, username as string, password as string);
  if (failure) return reply.code(400).send({ error: failure });
  // Every session was closed with the old password; this browser gets a fresh one.
  setCookie(request, reply, await openSession(admin.id));
  return { username, mustChange: false };
});

/** The AI settings for the web interface. The key itself is never sent back, only whether one is saved. */
app.get("/admin/api/ai", async (request, reply) => {
  if (!(await mayManage(request))) return refuse(reply);
  const saved = ((await setting("ai")) ?? {}) as Record<string, any>;
  const { upstream, prompts } = config(saved);
  return { protocol: upstream?.protocol ?? "chat", baseUrl: upstream?.baseUrl ?? "", model: upstream?.model ?? "", hasKey: !!upstream?.apiKey, fromEnvironment: !!upstream && !saved.baseUrl, prompts, defaults: DEFAULT_PROMPTS, today: await answersToday(), dailyLimit: DAILY_LIMIT };
});

app.put<{ Body: Record<string, any> }>("/admin/api/ai", async (request, reply) => {
  if (!(await mayManage(request))) return refuse(reply);
  const problem = invalidSettings(request.body);
  if (problem) return reply.code(400).send({ error: problem });
  const saved = ((await setting("ai")) ?? {}) as Record<string, any>;
  const { protocol, baseUrl, model, apiKey, prompts = {} } = request.body;
  // An empty key field means "leave the saved key as it is": the page never has it to send back.
  await setSetting("ai", { protocol, baseUrl: baseUrl.trim(), model: model.trim(), apiKey: apiKey || saved.apiKey || "", prompts });
  return { ok: true };
});

/** Runs one feature against the configured model with built-in sample facts. The administrator gets the upstream's own error. */
app.post<{ Body: { feature?: unknown } }>("/admin/api/ai/test", strict, async (request, reply) => {
  if (!(await mayManage(request))) return refuse(reply);
  const { feature } = request.body ?? {};
  if (!isFeature(feature)) return reply.code(400).send({ error: "未知的功能" });
  const { upstream, prompts } = await aiConfig();
  if (!upstream) return reply.code(400).send({ error: "还没有配置模型" });
  try {
    return await ask(upstream, prompts[feature], "简体中文", factsText(SAMPLES[feature])!);
  } catch (e) {
    return reply.code(502).send({ error: String((e as Error).message ?? e) });
  }
});

app.get("/admin/api/programs", async (request, reply) => ((await mayManage(request)) ? programs() : refuse(reply)));

// The built web interface (web/dist), read once at start.

const WEB = join(import.meta.dirname, "../web/dist");
const TYPES: Record<string, string> = { ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".ico": "image/x-icon" };
if (existsSync(WEB)) {
  const files = new Map<string, { body: Buffer; type: string }>();
  for (const name of readdirSync(WEB, { recursive: true }) as string[]) {
    const type = TYPES[extname(name)];
    if (type) files.set(name.replaceAll("\\", "/"), { body: readFileSync(join(WEB, name)), type });
  }
  const page = (request: FastifyRequest<{ Params: { "*"?: string } }>, reply: FastifyReply) => {
    const asset = files.get(request.params["*"] ?? "");
    const file = asset ?? files.get("index.html")!;
    // Scripts and styles only from this server; nothing may frame the page or receive its forms.
    reply.header("Content-Security-Policy", "default-src 'self'; img-src 'self' data:; frame-ancestors 'none'; form-action 'self'; base-uri 'none'");
    reply.header("Cache-Control", asset && request.params["*"]?.startsWith("assets/") ? "public, max-age=31536000, immutable" : "no-cache");
    return reply.type(file.type).send(file.body);
  };
  app.get("/admin", page);
  app.get("/admin/*", page);
}

await migrate();
await ensureAdmin();
await app.listen({ host: "0.0.0.0", port: Number(process.env.PORT ?? 8080) });

for (const signal of ["SIGTERM", "SIGINT"] as const) {
  process.once(signal, async () => {
    await app.close();
    await pool.end();
  });
}
