"use client";

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
import { fmtAgo, fmtTime } from "@/lib/utils";

type Payload = { updatedAt: number; status: StatusReport };

export default function MachinesPage() {
  const [interval] = useRefreshInterval();
  const { data, error, reload, loading } = usePolling<Payload>(
    "/api/status",
    interval * 1000,
  );
  const machines = Object.entries(data?.status.machines ?? {});

  return (
    <div>
      <PageHeader title="Machines" description="Машины флота и последние исходы SSH-пуллов." />
      {error ? <p className="mb-4 text-sm text-red-300">Хаб недоступен: {error}</p> : null}

      {loading && !data ? (
        <Skeleton className="h-48" />
      ) : machines.length === 0 ? (
        <Empty title="Нет машин" description="Хаб ещё не получал push'ей." />
      ) : (
        <Card className="overflow-hidden">
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Машина</TableHead>
                <TableHead>Статус</TableHead>
                <TableHead>Последний push</TableHead>
                <TableHead>Версия</TableHead>
                <TableHead>Проблемы</TableHead>
                <TableHead className="text-right">Действия</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {machines
                .sort(([a], [b]) => a.localeCompare(b))
                .map(([name, m]) => {
                  const failed = Object.entries(m.pulls).filter(([, p]) => !p.ok);
                  return (
                    <TableRow key={name}>
                      <TableCell className="font-medium text-slate-200">{name}</TableCell>
                      <TableCell>
                        <Badge variant={m.online ? "success" : "secondary"}>
                          {m.online ? "online" : "offline"}
                        </Badge>
                      </TableCell>
                      <TableCell className="text-slate-400">
                        {fmtAgo(m.last_push)}
                        <div className="text-[11px] text-slate-600">{fmtTime(m.last_push)}</div>
                      </TableCell>
                      <TableCell>
                        <span className="text-xs text-slate-400">{m.version ?? "unknown"}</span>
                      </TableCell>
                      <TableCell>
                        {failed.length === 0 ? (
                          <Badge variant="success">ok</Badge>
                        ) : (
                          <details>
                            <summary className="cursor-pointer">
                              <Badge variant="destructive">{failed.length} failed</Badge>
                            </summary>
                            <div className="mt-2 space-y-1 text-xs text-slate-400">
                              {failed.map(([project, p]) => (
                                <div key={project}>
                                  <span className="text-slate-300">{project}</span>:{" "}
                                  {p.error ?? "failed"} ({p.attempts} поп.)
                                </div>
                              ))}
                            </div>
                          </details>
                        )}
                      </TableCell>
                      <TableCell className="text-right">
                        <div className="flex justify-end gap-2">
                          <ActionButton
                            payload={{ type: "push", machine: name }}
                            label="push"
                            size="sm"
                            variant="outline"
                            onDone={() => void reload()}
                          />
                          <ActionButton
                            payload={{ type: "pull", machine: name }}
                            label="pull"
                            size="sm"
                            variant="outline"
                            onDone={() => void reload()}
                          />
                        </div>
                      </TableCell>
                    </TableRow>
                  );
                })}
            </TableBody>
          </Table>
        </Card>
      )}
    </div>
  );
}
