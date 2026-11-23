import { listen } from "@tauri-apps/api/event";
import { computed, reactive, ref } from "vue";
import { type App, type Category, type Inventory, type StartupItem, type TraceRecord, api } from "./api";
import { prefs } from "./prefs";

// shortcut: provisional window, replace with the original's value once docs/reverse-notes.md has it.
const RECENT_DAYS = 7;

export type View = "uninstaller" | "startup";
export type ListCategory = Category | "recent";
export type JobMode = "uninstall" | "modify" | "force" | "entry";

export interface Row {
  app: App;
  depth: number;
  kids: number;
  open: boolean;
}

export const view = ref<View>("uninstaller");
export const inv = ref<Inventory>({ apps: [], warnings: [] });
export const loading = ref(true);
/** How many size-measuring passes are running. */
export const sizing = ref(0);
export const category = ref<ListCategory>("apps");
export const filter = ref("");
export const selected = reactive(new Set<string>());
/// `ask`: confirm even if the user turned confirmations off (requests coming from outside the window).
export const job = ref<{ mode: JobMode; apps: App[]; ask?: boolean } | null>(null);
/// The install-and-trace dialog: `null` closed, otherwise open (optionally appending to a program).
export const tracing = ref<{ appendTo?: App; path?: string } | null>(null);
export const tracedView = ref<TraceRecord | null>(null);

/** Parents whose expand state differs from the default set in preferences. */
const toggled = reactive(new Set<string>());

// Sizes measured in earlier runs are shown at once and corrected as the new pass reports.
const SIZES_KEY = "rid.sizes";
function savedSizes(): [string, number][] {
  try {
    return Object.entries(JSON.parse(localStorage.getItem(SIZES_KEY) ?? "{}")).filter((e): e is [string, number] => typeof e[1] === "number");
  } catch {
    return [];
  }
}
const sizes = reactive(new Map<string, number>(savedSizes()));

export const sizeOf = (a: App): number | null => a.size_kb ?? sizes.get(a.id) ?? null;

const byId = computed(() => new Map(inv.value.apps.map((a) => [a.id, a])));

const childrenOf = computed(() => {
  const map = new Map<string, App[]>();
  for (const a of inv.value.apps) {
    if (a.parent && byId.value.has(a.parent)) map.set(a.parent, [...(map.get(a.parent) ?? []), a]);
  }
  return map;
});

export function isRecent(a: App): boolean {
  const cutoff = new Date(Date.now() - RECENT_DAYS * 864e5).toISOString().slice(0, 10);
  return a.category !== "hidden" && !!a.install_date && a.install_date >= cutoff;
}

export const recentCount = computed(() => inv.value.apps.filter(isRecent).length);

function compare(a: App, b: App): number {
  const { key, desc } = prefs.sort;
  let r: number;
  if (key === "size") r = (sizeOf(a) ?? -1) - (sizeOf(b) ?? -1);
  else if (key === "date") r = (a.install_date ?? "").localeCompare(b.install_date ?? "");
  else r = a[key].localeCompare(b[key], undefined, { sensitivity: "base", numeric: true });
  if (r === 0) r = a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });
  return desc ? -r : r;
}

export const rows = computed<Row[]>(() => {
  const q = filter.value.trim().toLowerCase();
  const matches = (a: App) => !q || a.name.toLowerCase().includes(q) || a.publisher.toLowerCase().includes(q);
  const out: Row[] = [];
  const top = inv.value.apps.filter(
    (a) =>
      (category.value === "recent" ? isRecent(a) : a.category === category.value) &&
      !(a.parent && byId.value.has(a.parent)),
  );
  for (const app of top.sort(compare)) {
    const kids = prefs.showUpdates ? (childrenOf.value.get(app.id) ?? []) : [];
    const hits = q ? kids.filter(matches) : kids;
    if (!matches(app) && !(q && hits.length)) continue;
    const open =
      kids.length > 0 &&
      (prefs.updates === "always" ||
        (q !== "" && hits.length > 0) ||
        (prefs.updates === "expanded") !== toggled.has(app.id));
    out.push({ app, depth: 0, kids: kids.length, open });
    if (open) for (const kid of [...hits].sort(compare)) out.push({ app: kid, depth: 1, kids: 0, open: false });
  }
  return out;
});

export const selection = computed(() => rows.value.map((r) => r.app).filter((a) => selected.has(a.id)));
export const current = computed(() => (selection.value.length === 1 ? selection.value[0] : null));
export const totalKb = computed(() => rows.value.reduce((sum, r) => sum + (sizeOf(r.app) ?? 0), 0));

export function setOpen(row: Row, open: boolean) {
  if (row.kids === 0 || prefs.updates === "always" || row.open === open) return;
  if (toggled.has(row.app.id)) toggled.delete(row.app.id);
  else toggled.add(row.app.id);
}

let generation = 0;

void listen<[string, number]>("dir-size", ({ payload: [id, kb] }) => sizes.set(id, kb));

/** Installers often leave EstimatedSize out; those folders are measured in the background. */
async function measure(gen: number, apps: App[]) {
  const items = apps.filter((a) => a.size_kb == null && a.install_location).map((a): [string, string] => [a.id, a.install_location]);
  if (!items.length) return;
  sizing.value++;
  try {
    await api.dirSizes(gen, items);
  } finally {
    sizing.value--;
    if (gen === generation) localStorage.setItem(SIZES_KEY, JSON.stringify(Object.fromEntries(sizes)));
  }
}

export async function refresh() {
  const gen = ++generation;
  loading.value = true;
  try {
    inv.value = await api.listApps();
  } finally {
    loading.value = false;
  }
  const ids = new Set(inv.value.apps.map((a) => a.id));
  for (const id of [...selected]) if (!ids.has(id)) selected.delete(id);
  for (const id of [...sizes.keys()]) if (!ids.has(id)) sizes.delete(id);

  // Neither is awaited: the list is usable as soon as it is loaded.
  measure(gen, inv.value.apps).catch(console.error);
  api
    .msiLocations()
    .then((found) => {
      if (gen !== generation) return;
      const filled: App[] = [];
      for (const [id, folder] of found) {
        const app = byId.value.get(id);
        if (!app) continue;
        app.install_location = folder;
        filled.push(app);
      }
      return measure(gen, filled);
    })
    .catch(console.error);
}

export function start(mode: JobMode) {
  if (selection.value.length) job.value = { mode, apps: selection.value };
}

export type StartupGroup = "all" | "registry" | "folder" | "tasks" | "store";

export const startup = reactive({
  items: [] as StartupItem[],
  group: "all" as StartupGroup,
  loading: false,
  adding: false,
});

export function startupGroupOf(item: StartupItem): StartupGroup {
  if (item.source === "task") return "tasks";
  if (item.source === "store_app") return "store";
  return item.source.startsWith("folder") ? "folder" : "registry";
}

export async function refreshStartup() {
  startup.loading = true;
  try {
    startup.items = await api.startupList();
  } finally {
    startup.loading = false;
  }
}
