"use client";

import { useEffect, useState } from "react";
import { Activity, AlertTriangle, Clock3, Server, Skull } from "lucide-react";

import { ActionButton } from "@/components/action-button";
import {
  Alert,
  Badge,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  PageHeader,
  Skeleton,
} from "@/components/ui";
import type { StatusReport } from "@/lib/types";
import { usePolling } from "@/lib/use-polling";
import { useRefreshInterval } from "@/lib/use-refresh-interval";
import { fmtAgo, fmtTime } from "@/lib/utils";

type Payload = { updatedAt: number; status: StatusReport };

function Stat({
  label,
  value,
  hint,
  tone = "default",
}: {
  label: string;
  value: string | number;
  hint?: string;
  tone?: "default" | "danger" | "warning" | "success";
}) {
  const color =
    tone === "danger"
      ? "text-red-300"
      : tone === "warning"
        ? "text-amber-300"
        : tone === "success"
          ? "text-emerald-300"
          : "text-slate-100";
  return (
    <Card>
      <CardHeader>
        <CardTitle>{label}</CardTitle>
      </CardHeader>
      <CardContent>
        <p className={`text-3xl font-semibold ${color}`}>{value}</p>
        {hint ? <p className="mt-1 text-xs text-slate-500">{hint}</p> : null}
      </CardContent>
    </Card>
  );
}

export default function DashboardPage() {
  const [interval] = useRefreshInterval();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), 5_000);
    return () => clearInterval(id);
  }, []);
  const { data, error, updatedAt, loading, reload } = usePolling<Payload>(
    "/api/status",
    interval * 1000,
  );
  const status = data?.status;
  const machineEntries = Object.entries(status?.machines ?? {}).sort(
    (a, b) => b[1].last_push - a[1].last_push,
  );
  const machines = machineEntries.map(([, m]) => m);
  const online = machines.filter((m) => m.online).length;
  const offline = machines.length - online;
  const drift = status
    ? machines.filter((m) => m.version && m.version !== status.hub_version).length
    : 0;
  const failedPulls = machines.reduce(
    (n, m) => n + Object.values(m.pulls).filter((p) => !p.ok).length,
    0,
  );
  const stale = !!updatedAt && now - updatedAt > interval * 3 * 1000;

  return (
    <div>
      <PageHeader
        title="Dashboard"
        description="Здоровье флота по данным хаба и локального state-индекса."
      >
        <div className="flex flex-wrap items-center gap-2">
          <ActionButton payload={{ type: "push" }} label="Push" size="sm" onDone={() => void reload()} />
          <ActionButton payload={{ type: "pull" }} label="Pull" size="sm" onDone={() => void reload()} />
          <ActionButton payload={{ type: "doctor" }} label="Doctor" size="sm" variant="secondary" onDone={() => void reload()} />
        </div>
      </PageHeader>

      {error ? (
        <Alert variant="danger" className="mb-4">
          <div className="flex items-center gap-2">
            <AlertTriangle className="h-4 w-4" />
            <span>Хаб недоступен: {error}</span>
          </div>
        </Alert>
      ) : null}

      <div className="mb-4 flex flex-wrap items-center gap-2 text-xs text-slate-500">
        <Clock3 className="h-3.5 w-3.5" />
        {updatedAt ? <span>обновлено {fmtTime(updatedAt / 1000)}</span> : null}
        {stale ? <Badge variant="warning">данные устарели</Badge> : null}
        {status ? (
          <Badge variant="outline">client {status.client_version}</Badge>
        ) : null}
        {status?.hub_version ? <Badge variant="outline">hub {status.hub_version}</Badge> : null}
      </div>

      {loading && !status ? (
        <div className="grid gap-3 md:grid-cols-4">
          {[0, 1, 2, 3].map((i) => (
            <Skeleton key={i} className="h-28" />
          ))}
        </div>
      ) : (
        <div className="grid gap-3 md:grid-cols-4">
          <Stat
            label="Машины online"
            value={online}
            hint={offline ? `${offline} offline` : "весь флот на связи"}
            tone={offline ? "warning" : "success"}
          />
          <Stat
            label="Дрейф версий"
            value={drift}
            hint="клиент ≠ хаб"
            tone={drift ? "warning" : "success"}
          />
          <Stat
            label="Упавшие pull'ы"
            value={failedPulls}
            hint="по последним исходам"
            tone={failedPulls ? "danger" : "success"}
          />
          <Stat
            label="Dead-letter"
            value={status?.local_state.dead_letter ?? 0}
            hint={`state seq ${status?.local_state.seq ?? 0}`}
            tone={(status?.local_state.dead_letter ?? 0) ? "danger" : "success"}
          />
        </div>
      )}

      <div className="mt-6 grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Server className="h-4 w-4" /> Последние push&apos;и
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-2">
            {machines.length === 0 ? (
              <p className="text-sm text-slate-500">Нет данных о машинах.</p>
            ) : (
              machineEntries
                .slice(0, 6)
                .map(([name, m]) => {
                  return (
                    <div key={name} className="flex items-center justify-between gap-3 text-sm">
                      <span className="flex items-center gap-2 text-slate-300">
                        <span
                          className={`h-2 w-2 rounded-full ${m.online ? "bg-emerald-400" : "bg-slate-600"}`}
                        />
                        {name}
                      </span>
                      <span className="text-xs text-slate-500">{fmtAgo(m.last_push)}</span>
                    </div>
                  );
                })
            )}
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2">
              <Activity className="h-4 w-4" /> Локальный state
            </CardTitle>
          </CardHeader>
          <CardContent className="grid grid-cols-2 gap-3 text-sm">
            <div>
              <p className="text-xs text-slate-500">seq</p>
              <p className="text-lg text-slate-200">{status?.local_state.seq ?? 0}</p>
            </div>
            <div>
              <p className="text-xs text-slate-500">применено</p>
              <p className="text-lg text-slate-200">{status?.local_state.applied ?? 0}</p>
            </div>
            <div>
              <p className="text-xs text-slate-500">failing</p>
              <p className="text-lg text-amber-300">{status?.local_state.failing ?? 0}</p>
            </div>
            <div>
              <p className="text-xs text-slate-500">dead-letter</p>
              <p className="flex items-center gap-1 text-lg text-red-300">
                <Skull className="h-4 w-4" />
                {status?.local_state.dead_letter ?? 0}
              </p>
            </div>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
