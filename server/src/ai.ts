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

/** The upstream named by AI_PROTOCOL, AI_BASE_URL, AI_API_KEY and AI_MODEL; `null` while it is not configured. */
export function upstream(env: NodeJS.ProcessEnv = process.env): Upstream | null {
  const protocol = env.AI_PROTOCOL || "chat";
  if (!env.AI_BASE_URL || !env.AI_MODEL) return null;
  if (!PROTOCOLS.includes(protocol as Protocol)) throw new Error(`AI_PROTOCOL must be one of: ${PROTOCOLS.join(", ")}`);
  return { protocol: protocol as Protocol, baseUrl: env.AI_BASE_URL.replace(/\/+$/, ""), apiKey: env.AI_API_KEY ?? "", model: env.AI_MODEL };
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
        body: { model: up.model, max_tokens: 1024, system, messages: [{ role: "user", content: user }] },
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

export async function ask(up: Upstream, system: string, user: string): Promise<string> {
  const { url, headers, body } = request(up, system, user);
  const response = await fetch(url, { method: "POST", headers, body: JSON.stringify(body), signal: AbortSignal.timeout(60_000) });
  const raw = await response.text();
  if (!response.ok) throw new Error(`upstream answered ${response.status}: ${raw.slice(0, 500)}`);
  const text = textOf(up.protocol, JSON.parse(raw));
  if (!text) throw new Error(`upstream answer has no text: ${raw.slice(0, 500)}`);
  return text;
}

/** Why `value` cannot be published as the current release, or `null` when it can. */
export function invalidRelease(value: any): string | null {
  if (typeof value?.version !== "string" || !/^\d+\.\d+\.\d+$/.test(value.version)) return "version must look like 1.2.3";
  // Clients open this address in a browser, so it must be one that cannot be tampered with on the way.
  if (typeof value.url !== "string" || !/^https:\/\/[^\s]+$/.test(value.url) || value.url.length > 500) return "url must be an https address";
  if (value.notes !== undefined && (typeof value.notes !== "string" || value.notes.length > 2000)) return "notes must be text of at most 2000 characters";
  return null;
}
