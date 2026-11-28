import { appendFile, mkdir, readFile, rename, stat } from "node:fs/promises";
import path from "node:path";

import type { AuditEntry } from "@/lib/types";

const HOME = process.env.HOME ?? "";
const DIR = path.join(HOME, ".local/share/dsync/web");
const FILE = path.join(DIR, "audit.jsonl");
const MAX_BYTES = 5 * 1024 * 1024;

export async function appendAudit(entry: AuditEntry): Promise<void> {
  try {
    await mkdir(DIR, { recursive: true, mode: 0o700 });
    const st = await stat(FILE).catch(() => null);
    if (st && st.size > MAX_BYTES) {
      await rename(FILE, `${FILE}.1`).catch(() => {});
    }
    await appendFile(FILE, `${JSON.stringify(entry)}\n`, { mode: 0o600 });
  } catch {
    // Аудит best-effort: не валим действие из-за файловой системы.
  }
}

export async function readAudit(limit = 200): Promise<AuditEntry[]> {
  try {
    const text = await readFile(FILE, "utf8");
    return text
      .trim()
      .split("\n")
      .filter(Boolean)
      .slice(-limit)
      .reverse()
      .map((line) => JSON.parse(line) as AuditEntry);
  } catch {
    return [];
  }
}
