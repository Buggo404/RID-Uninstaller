import { reactive, watch } from "vue";
import { api } from "./api";

export type SortKey = "name" | "size" | "date" | "publisher" | "version";

const defaults = {
  lang: "auto" as "auto" | "en" | "zh",
  theme: "auto" as "auto" | "light" | "dark",
  compact: false,
  /** Whether the navigation sidebar is expanded, and whether the details panel is shown. */
  sidebar: true,
  details: true,
  mark32: true,
  confirmUninstall: true,
  quietUninstall: false,
  /** How long removed items stay restorable. */
  quarantine: "30" as "7" | "30" | "90" | "never",
  /** Address of the leftover rules server. Empty: no program name ever leaves this machine. */
  rulesServer: "",
  columns: { size: true, date: true, publisher: false, version: false, traced: true },
  showUpdates: true,
  updates: "collapsed" as "collapsed" | "expanded" | "always",
  highlightRecent: true,
  recentInToolbar: true,
  wizard: "ask" as "ask" | "auto" | "off",
  sort: { key: "name" as SortKey, desc: false },
};

export type Prefs = typeof defaults;
export const prefs = reactive<Prefs>(structuredClone(defaults));

export function resetColumns() {
  prefs.columns = structuredClone(defaults.columns);
  prefs.sort = structuredClone(defaults.sort);
}

export async function loadPrefs() {
  const saved = await api.loadPrefs().catch(() => null);
  if (saved) {
    // Unknown keys from a newer version are dropped; missing ones keep their default.
    for (const key of Object.keys(defaults) as (keyof Prefs)[]) {
      const value = saved[key];
      if (value === undefined || typeof value !== typeof defaults[key]) continue;
      const def = defaults[key];
      (prefs as Record<string, unknown>)[key] = typeof def === "object" ? { ...def, ...(value as object) } : value;
    }
  }
  watch(prefs, (p) => api.savePrefs(p).catch(console.error), { deep: true });
  if (prefs.quarantine !== "never") api.quarantinePurgeOld(Number(prefs.quarantine)).catch(console.error);
}
