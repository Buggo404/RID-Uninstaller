// Publishes seed/rules.txt to a rules server (replacing the rules of the programs it names):
//   RID_SERVER=https://… RID_ADMIN_TOKEN=… node scripts/seed.mjs
import { readFileSync } from "node:fs";

const server = (process.env.RID_SERVER ?? "http://127.0.0.1:8080").replace(/\/+$/, "");
const backslash = String.fromCharCode(92);

const programs = [];
for (const line of readFileSync(new URL("../seed/rules.txt", import.meta.url), "utf8").split(/\r?\n/)) {
  const header = /^\[(.+?)\]\s+(.+)$/.exec(line);
  if (header) programs.push({ key: header[1], name: header[2].trim(), rules: [] });
  else if (line.trim() && !line.startsWith("#")) {
    const [kind, ...path] = line.trim().split(" ");
    programs.at(-1).rules.push({ kind, path: path.join(" ").replaceAll("/", backslash) });
  }
}

let failed = 0;
for (const { key, name, rules } of programs) {
  const response = await fetch(`${server}/v1/rules/${encodeURIComponent(key)}`, {
    method: "PUT",
    headers: { "content-type": "application/json", authorization: `Bearer ${process.env.RID_ADMIN_TOKEN}` },
    body: JSON.stringify({ name, rules }),
  });
  console.log(`${response.status} ${key} (${rules.length} rules)${response.ok ? "" : ` ${await response.text()}`}`);
  failed += !response.ok;
}
process.exit(failed ? 1 : 0);
