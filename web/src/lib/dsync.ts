import { execFile } from "node:child_process";
import { promisify } from "node:util";

import type { DoctorReport, StateReport, StatusReport } from "@/lib/types";

const pexec = promisify(execFile);
const HOME = process.env.HOME ?? "";
const DSYNC_BIN = process.env.DSYNC_BIN ?? "dsync";
const ENV: NodeJS.ProcessEnv = {
  ...process.env,
  PATH: `${HOME}/.local/bin:/usr/local/bin:/usr/bin:/bin`,
};

export type RunResult = {
  ok: boolean;
  code: number | null;
  stdout: string;
  stderr: string;
};

export async function runDsync(args: string[], timeout = 60_000): Promise<RunResult> {
  try {
    const { stdout, stderr } = await pexec(DSYNC_BIN, args, {
      env: ENV,
      timeout,
      maxBuffer: 8 * 1024 * 1024,
    });
    return { ok: true, code: 0, stdout, stderr };
  } catch (e: unknown) {
    const err = e as {
      stdout?: string;
      stderr?: string;
      code?: number | null;
      message?: string;
    };
    return {
      ok: false,
      code: err.code ?? null,
      stdout: err.stdout ?? "",
      stderr: err.stderr ?? err.message ?? "dsync failed",
    };
  }
}

/** Убрать из вывода строки, похожие на секреты (токены/пароли/ключи). */
export function redact(text: string): string {
  return text
    .split("\n")
    .filter((line) => !/token|password|passphrase|private key/i.test(line))
    .join("\n")
    .trim();
}

function fail(result: RunResult, fallback: string): never {
  const msg = redact(result.stderr) || redact(result.stdout) || fallback;
  throw new Error(msg);
}

export async function statusJson(): Promise<StatusReport> {
  const r = await runDsync(["status", "--json"]);
  if (!r.ok) fail(r, "status failed");
  return JSON.parse(r.stdout) as StatusReport;
}

export async function stateJson(): Promise<StateReport> {
  const r = await runDsync(["state", "list", "--json"]);
  if (!r.ok) fail(r, "state list failed");
  return JSON.parse(r.stdout) as StateReport;
}

export async function doctorJson(): Promise<{ report: DoctorReport; code: number | null }> {
  const r = await runDsync(["doctor", "--json"], 120_000);
  if (!r.stdout.trim()) fail(r, "doctor failed");
  return { report: JSON.parse(r.stdout) as DoctorReport, code: r.code };
}
