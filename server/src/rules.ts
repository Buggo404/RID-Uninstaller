// Leftover rules: for one program, the places it is known to leave things behind.
// The client treats a rule as a suggestion only; it still applies its own safety checks.

export type Kind = "dir" | "file" | "reg_key";

export interface Rule {
  kind: Kind;
  path: string;
}

/** Folders a rule may be rooted in. The client expands these; nothing else is accepted. */
const ROOTS = ["%APPDATA%", "%LOCALAPPDATA%", "%LOCALAPPDATALOW%", "%PROGRAMDATA%", "%PROGRAMFILES%", "%PROGRAMFILES(X86)%", "%USERPROFILE%"];

/** A program key is the client's normalised name: letters and digits only, lower case. */
export function isKey(key: string): boolean {
  return /^[\p{L}\p{N}]{4,100}$/u.test(key) && key === key.toLowerCase();
}

/** Why `rule` is not acceptable, or null if it is. */
export function invalid(rule: unknown): string | null {
  const { kind, path } = (rule ?? {}) as Partial<Rule>;
  if (typeof path !== "string" || path.length > 400) return "path must be a string of at most 400 characters";
  const parts = path.split("\\");
  // No "..", no empty component, nothing that could re-root the path.
  if (parts.some((p) => p === "" || p === "." || p === ".." || p !== p.trim())) return `not a plain path: ${path}`;
  if (kind === "reg_key") {
    return (parts[0] === "HKLM" || parts[0] === "HKCU") && parts.length >= 3 ? null : "a registry rule starts with HKLM\\ or HKCU\\ and names a key below a top-level key";
  }
  // Registry key names may contain these ("Classes\*"); file names may not.
  if (parts.some((p) => /[/:*?"<>|]/.test(p))) return `not a plain path: ${path}`;
  if (kind === "dir" || kind === "file") {
    return ROOTS.includes(parts[0].toUpperCase()) && parts.length >= 2 ? null : `a file rule starts with one of ${ROOTS.join(", ")} and names something inside it`;
  }
  return "kind must be dir, file or reg_key";
}
