import { createHash, randomBytes, randomInt, scrypt, timingSafeEqual } from "node:crypto";
import { promisify } from "node:util";
import { pool } from "./db.ts";

const derive = promisify(scrypt) as (password: string, salt: Buffer, length: number) => Promise<Buffer>;

export async function hashPassword(password: string): Promise<string> {
  const salt = randomBytes(16);
  return `scrypt$${salt.toString("base64")}$${(await derive(password, salt, 32)).toString("base64")}`;
}

export async function passwordMatches(password: string, stored: string): Promise<boolean> {
  const [scheme, salt, hash] = stored.split("$");
  if (scheme !== "scrypt" || !salt || !hash) return false;
  const wanted = Buffer.from(hash, "base64");
  return timingSafeEqual(await derive(password, Buffer.from(salt, "base64"), wanted.length), wanted);
}

/** Why these cannot be an administrator's credentials, or `null` when they can. */
export function invalidCredentials(username: unknown, password: unknown): string | null {
  if (typeof username !== "string" || !/^[A-Za-z0-9_.-]{3,32}$/.test(username)) return "用户名为 3–32 位字母、数字或 _ . -";
  if (typeof password !== "string" || password.length < 10 || password.length > 200) return "密码至少 10 位";
  if (password.toLowerCase().includes(username.toLowerCase())) return "密码不能包含用户名";
  return null;
}

// Captcha: digits drawn as jittered seven-segment strokes, so the image holds no text to read out.

const CAPTCHA_MS = 3 * 60_000;
const SEGMENTS: Record<string, [number, number, number, number]> = {
  a: [0, 0, 12, 0],
  b: [12, 0, 12, 12],
  c: [12, 12, 12, 24],
  d: [0, 24, 12, 24],
  e: [0, 12, 0, 24],
  f: [0, 0, 0, 12],
  g: [0, 12, 12, 12],
};
const DIGITS = ["abcdef", "bc", "abged", "abgcd", "fgbc", "afgcd", "afgedc", "abc", "abcdefg", "abcdfg"];
const captchas = new Map<string, { answer: string; expires: number }>();

function draw(answer: string): string {
  const rand = (from: number, to: number) => (from + Math.random() * (to - from)).toFixed(1);
  const strokes = [...answer].map((digit, i) => {
    const lines = [...DIGITS[Number(digit)]].map((s) => {
      const [x1, y1, x2, y2] = SEGMENTS[s].map((v) => v + Number(rand(-1.5, 1.5)));
      return `M${x1} ${y1}L${x2} ${y2}`;
    });
    return `<path transform="translate(${14 + i * 24} ${Number(rand(8, 14))}) rotate(${rand(-14, 14)} 6 12) skewX(${rand(-12, 12)})" d="${lines.join("")}"/>`;
  });
  const noise = Array.from({ length: 5 }, () => `<path opacity=".45" stroke-width="1.2" d="M${rand(0, 40)} ${rand(0, 48)}Q${rand(40, 100)} ${rand(-10, 58)} ${rand(100, 140)} ${rand(0, 48)}"/>`);
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 140 48" fill="none" stroke="#8a8f98" stroke-width="2.6" stroke-linecap="round">${noise.join("")}${strokes.join("")}</svg>`;
}

export function newCaptcha(now = Date.now()): { id: string; svg: string } {
  // Entries are in order of expiry; the map is also capped so that asking for images cannot fill memory.
  for (const [id, captcha] of captchas) {
    if (captcha.expires > now && captchas.size < 5000) break;
    captchas.delete(id);
  }
  const answer = Array.from({ length: 5 }, () => randomInt(10)).join("");
  const id = randomBytes(16).toString("hex");
  captchas.set(id, { answer, expires: now + CAPTCHA_MS });
  return { id, svg: draw(answer) };
}

/** Each image can be answered once, right or wrong. */
export function captchaPasses(id: unknown, answer: unknown, now = Date.now()): boolean {
  const captcha = typeof id === "string" ? captchas.get(id) : undefined;
  if (typeof id === "string") captchas.delete(id);
  return !!captcha && captcha.expires > now && answer === captcha.answer;
}

/** For tests. */
export const captchaAnswer = (id: string) => captchas.get(id)?.answer;

