"use client";

import {
  Badge,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Empty,
  PageHeader,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui";
import type { AuditEntry, Job } from "@/lib/types";
import { usePolling } from "@/lib/use-polling";
import { fmtTime } from "@/lib/utils";

function jobBadge(state: Job["state"]) {
  if (state === "ok") return <Badge variant="success">ok</Badge>;
  if (state === "failed") return <Badge variant="destructive">failed</Badge>;
  if (state === "running") return <Badge variant="default">running</Badge>;
  return <Badge variant="secondary">queued</Badge>;
}

export default function ActivityPage() {
  const jobs = usePolling<{ jobs: Job[] }>("/api/jobs", 5_000);
  const audit = usePolling<{ entries: AuditEntry[] }>("/api/audit", 5_000);

  return (
    <div>
      <PageHeader title="Activity" description="Задачи панели и журнал действий." />

      <div className="grid gap-4 xl:grid-cols-2">
        <Card className="overflow-hidden">
          <CardHeader><CardTitle>Задачи</CardTitle></CardHeader>
          <CardContent>
            {!jobs.data?.jobs.length ? (
              <Empty title="Задач пока нет" />
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Действие</TableHead>
                    <TableHead>Цель</TableHead>
                    <TableHead>Статус</TableHead>
                    <TableHead>Вывод</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {jobs.data.jobs.map((j) => (
                    <TableRow key={j.id}>
                      <TableCell className="text-slate-300">{j.action}</TableCell>
                      <TableCell className="text-xs text-slate-400">{j.target}</TableCell>
                      <TableCell>{jobBadge(j.state)}</TableCell>
                      <TableCell className="max-w-[22rem]">
                        <p className="truncate text-xs text-slate-500" title={j.output.at(-1)}>
                          {j.output.at(-1) ?? j.error ?? "—"}
                        </p>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>

        <Card className="overflow-hidden">
          <CardHeader><CardTitle>Аудит</CardTitle></CardHeader>
          <CardContent>
            {!audit.data?.entries.length ? (
              <Empty title="Журнал пуст" />
            ) : (
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>Время</TableHead>
                    <TableHead>Действие</TableHead>
                    <TableHead>Цель</TableHead>
                    <TableHead>Результат</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {audit.data.entries.map((e, i) => (
                    <TableRow key={`${e.ts}-${i}`}>
                      <TableCell className="text-xs text-slate-500">
                        {fmtTime(Math.floor(e.ts / 1000))}
                      </TableCell>
                      <TableCell className="text-slate-300">{e.action}</TableCell>
                      <TableCell className="text-xs text-slate-400">{e.target}</TableCell>
                      <TableCell>
                        {e.result === "ok" ? (
                          <Badge variant="success">ok</Badge>
                        ) : (
                          <details>
                            <summary className="cursor-pointer">
                              <Badge variant="destructive">failed</Badge>
                            </summary>
                            <p className="mt-1 max-w-sm text-xs text-red-300">{e.error}</p>
                          </details>
                        )}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
