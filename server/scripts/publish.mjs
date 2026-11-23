// Publishes a release to a server, which then offers it to clients as an update:
//   RID_SERVER=https://… RID_ADMIN_TOKEN=… node scripts/publish.mjs <version> <installer address> <installer .sig file> [notes]
// The same can be done on the "版本" page of the admin interface.
import { readFileSync } from "node:fs";

const [version, url, signatureFile, notes = ""] = process.argv.slice(2);
if (!signatureFile) {
  console.error("usage: node scripts/publish.mjs <version> <installer address> <installer .sig file> [notes]");
  process.exit(2);
}
const server = (process.env.RID_SERVER ?? "http://127.0.0.1:8080").replace(/\/+$/, "");
const response = await fetch(`${server}/v1/update`, {
  method: "PUT",
  headers: { "content-type": "application/json", authorization: `Bearer ${process.env.RID_ADMIN_TOKEN}` },
  body: JSON.stringify({ version, url, signature: readFileSync(signatureFile, "utf8").trim(), notes }),
});
console.log(response.status, await response.text());
process.exit(response.ok ? 0 : 1);
