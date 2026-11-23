import { DEFAULT_PROMPTS, FEATURES, type Feature, FORMAT } from "./prompts.ts";

/** The wire formats an upstream model may speak: OpenAI chat/completions, OpenAI responses, Anthropic messages. */
const PROTOCOLS = ["chat", "responses", "messages"] as const;
export type Protocol = (typeof PROTOCOLS)[number];

export interface Upstream {
  protocol: Protocol;
  /** Up to and including the version segment, e.g. `https://api.openai.com/v1`. */
  baseUrl: string;
  apiKey: string;
  model: string;
}

export interface Config {
  /** `null` while no model is configured: the AI features are off. */
  upstream: Upstream | null;
  prompts: Record<Feature, string>;
}

/** What the administrator saved, falling back to AI_* in the environment for the model and to the built-in prompts. */
export function config(saved: any, env: NodeJS.ProcessEnv = process.env): Config {
  const from = saved?.baseUrl && saved?.model ? saved : { protocol: env.AI_PROTOCOL || "chat", baseUrl: env.AI_BASE_URL, apiKey: env.AI_API_KEY, model: env.AI_MODEL };
  const usable = from.baseUrl && from.model && PROTOCOLS.includes(from.protocol);
  return {
    upstream: usable ? { protocol: from.protocol, baseUrl: String(from.baseUrl).replace(/\/+$/, ""), apiKey: from.apiKey ?? "", model: from.model } : null,
    prompts: Object.fromEntries(FEATURES.map((f) => [f, (typeof saved?.prompts?.[f] === "string" && saved.prompts[f].trim()) || DEFAULT_PROMPTS[f]])) as Record<Feature, string>,
  };
}

/** Why `value` cannot be saved as the AI settings, or `null` when it can. An empty address switches the AI off. */
export function invalidSettings(value: any): string | null {
  if (!PROTOCOLS.includes(value?.protocol)) return "协议必须是 chat、responses 或 messages";
  if (typeof value.baseUrl !== "string" || value.baseUrl.length > 300 || (value.baseUrl && !/^https?:\/\/[^\s]+$/.test(value.baseUrl))) return "接口地址必须以 http:// 或 https:// 开头";
  if (typeof value.model !== "string" || value.model.length > 100 || (value.baseUrl && !value.model.trim())) return "请填写模型名称";
  if (value.apiKey !== undefined && (typeof value.apiKey !== "string" || value.apiKey.length > 500)) return "密钥过长";
  for (const feature of FEATURES) {
    const prompt = value.prompts?.[feature];
    if (prompt !== undefined && (typeof prompt !== "string" || prompt.length > 8000)) return "提示词最多 8000 个字符";
  }
  return null;
}

export const isFeature = (value: unknown): value is Feature => FEATURES.includes(value as Feature);

/**
 * The facts a client collected, as the text the model reads; `null` when they are not what a client sends.
 * Names are plain identifiers and sizes are capped, so the request cannot be used to relay arbitrary prompts at will.
 */
export function factsText(facts: unknown): string | null {
  if (typeof facts !== "object" || facts === null || Array.isArray(facts)) return null;
  const entries = Object.entries(facts);
  if (!entries.length || entries.length > 30) return null;
  const lines: string[] = [];
  for (const [name, value] of entries) {
    if (!/^[a-z_]{1,32}$/.test(name)) return null;
    if (typeof value === "string" && value.length <= 2000) {
      if (value.trim()) lines.push(`${name}: ${value.trim()}`);
    } else if (Array.isArray(value) && value.length <= 60 && value.every((v) => typeof v === "string" && v.length <= 300)) {
      if (value.length) lines.push(`${name}:`, ...value.map((v) => `- ${v}`));
    } else return null;
  }
  const text = lines.join("\n");
  return text && text.length <= 12_000 ? text : null;
}

export function request(up: Upstream, system: string, user: string): { url: string; headers: Record<string, string>; body: unknown } {
  const bearer = { "content-type": "application/json", authorization: `Bearer ${up.apiKey}` };
  switch (up.protocol) {
    case "chat":
      return {
        url: `${up.baseUrl}/chat/completions`,
        headers: bearer,
        body: { model: up.model, messages: [{ role: "system", content: system }, { role: "user", content: user }] },
      };
    case "responses":
      return { url: `${up.baseUrl}/responses`, headers: bearer, body: { model: up.model, instructions: system, input: user } };
    case "messages":
      return {
        url: `${up.baseUrl}/messages`,
        headers: { "content-type": "application/json", "x-api-key": up.apiKey, "anthropic-version": "2023-06-01" },
        body: { model: up.model, max_tokens: 1500, system, messages: [{ role: "user", content: user }] },
      };
  }
}

