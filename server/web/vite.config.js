import vue from "@vitejs/plugin-vue";
import { defineConfig } from "vite";

export default defineConfig({
  base: "/admin/",
  plugins: [vue()],
  // `pnpm dev` talks to the server started with docker compose.
  server: { proxy: { "/admin/api": "http://127.0.0.1:8080", "/v1": "http://127.0.0.1:8080", "/healthz": "http://127.0.0.1:8080" } },
});
