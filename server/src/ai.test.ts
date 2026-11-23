import assert from "node:assert/strict";
import { test } from "node:test";
import { config, factsText, invalidRelease, invalidSettings, parseAnswer, request, textOf } from "./ai.ts";
import { DEFAULT_PROMPTS } from "./prompts.ts";

test("the model comes from the saved settings, else the environment, else the AI is off", () => {
  assert.equal(config(null, {}).upstream, null);
  assert.equal(config(null, { AI_BASE_URL: "https://x/v1" }).upstream, null);
  assert.deepEqual(config(null, { AI_BASE_URL: "https://x/v1/", AI_MODEL: "m", AI_API_KEY: "k" }).upstream, { protocol: "chat", baseUrl: "https://x/v1", apiKey: "k", model: "m" });
  const saved = { protocol: "messages", baseUrl: "https://y/v1", apiKey: "s", model: "n", prompts: { startup: "mine", leftover: "  " } };
  const both = config(saved, { AI_BASE_URL: "https://x/v1", AI_MODEL: "m" });
  assert.deepEqual(both.upstream, { protocol: "messages", baseUrl: "https://y/v1", apiKey: "s", model: "n" });
  assert.equal(both.prompts.startup, "mine");
  assert.equal(both.prompts.leftover, DEFAULT_PROMPTS.leftover, "a blank prompt falls back to the built-in one");
  assert.equal(both.prompts.program, DEFAULT_PROMPTS.program);
  assert.equal(config({ protocol: "soap", baseUrl: "https://y", model: "n" }, {}).upstream, null);
});

test("settings are checked before they are saved", () => {
  assert.equal(invalidSettings({ protocol: "chat", baseUrl: "https://x/v1", model: "m", prompts: { program: "p" } }), null);
  assert.equal(invalidSettings({ protocol: "chat", baseUrl: "", model: "" }), null, "an empty address switches the AI off");
  for (const bad of [{}, { protocol: "chat", baseUrl: "ftp://x", model: "m" }, { protocol: "chat", baseUrl: "https://x", model: " " }, { protocol: "chat", baseUrl: "https://x", model: "m", prompts: { program: "x".repeat(8001) } }]) {
    assert.ok(invalidSettings(bad), JSON.stringify(bad).slice(0, 80));
  }
});

test("facts become plain lines and anything else is refused", () => {
  assert.equal(factsText({ name: " 7-Zip ", empty: "", files: ["a.exe", "b.dll"] }), "name: 7-Zip\nfiles:\n- a.exe\n- b.dll");
  for (const bad of [null, [], {}, "text", { "Bad Key": "x" }, { name: 5 }, { name: "x".repeat(2001) }, { files: [1] }, { files: Array(61).fill("a") }, { only_blank: " " }]) {
    assert.equal(factsText(bad), null, JSON.stringify(bad).slice(0, 60));
  }
});

test("an answer is read out of whatever surrounds it, and never trusted beyond its shape", () => {
  const wrapped = '```json\n{"summary": "A file archiver.", "verdict": "safe", "confidence": "high", "points": ["Open source", "", 5, "No dependants"]}\n```';
  assert.deepEqual(parseAnswer(wrapped), { summary: "A file archiver.", verdict: "safe", confidence: "high", points: ["Open source", "No dependants"] });
  assert.deepEqual(parseAnswer('{"summary": "x", "verdict": "delete everything", "confidence": 1, "points": "none"}'), { summary: "x", verdict: "unknown", confidence: "low", points: [] });
  assert.deepEqual(parseAnswer("I do not know this program."), { summary: "I do not know this program.", verdict: "unknown", confidence: "low", points: [] });
  assert.equal(parseAnswer('{"verdict": "safe"}').verdict, "unknown", "a verdict without a summary is not shown as one");
  assert.equal(parseAnswer('{"summary": "' + "x".repeat(2000) + '"}').summary.length, 800);
});

test("each protocol gets its own address, credentials and answer shape", () => {
  const up = { baseUrl: "https://x/v1", apiKey: "k", model: "m" };
  const chat = request({ ...up, protocol: "chat" }, "sys", "hi");
  assert.equal(chat.url, "https://x/v1/chat/completions");
  assert.equal(chat.headers.authorization, "Bearer k");
  assert.equal(request({ ...up, protocol: "responses" }, "sys", "hi").url, "https://x/v1/responses");
  const messages = request({ ...up, protocol: "messages" }, "sys", "hi");
  assert.equal(messages.url, "https://x/v1/messages");
  assert.equal(messages.headers["x-api-key"], "k");
  assert.equal(messages.headers.authorization, undefined);

  assert.equal(textOf("chat", { choices: [{ message: { content: " a " } }] }), "a");
  assert.equal(textOf("messages", { content: [{ type: "thinking", thinking: "x" }, { type: "text", text: "b" }] }), "b");
  assert.equal(textOf("responses", { output: [{ type: "reasoning" }, { type: "message", content: [{ type: "output_text", text: "c" }] }] }), "c");
  for (const protocol of ["chat", "messages", "responses"] as const) assert.equal(textOf(protocol, { error: "nope" }), "");
});

test("a release needs a plain version and an https address", () => {
  const signature = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkK";
  assert.equal(invalidRelease({ version: "1.2.3", url: "https://example.com/rid.exe", signature, notes: "fixes" }), null);
  for (const bad of [null, {}, { version: "1.2", url: "https://a/b", signature }, { version: "1.2.3", url: "http://a/b", signature }, { version: "1.2.3", url: "file:///c:/x", signature }, { version: "1.2.3", url: "https://a/b" }, { version: "1.2.3", url: "https://a/b", signature: "<script>" }, { version: "1.2.3", url: "https://a/b", signature, notes: 5 }]) {
    assert.ok(invalidRelease(bad), JSON.stringify(bad));
  }
});
