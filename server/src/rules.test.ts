import assert from "node:assert/strict";
import { test } from "node:test";
import { invalid, isKey } from "./rules.ts";

test("program keys are lower-case letters and digits", () => {
  for (const ok of ["7zip", "googlechrome", "百度网盘客户端"]) assert.ok(isKey(ok), ok);
  for (const bad of ["", "abc", "7-Zip", "Chrome", "a b c d", "../etc", "x".repeat(101)]) assert.ok(!isKey(bad), bad);
});

test("rules are confined to known roots", () => {
  const ok = [
    { kind: "dir", path: "%APPDATA%\\7-Zip" },
    { kind: "file", path: "%programdata%\\Vendor\\app.ini" },
    { kind: "reg_key", path: "HKCU\\Software\\7-Zip" },
    { kind: "reg_key", path: "HKLM\\SOFTWARE\\Classes\\CLSID\\{23170F69-40C1-278A-1000-000100020000}" },
    { kind: "reg_key", path: "HKLM\\SOFTWARE\\Classes\\*\\shellex\\ContextMenuHandlers\\7-Zip" },
  ];
  for (const rule of ok) assert.equal(invalid(rule), null, rule.path);

  const bad = [
    { kind: "dir", path: "C:\\Windows" },
    { kind: "dir", path: "%APPDATA%" },
    { kind: "dir", path: "%APPDATA%\\..\\..\\Documents" },
    { kind: "dir", path: "%APPDATA%\\\\x" },
    { kind: "dir", path: "%WINDIR%\\System32" },
    { kind: "dir", path: "%APPDATA%/x" },
    { kind: "dir", path: "%APPDATA%\\*" },
    { kind: "reg_key", path: "HKLM\\SOFTWARE\\..\\SYSTEM" },
    { kind: "file", path: "\\\\server\\share\\x" },
    { kind: "reg_key", path: "HKLM\\SOFTWARE" },
    { kind: "reg_key", path: "HKU\\S-1-5-18\\Software\\X" },
    { kind: "reg_value", path: "HKCU\\Software\\X" },
    { kind: "dir", path: 42 },
    null,
  ];
  for (const rule of bad) assert.notEqual(invalid(rule), null, JSON.stringify(rule));
});