// Administrators and their sessions

export interface Admin {
  id: number;
  username: string;
  must_change: boolean;
}

const MAX_FAILURES = 5;
const LOCK_MINUTES = 15;
const SESSION_HOURS = 12;

/** Creates the first administrator with a random password, shown once in the log. */
export async function ensureAdmin(): Promise<void> {
  const password = randomBytes(12).toString("base64url");
  const created = await pool.query(
    "INSERT INTO admins (username, password_hash) SELECT 'admin', $1 WHERE NOT EXISTS (SELECT 1 FROM admins) RETURNING id",
    [await hashPassword(password)],
  );
  if (created.rowCount) console.log(`\n*** Initial administrator: admin / ${password}\n*** Sign in at /admin and change it; this password is not shown again.\n`);
}

/** The administrator for these credentials; "locked" after too many wrong passwords. */
export async function signIn(username: string, password: string): Promise<Admin | "locked" | null> {
  const found = await pool.query("SELECT id, username, password_hash, must_change, locked_until > now() AS locked FROM admins WHERE username = $1", [username]);
  const admin = found.rows[0];
  if (admin?.locked) return "locked";
  // An unknown name costs the same time as a wrong password, so names cannot be probed by timing.
  const ok = await passwordMatches(password, admin?.password_hash ?? (await dummyHash));
  if (!admin) return null;
  if (!ok) {
    await pool.query(
      `UPDATE admins SET failed = failed + 1,
         locked_until = CASE WHEN failed + 1 >= $2 THEN now() + make_interval(mins => $3) ELSE locked_until END
       WHERE id = $1`,
      [admin.id, MAX_FAILURES, LOCK_MINUTES],
    );
    return null;
  }
  await pool.query("UPDATE admins SET failed = 0, locked_until = NULL WHERE id = $1", [admin.id]);
  return { id: admin.id, username: admin.username, must_change: admin.must_change };
}
const dummyHash = hashPassword(randomBytes(16).toString("hex"));

const digest = (token: string) => createHash("sha256").update(token).digest("hex");

/** Only the token's hash is stored: a copy of the database does not hand out live sessions. */
export async function openSession(adminId: number): Promise<string> {
  const token = randomBytes(32).toString("base64url");
  await pool.query("DELETE FROM sessions WHERE expires_at < now()");
  await pool.query("INSERT INTO sessions (token_hash, admin_id, expires_at) VALUES ($1, $2, now() + make_interval(hours => $3))", [digest(token), adminId, SESSION_HOURS]);
  return token;
}

export async function sessionAdmin(token: string | undefined): Promise<Admin | null> {
  if (!token) return null;
  const found = await pool.query(
    "SELECT a.id, a.username, a.must_change FROM sessions s JOIN admins a ON a.id = s.admin_id WHERE s.token_hash = $1 AND s.expires_at > now()",
    [digest(token)],
  );
  return found.rows[0] ?? null;
}

export async function closeSession(token: string | undefined): Promise<void> {
  if (token) await pool.query("DELETE FROM sessions WHERE token_hash = $1", [digest(token)]);
}

/** Changes the credentials and signs out every session of this administrator. */
export async function changeCredentials(admin: Admin, currentPassword: string, username: string, password: string): Promise<string | null> {
  const stored = await pool.query("SELECT password_hash FROM admins WHERE id = $1", [admin.id]);
  if (!(await passwordMatches(currentPassword, stored.rows[0]?.password_hash ?? ""))) return "当前密码不正确";
  if (await passwordMatches(password, stored.rows[0].password_hash)) return "新密码不能与当前密码相同";
  const taken = await pool.query("SELECT 1 FROM admins WHERE username = $1 AND id <> $2", [username, admin.id]);
  if (taken.rowCount) return "用户名已被使用";
  await pool.query("UPDATE admins SET username = $2, password_hash = $3, must_change = false WHERE id = $1", [admin.id, username, await hashPassword(password)]);
  await pool.query("DELETE FROM sessions WHERE admin_id = $1", [admin.id]);
  return null;
}

export function cookieOf(header: string | undefined, name: string): string | undefined {
  for (const part of (header ?? "").split(";")) {
    const [key, ...value] = part.trim().split("=");
    if (key === name) return value.join("=");
  }
}
