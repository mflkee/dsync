"use client";

import { ActionButton } from "@/components/action-button";
import {
  Alert,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
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
import type { StateReport } from "@/lib/types";
import { usePolling } from "@/lib/use-polling";
import { fmtAgo } from "@/lib/utils";

type Payload = { updatedAt: number; state: StateReport };

export default function StatePage() {
  const { data, error, loading, reload } = usePolling<Payload>("/api/state", 10_000);
  const state = data?.state;

  return (
    <div>
      <PageHeader
        title="State"
        description="Локальный non-git state машины, на которой запущена панель."
      />

      <Alert className="mb-4">
        Это state <b>этой</b> машины: tmux-снапшоты и сессии opencode применяются локально.
        Удалённый retry с другой машины отсюда недоступен.
      </Alert>

      {error ? <p className="mb-4 text-sm text-red-300">Ошибка: {error}</p> : null}

      {loading && !state ? (
        <Skeleton className="h-40" />
      ) : (
        <>
          <div className="mb-4 grid gap-3 md:grid-cols-4">
            <Card>
              <CardHeader><CardTitle>seq</CardTitle></CardHeader>
              <CardContent><p className="text-2xl text-slate-100">{state?.seq ?? 0}</p></CardContent>
            </Card>
            <Card>
              <CardHeader><CardTitle>применено</CardTitle></CardHeader>
              <CardContent><p className="text-2xl text-slate-100">{state?.applied ?? 0}</p></CardContent>
            </Card>
            <Card>
              <CardHeader><CardTitle>failing</CardTitle></CardHeader>
              <CardContent><p className="text-2xl text-amber-300">{state?.failures.length ?? 0}</p></CardContent>
            </Card>
            <Card>
              <CardHeader><CardTitle>dead-letter</CardTitle></CardHeader>
              <CardContent><p className="text-2xl text-red-300">{state?.dead_letters.length ?? 0}</p></CardContent>
            </Card>
          </div>

          <Card className="mb-4 overflow-hidden">
            <CardHeader className="flex-row items-center justify-between">
              <CardTitle>Dead-letter</CardTitle>
              <ActionButton
                payload={{ type: "state-retry" }}
                label="retry all"
                size="sm"
                variant="outline"
                confirm="Повторить все dead-letter элементы?"
                onDone={() => void reload()}
              />
            </CardHeader>
            <CardContent>
              {!state || state.dead_letters.length === 0 ? (
                <Empty title="Dead-letter пуст" />
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Ключ</TableHead>
                      <TableHead>Ошибка</TableHead>
                      <TableHead>Когда</TableHead>
                      <TableHead className="text-right">Действие</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {state.dead_letters.map((d) => (
                      <TableRow key={d.key}>
                        <TableCell className="font-mono text-xs text-slate-300">{d.key}</TableCell>
                        <TableCell className="text-xs text-red-300">{d.error}</TableCell>
                        <TableCell className="text-xs text-slate-500">{fmtAgo(d.at)}</TableCell>
                        <TableCell className="text-right">
                          <ActionButton
                            payload={{ type: "state-retry", key: d.key }}
                            label="retry"
                            size="sm"
                            variant="outline"
                            onDone={() => void reload()}
                          />
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>

          <Card className="overflow-hidden">
            <CardHeader><CardTitle>Failing (будут повторены)</CardTitle></CardHeader>
            <CardContent>
              {!state || state.failures.length === 0 ? (
                <Empty title="Сбойных элементов нет" />
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead>Ключ</TableHead>
                      <TableHead>Попытки</TableHead>
                      <TableHead>seq</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {state.failures.map((f) => (
                      <TableRow key={f.key}>
                        <TableCell className="font-mono text-xs text-slate-300">{f.key}</TableCell>
                        <TableCell className="text-slate-400">{f.attempts}</TableCell>
                        <TableCell className="text-slate-500">{f.seq}</TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>
        </>
      )}
    </div>
  );
}