/** The text of an upstream answer; empty when it carries none. */
export function textOf(protocol: Protocol, answer: any): string {
  const texts = (parts: any, type: string): string[] =>
    Array.isArray(parts) ? parts.filter((p) => p?.type === type && typeof p.text === "string").map((p) => p.text) : [];
  let text: unknown;
  if (protocol === "chat") text = answer?.choices?.[0]?.message?.content;
  else if (protocol === "messages") text = texts(answer?.content, "text").join("");
  else text = (Array.isArray(answer?.output) ? answer.output : []).flatMap((item: any) => texts(item?.content, "output_text")).join("");
  return typeof text === "string" ? text.trim() : "";
}

export interface Answer {
  summary: string;
  verdict: "safe" | "caution" | "keep" | "unknown";
  confidence: "high" | "medium" | "low";
  points: string[];
}

/** What the model said, in the shape the client shows. Text that is not the requested JSON becomes the summary of an "unknown". */
export function parseAnswer(text: string): Answer {
  const plain: Answer = { summary: text.trim().slice(0, 800), verdict: "unknown", confidence: "low", points: [] };
  const [from, to] = [text.indexOf("{"), text.lastIndexOf("}")];
  if (from < 0 || to < from) return plain;
  let parsed: any;
  try {
    parsed = JSON.parse(text.slice(from, to + 1));
  } catch {
    return plain;
  }
  if (typeof parsed?.summary !== "string" || !parsed.summary.trim()) return plain;
  const one = <T extends string>(value: unknown, allowed: readonly T[], otherwise: T): T => (allowed.includes(String(value).trim() as T) ? (String(value).trim() as T) : otherwise);
  return {
    summary: parsed.summary.trim().slice(0, 800),
    verdict: one(parsed.verdict, ["safe", "caution", "keep", "unknown"] as const, "unknown"),
    confidence: one(parsed.confidence, ["high", "medium", "low"] as const, "low"),
    points: (Array.isArray(parsed.points) ? parsed.points : []).filter((p: unknown) => typeof p === "string" && p.trim()).slice(0, 5).map((p: string) => p.trim().slice(0, 400)),
  };
}

/** Asks the model about `facts` (already as text) for one feature. Throws with the upstream's own words. */
export async function ask(up: Upstream, prompt: string, language: string, facts: string): Promise<Answer> {
  const { url, headers, body } = request(up, `${prompt.trim()}\n\n${FORMAT(language)}`, facts);
  const response = await fetch(url, { method: "POST", headers, body: JSON.stringify(body), signal: AbortSignal.timeout(90_000) });
  const raw = await response.text();
  if (!response.ok) throw new Error(`upstream answered ${response.status}: ${raw.slice(0, 500)}`);
  let text = "";
  try {
    text = textOf(up.protocol, JSON.parse(raw));
  } catch {
    /* not JSON: reported below */
  }
  if (!text) throw new Error(`upstream answer has no text: ${raw.slice(0, 500)}`);
  return parseAnswer(text);
}

/** Why `value` cannot be published as the current release, or `null` when it can. */
export function invalidRelease(value: any): string | null {
  if (typeof value?.version !== "string" || !/^\d+\.\d+\.\d+$/.test(value.version)) return "version must look like 1.2.3";
  // Clients open this address in a browser, so it must be one that cannot be tampered with on the way.
  if (typeof value.url !== "string" || !/^https:\/\/[^\s]+$/.test(value.url) || value.url.length > 500) return "url must be an https address";
  // Made with the updater key when the installer is built; clients refuse an installer it does not match.
  if (typeof value.signature !== "string" || !/^[A-Za-z0-9+/=\s]{40,4000}$/.test(value.signature)) return "signature is required (the contents of the installer's .sig file)";
  if (value.notes !== undefined && (typeof value.notes !== "string" || value.notes.length > 2000)) return "notes must be text of at most 2000 characters";
  return null;
}
