import type { StatusReport } from "@/lib/types";

export type ActionInput = {
  type?: unknown;
  machine?: unknown;
  project?: unknown;
  key?: unknown;
  force?: unknown;
};

export type ResolvedAction = {
  action: string;
  target: string;
  args: string[];
  timeout: number;
};

function str(v: unknown): string | undefined {
  return typeof v === "string" && v.trim() ? v.trim() : undefined;
}

function validName(v: string): boolean {
  return /^[A-Za-z0-9._-]+$/.test(v);
}

function machineExists(status: StatusReport | null, machine: string): boolean {
  return !!status && Object.prototype.hasOwnProperty.call(status.machines, machine);
}

export function resolveAction(
  input: ActionInput,
  status: StatusReport | null,
): ResolvedAction | { error: string } {
  const type = str(input.type);
  const machine = str(input.machine);
  const project = str(input.project);
  const key = str(input.key);
  const force = input.force === true;

  if (!type) return { error: "action.type is required" };
  if (machine && !validName(machine)) return { error: "invalid machine name" };
  if (project && !validName(project)) return { error: "invalid project name" };
  if (key && !/^[a-z]+:[A-Za-z0-9._-]+$/.test(key)) return { error: "invalid state key" };

  switch (type) {
    case "push": {
      if (machine && !machineExists(status, machine)) {
        return { error: `unknown machine ${machine}` };
      }
      const args = machine ? ["push", machine] : ["push"];
      return { action: "push", target: `push:${machine ?? "all"}`, args, timeout: 180_000 };
    }
    case "pull": {
      if (machine && !machineExists(status, machine)) {
        return { error: `unknown machine ${machine}` };
      }
      const args = machine ? ["pull", machine] : ["pull"];
      return { action: "pull", target: `pull:${machine ?? "all"}`, args, timeout: 180_000 };
    }
    case "deploy": {
      if (!machine) return { error: "deploy requires machine" };
      if (!machineExists(status, machine)) return { error: `unknown machine ${machine}` };
      const args = ["deploy", "--machine", machine];
      if (project) args.push("--project", project);
      if (force) args.push("--force");
      return {
        action: "deploy",
        target: `deploy:${machine}:${project ?? "all"}`,
        args,
        timeout: 300_000,
      };
    }
    case "state-retry": {
      const args = key ? ["state", "retry", key] : ["state", "retry"];
      return {
        action: "state-retry",
        target: `state-retry:${key ?? "all"}`,
        args,
        timeout: 60_000,
      };
    }
    case "doctor": {
      return {
        action: "doctor",
        target: "doctor",
        args: ["doctor", "--json"],
        timeout: 120_000,
      };
    }
    default:
      return { error: `unknown action ${type}` };
  }
}
