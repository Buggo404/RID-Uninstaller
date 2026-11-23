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
  `);
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
