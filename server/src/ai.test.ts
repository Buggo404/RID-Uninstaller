import assert from "node:assert/strict";
import { test } from "node:test";
import { invalidRelease, request, textOf, upstream } from "./ai.ts";

test("the upstream is off until an address and a model are set", () => {
  assert.equal(upstream({}), null);
  assert.equal(upstream({ AI_BASE_URL: "https://x/v1" }), null);
  assert.deepEqual(upstream({ AI_BASE_URL: "https://x/v1/", AI_MODEL: "m", AI_API_KEY: "k" }), { protocol: "chat", baseUrl: "https://x/v1", apiKey: "k", model: "m" });
  assert.throws(() => upstream({ AI_BASE_URL: "https://x/v1", AI_MODEL: "m", AI_PROTOCOL: "soap" }));
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
  assert.equal(invalidRelease({ version: "1.2.3", url: "https://example.com/rid.exe", notes: "fixes" }), null);
  for (const bad of [null, {}, { version: "1.2", url: "https://a/b" }, { version: "1.2.3", url: "http://a/b" }, { version: "1.2.3", url: "file:///c:/x" }, { version: "1.2.3", url: "https://a/b", notes: 5 }]) {
    assert.ok(invalidRelease(bad), JSON.stringify(bad));
  }
});
