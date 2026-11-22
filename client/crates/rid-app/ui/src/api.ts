import { convertFileSrc, invoke } from "@tauri-apps/api/core";

export type Category = "apps" | "system" | "hidden" | "store" | "packages";

/** Where a list entry comes from; shown as-is, these are product names. */
export const SOURCES = { registry: "", store: "Microsoft Store", steam: "Steam", scoop: "Scoop", chocolatey: "Chocolatey" };

export interface App {
  id: string;
  source: keyof typeof SOURCES;
  name: string;
  version: string;
  publisher: string;
  category: Category;
  parent: string | null;
  install_date: string | null;
  size_kb: number | null;
  install_location: string;
  uninstall_string: string;
  quiet_uninstall_string: string;
  modify_path: string;
  icon: string;
  url: string;
  reg_path: string;
  is_msi: boolean;
  is_32bit: boolean;
  traced: boolean;
  broken: boolean;
}

export interface Inventory {
  apps: App[];
  warnings: string[];
}

export const api = {
  listApps: () => invoke<Inventory>("list_apps"),
  dirSize: (path: string) => invoke<number>("dir_size", { path }),
  openFolder: (path: string) => invoke<void>("open_folder", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  openRegistry: (keyPath: string) => invoke<void>("open_registry", { keyPath }),
  systemTool: (name: string) => invoke<void>("system_tool", { name }),
  loadPrefs: () => invoke<Record<string, unknown> | null>("load_prefs"),
  savePrefs: (prefs: unknown) => invoke<void>("save_prefs", { prefs }),
  saveTextFile: (path: string, contents: string) => invoke<void>("save_text_file", { path, contents }),
  uninstallRun: (id: string, modify: boolean, quiet: boolean) =>
    invoke<RunResult>("uninstall_run", { id, modify, quiet }),
  quietSupport: (ids: string[]) => invoke<boolean[]>("quiet_support", { ids }),
  uninstallCancel: () => invoke<void>("uninstall_cancel"),
  scanLeftovers: (ids: string[]) => invoke<Leftover[]>("scan_leftovers", { ids }),
  processes: (ids: string[]) => invoke<Proc[]>("processes", { ids }),
  killProcesses: (ids: string[], pids: number[]) => invoke<string[]>("kill_processes", { ids, pids }),
  removeLeftovers: (label: string, ids: string[], targets: Target[]) =>
    invoke<Report>("remove_leftovers", { label, ids, targets }),
  removeEntries: (label: string, ids: string[]) => invoke<Report>("remove_entries", { label, ids }),
  quarantineList: () => invoke<BatchInfo[]>("quarantine_list"),
  quarantineRestore: (id: string) => invoke<string[]>("quarantine_restore", { id }),
  quarantinePurge: (id: string) => invoke<void>("quarantine_purge", { id }),
  quarantinePurgeOld: (days: number) => invoke<number>("quarantine_purge_old", { days }),
  startupList: () => invoke<StartupItem[]>("startup_list"),
  startupSetEnabled: (source: StartupSource, name: string, enabled: boolean) =>
    invoke<void>("startup_set_enabled", { source, name, enabled }),
  startupDelete: (source: StartupSource, name: string) => invoke<void>("startup_delete", { source, name }),
  startupAdd: (to: string, name: string, exe: string, args: string) =>
    invoke<void>("startup_add", { to, name, exe, arguments: args }),
  driverStatus: () => invoke<"ready" | "installed" | "not_installed">("driver_status"),
  driverInstall: () => invoke<void>("driver_install"),
  driverRemove: () => invoke<void>("driver_remove"),
  traceRun: (path: string, args: string) => invoke<TraceResult>("trace_run", { path, arguments: args }),
  traceSave: (appId: string | null, customName: string | null, dependents: string[], append: boolean) =>
    invoke<void>("trace_save", { appId, customName, dependents, append }),
  traceRevert: () => invoke<Report>("trace_revert"),
  traceGet: (id: string) => invoke<TraceRecord | null>("trace_get", { id }),
  traceForget: (id: string) => invoke<void>("trace_forget", { id }),
  launchRequest: () => invoke<LaunchRequest | null>("launch_request"),
  contextMenuEnabled: () => invoke<boolean>("context_menu_enabled"),
  setContextMenu: (enabled: boolean, traceLabel: string, uninstallLabel: string) =>
    invoke<void>("set_context_menu", { enabled, traceLabel, uninstallLabel }),
};

export type LaunchRequest =
  | { kind: "trace"; path: string }
  | { kind: "uninstall"; app_id: string | null; shortcut: string }
  | { kind: "startup" }
  | { kind: "store_apps" };

export interface TraceResult {
  items: Target[];
  dropped: number;
  new_apps: App[];
}

export interface TraceRecord {
  app_id: string;
  name: string;
  created: number;
  items: Target[];
  dependents: string[];
}

export type TraceEvent = { pid: number } & (
  | { kind: "created" | "deleted"; path: string; dir: boolean }
  | { kind: "renamed"; from: string; to: string; dir: boolean }
  | { kind: "key_created" | "key_deleted"; key: string }
  | { kind: "value_set" | "value_deleted"; key: string; name: string }
  | { kind: "process_started"; image: string; followed: boolean }
  | { kind: "process_exited" }
);

export type StartupSource =
  | "hklm_run"
  | "hklm_run32"
  | "hklm_run_once"
  | "hklm_run_once32"
  | "hkcu_run"
  | "hkcu_run_once"
  | "folder_user"
  | "folder_common"
  | "task"
  | "store_app";

export interface StartupItem {
  source: StartupSource;
  name: string;
  title: string;
  command: string;
  exe: string;
  company: string;
  status: "running" | "not_running" | "disabled" | "invalid";
  can_disable: boolean;
  is_32bit: boolean;
}

export interface RunResult {
  exit_code: number;
  cancelled: boolean;
  still_installed: boolean;
}

export type Target =
  | { kind: "dir"; path: string }
  | { kind: "file"; path: string }
  | { kind: "reg_key"; key: string }
  | { kind: "reg_value"; key: string; name: string }
  | { kind: "service"; name: string };

export type Evidence =
  | "install_location"
  | "uninstaller_folder"
  | "shortcut"
  | "name_match"
  | "publisher_and_name"
  | "uninstall_entry"
  | "runs_from_folder"
  | "installer_record"
  | "traced";

export interface Leftover {
  app_id: string;
  target: Target;
  evidence: Evidence;
  certain: boolean;
  size_kb: number;
  documents: number;
}

export interface Proc {
  pid: number;
  path: string;
}

export interface Report {
  batch: string | null;
  log: { target: Target; ok: boolean; message: string }[];
  pending_reboot: string[];
}

export interface BatchInfo {
  id: string;
  label: string;
  created: number;
  items: number;
  size_kb: number;
}

export function targetText(t: Target): string {
  if (t.kind === "dir" || t.kind === "file") return t.path;
  if (t.kind === "reg_key") return t.key;
  if (t.kind === "reg_value") return `${t.key} → ${t.name}`;
  return t.name;
}

export function iconUrl(source: string): string {
  const hex = Array.from(new TextEncoder().encode(source), (b) => b.toString(16).padStart(2, "0")).join("");
  return convertFileSrc(hex, "icon");
}

export function formatSize(kb: number | null | undefined): string {
  if (!kb) return "";
  if (kb < 1024) return `${kb} KB`;
  if (kb < 1024 * 1024) return `${(kb / 1024).toFixed(1)} MB`;
  return `${(kb / 1024 / 1024).toFixed(2)} GB`;
}
