// A stand-in for an OpenAI-compatible model, for checking the server's AI path without a real one:
//   node server/scripts/mock-model.mjs   (listens on 18999; point the admin page at http://host.docker.internal:18999/v1)
import { createServer } from "node:http";

createServer((request, response) => {
  let body = "";
  request.on("data", (chunk) => (body += chunk));
  request.on("end", () => {
    const { messages = [] } = JSON.parse(body || "{}");
    const facts = messages.at(-1)?.content ?? "";
    console.log(`${request.method} ${request.url}\n--- system\n${messages[0]?.content}\n--- user\n${facts}\n`);
    const name = /^(?:name|item): (.+)$/m.exec(facts)?.[1] ?? "?";
    const answer = { summary: `（模拟回答）${name} 是一个示例程序。`, verdict: "caution", confidence: "medium", points: ["这是模拟模型返回的内容", `收到 ${facts.split("\n").length} 行事实`] };
    response.setHeader("content-type", "application/json");
    response.end(JSON.stringify({ choices: [{ message: { role: "assistant", content: "```json\n" + JSON.stringify(answer) + "\n```" } }] }));
  });
}).listen(18999, () => console.log("mock model on 18999"));
