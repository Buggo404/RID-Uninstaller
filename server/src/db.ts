import pg from "pg";
import type { Rule } from "./rules.ts";

export const pool = new pg.Pool({ connectionString: process.env.DATABASE_URL, max: 10 });

/** Idempotent; runs at every start. */
export async function migrate(): Promise<void> {
  await pool.query(`
    CREATE TABLE IF NOT EXISTS programs (
      key        text PRIMARY KEY,
      name       text NOT NULL,
      revision   integer NOT NULL DEFAULT 1,
      updated_at timestamptz NOT NULL DEFAULT now()
    );
    CREATE TABLE IF NOT EXISTS rules (
      id          bigserial PRIMARY KEY,
      program_key text NOT NULL REFERENCES programs(key) ON DELETE CASCADE,
      kind        text NOT NULL CHECK (kind IN ('dir', 'file', 'reg_key')),
      path        text NOT NULL,
      UNIQUE (program_key, kind, path)
    );
    CREATE TABLE IF NOT EXISTS settings (
      key   text PRIMARY KEY,
      value jsonb NOT NULL
    );
    CREATE TABLE IF NOT EXISTS admins (
      id            serial PRIMARY KEY,
      username      text NOT NULL UNIQUE,
      password_hash text NOT NULL,
      must_change   boolean NOT NULL DEFAULT true,
      failed        integer NOT NULL DEFAULT 0,
      locked_until  timestamptz
    );
    CREATE TABLE IF NOT EXISTS sessions (
      token_hash text PRIMARY KEY,
      admin_id   integer NOT NULL REFERENCES admins(id) ON DELETE CASCADE,
      expires_at timestamptz NOT NULL
    );
  `);
}

export async function programs(): Promise<unknown[]> {
  const found = await pool.query(
    `SELECT p.key, p.name, p.revision, count(r.id)::int AS rules
     FROM programs p LEFT JOIN rules r ON r.program_key = p.key GROUP BY p.key ORDER BY p.name`,
  );
  return found.rows;
}

export async function remove(key: string): Promise<boolean> {
  return !!(await pool.query("DELETE FROM programs WHERE key = $1", [key])).rowCount;
}

export async function setting(key: string): Promise<unknown> {
  const found = await pool.query("SELECT value FROM settings WHERE key = $1", [key]);
  return found.rows[0]?.value ?? null;
}

export async function setSetting(key: string, value: unknown): Promise<void> {
  await pool.query("INSERT INTO settings (key, value) VALUES ($1, $2) ON CONFLICT (key) DO UPDATE SET value = $2", [key, JSON.stringify(value)]);
}

export interface RuleSet {
  key: string;
  name: string;
  revision: number;
  rules: Rule[];
}

/** One indexed lookup per request, however large the tables grow. */
export async function find(key: string): Promise<RuleSet | null> {
  const program = await pool.query("SELECT key, name, revision FROM programs WHERE key = $1", [key]);
  if (!program.rowCount) return null;
  const rules = await pool.query("SELECT kind, path FROM rules WHERE program_key = $1 ORDER BY kind, path", [key]);
  return { ...program.rows[0], rules: rules.rows };
}

/** Replaces a program's rules as a whole and bumps its revision. */
export async function replace(key: string, name: string, rules: Rule[]): Promise<number> {
  const client = await pool.connect();
  try {
    await client.query("BEGIN");
    const saved = await client.query(
      `INSERT INTO programs (key, name) VALUES ($1, $2)
       ON CONFLICT (key) DO UPDATE SET name = $2, revision = programs.revision + 1, updated_at = now()
       RETURNING revision`,
      [key, name],
    );
    await client.query("DELETE FROM rules WHERE program_key = $1", [key]);
    for (const rule of rules) {
      await client.query("INSERT INTO rules (program_key, kind, path) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING", [key, rule.kind, rule.path]);
    }
    await client.query("COMMIT");
    return saved.rows[0].revision;
  } catch (e) {
    await client.query("ROLLBACK");
    throw e;
  } finally {
    client.release();
  }
}
