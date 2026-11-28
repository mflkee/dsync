import { randomUUID } from "node:crypto";

import { appendAudit } from "@/lib/audit";
import { redact, runDsync } from "@/lib/dsync";
import type { Job } from "@/lib/types";

const jobs = new Map<string, Job>();
const active = new Map<string, string>();
let chain: Promise<void> = Promise.resolve();

export type CreateJobResult = { job: Job } | { conflict: string };

export function createJob(
  action: string,
  target: string,
  args: string[],
  timeout = 60_000,
): CreateJobResult {
  const running = active.get(target);
  if (running) return { conflict: running };

  const id = randomUUID();
  const job: Job = {
    id,
    action,
    target,
    state: "queued",
    code: null,
    startedAt: Date.now(),
    output: [],
  };
  jobs.set(id, job);
  active.set(target, id);

  chain = chain
    .then(async () => {
      job.state = "running";
      const result = await runDsync(args, timeout);
      job.code = result.code;
      job.state = result.ok ? "ok" : "failed";
      job.finishedAt = Date.now();
      const output = redact(`${result.stdout}\n${result.stderr}`.trim());
      job.output = output.split("\n").filter(Boolean).slice(-200);
      job.error = result.ok ? undefined : job.output.at(-1) ?? "failed";
      active.delete(target);
      await appendAudit({
        ts: Date.now(),
        action,
        target,
        result: result.ok ? "ok" : "failed",
        code: result.code,
        error: job.error,
      });
    })
    .catch((e: unknown) => {
      job.state = "failed";
      job.finishedAt = Date.now();
      job.error = String(e);
      active.delete(target);
    });

  return { job };
}

export function getJob(id: string): Job | undefined {
  return jobs.get(id);
}

export function listJobs(limit = 20): Job[] {
  const cutoff = Date.now() - 60 * 60 * 1000;
  for (const [id, job] of jobs) {
    if (job.finishedAt && job.finishedAt < cutoff) jobs.delete(id);
  }
  return [...jobs.values()]
    .sort((a, b) => b.startedAt - a.startedAt)
    .slice(0, limit);
}
