"use client";

import { useState } from "react";

import { ActionButton } from "@/components/action-button";
import {
  Badge,
  Card,
  Empty,
  PageHeader,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui";
import type { StatusReport } from "@/lib/types";
import { usePolling } from "@/lib/use-polling";
import { useRefreshInterval } from "@/lib/use-refresh-interval";
import { fmtAgo } from "@/lib/utils";

type Payload = { updatedAt: number; status: StatusReport };

type Row = {
  machine: string;
  project: string;
  branch: string;
  commit: string;
  dirty: boolean;
  pull?: { ok: boolean; attempts: number; finished_at: number; error?: string };
};

export default function ProjectsPage() {
  const [interval] = useRefreshInterval();
  const { data, error, reload, loading } = usePolling<Payload>(
    "/api/status",
    interval * 1000,
  );
  const [onlyProblems, setOnlyProblems] = useState(false);

  const rows: Row[] = [];
  for (const [machine, m] of Object.entries(data?.status.machines ?? {})) {
    for (const p of m.projects ?? []) {
      rows.push({
        machine,
        project: p.name,
        branch: p.branch,
        commit: p.commit_hash,
        dirty: p.dirty,
        pull: m.pulls[p.name],
      });
    }
    for (const [project, pull] of Object.entries(m.pulls)) {
      if (!(m.projects ?? []).some((p) => p.name === project)) {
        rows.push({ machine, project, branch: "—", commit: "", dirty: false, pull });
      }
    }
  }
  const problems = rows.filter((r) => r.pull && !r.pull.ok);
  const visible = (onlyProblems ? problems : rows).sort((a, b) =>
    `${a.machine}/${a.project}`.localeCompare(`${b.machine}/${b.project}`),
  );

  return (
    <div>
      <PageHeader title="Projects" description="Матрица «машина × проект» по последним push'ам.">
        <label className="flex items-center gap-2 text-sm text-slate-400">
          <input
            type="checkbox"
            checked={onlyProblems}
            onChange={(e) => setOnlyProblems(e.target.checked)}
            className="h-4 w-4 rounded border-slate-700 bg-slate-950"
          />
          только проблемы
          {onlyProblems ? (
            <Badge variant="secondary">{Math.max(0, rows.length - problems.length)} скрыто</Badge>
          ) : null}
        </label>
      </PageHeader>

      {error ? <p className="mb-4 text-sm text-red-300">Хаб недоступен: {error}</p> : null}

      {loading && !data ? (
        <Skeleton className="h-48" />
      ) : visible.length === 0 ? (
        <Empty
          title={onlyProblems ? "Проблем нет" : "Проектов нет"}
          description={
            onlyProblems ? "Все последние pull'ы успешны." : "Хаб не получал состояний проектов."
          }
        />
      ) : (
        <Card className="overflow-hidden">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Машина</TableHead>
                <TableHead>Проект</TableHead>
                <TableHead>Ветка</TableHead>
                <TableHead>Commit</TableHead>
                <TableHead>Pull</TableHead>
                <TableHead className="text-right">Действия</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {visible.map((r) => (
                <TableRow key={`${r.machine}/${r.project}`}>
                  <TableCell className="text-slate-300">{r.machine}</TableCell>
                  <TableCell className="font-medium text-slate-200">
                    {r.project}
                    {r.dirty ? <Badge variant="warning" className="ml-2">dirty</Badge> : null}
                  </TableCell>
                  <TableCell className="text-slate-400">{r.branch}</TableCell>
                  <TableCell className="font-mono text-xs text-slate-500">
                    {r.commit ? r.commit.slice(0, 8) : "—"}
                  </TableCell>
                  <TableCell>
                    {!r.pull ? (
                      <Badge variant="secondary">нет данных</Badge>
                    ) : r.pull.ok ? (
                      <span className="text-xs text-emerald-300">
                        ok · {fmtAgo(r.pull.finished_at)}
                      </span>
                    ) : (
                      <details>
                        <summary className="cursor-pointer">
                          <Badge variant="destructive">failed</Badge>
                        </summary>
                        <p className="mt-1 max-w-sm text-xs text-red-300">{r.pull.error}</p>
                      </details>
                    )}
                  </TableCell>
                  <TableCell className="text-right">
                    <div className="flex justify-end gap-2">
                      <ActionButton
                        payload={{ type: "deploy", machine: r.machine, project: r.project }}
                        label="deploy"
                        size="sm"
                        variant="outline"
                        confirm={`Развернуть ${r.project} на ${r.machine}?`}
                        onDone={() => void reload()}
                      />
                      <ActionButton
                        payload={{
                          type: "deploy",
                          machine: r.machine,
                          project: r.project,
                          force: true,
                        }}
                        label="force"
                        size="sm"
                        variant="ghost"
                        confirm={`Force deploy ${r.project} на ${r.machine}?`}
                        onDone={() => void reload()}
                      />
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </Card>
      )}
    </div>
  );
}
