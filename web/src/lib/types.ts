export type PullReport = {
  ok: boolean;
  attempts: number;
  finished_at: number;
  error?: string;
};

export type ProjectReport = {
  name: string;
  branch: string;
  commit_hash: string;
  dirty: boolean;
  ahead: number;
  behind: number;
};

export type MachineReport = {
  online: boolean;
  last_seen: number;
  last_push: number;
  version?: string;
  pulls: Record<string, PullReport>;
  projects: ProjectReport[];
};

export type StateSummary = {
  seq: number;
  applied: number;
  failing: number;
  dead_letter: number;
};

export type StatusReport = {
  schema_version: number;
  client_version: string;
  hub_version?: string;
  machines: Record<string, MachineReport>;
  local_state: StateSummary;
};

export type FailureReport = {
  key: string;
  attempts: number;
  updated: number;
  seq: number;
};

export type DeadReport = {
  key: string;
  updated: number;
  seq: number;
  error: string;
  at: number;
};

export type StateReport = {
  schema_version: number;
  seq: number;
  applied: number;
  failures: FailureReport[];
  dead_letters: DeadReport[];
};

export type CheckStatus = "ok" | "warn" | "fail" | "skip";
export type DoctorCheck = {
  id: string;
  label: string;
  status: CheckStatus;
  detail?: string;
  error?: string;
};
export type DoctorReport = {
  schema_version: number;
  client_version: string;
  hub_version?: string;
  checks: DoctorCheck[];
};

export type JobState = "queued" | "running" | "ok" | "failed";

export type Job = {
  id: string;
  action: string;
  target: string;
  state: JobState;
  code: number | null;
  startedAt: number;
  finishedAt?: number;
  output: string[];
  error?: string;
};

export type AuditEntry = {
  ts: number;
  action: string;
  target: string;
  result: "ok" | "failed";
  code: number | null;
  error?: string;
};
