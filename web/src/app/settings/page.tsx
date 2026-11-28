"use client";

import {
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  PageHeader,
  Select,
} from "@/components/ui";
import { usePolling } from "@/lib/use-polling";
import { useRefreshInterval } from "@/lib/use-refresh-interval";

type Health = { ok: boolean; version: string; authRequired?: boolean };

export default function SettingsPage() {
  const [interval, setInterval] = useRefreshInterval();
  const health = usePolling<Health>("/api/health", 30_000);

  return (
    <div>
      <PageHeader
        title="Settings"
        description="Локальные настройки панели. Конфиг dsync и токены здесь не редактируются."
      />

      <div className="grid gap-4 lg:grid-cols-2">
        <Card>
          <CardHeader><CardTitle>Обновление данных</CardTitle></CardHeader>
          <CardContent className="space-y-3">
            <label className="block text-sm text-slate-400" htmlFor="interval">
              Интервал опроса (секунды)
            </label>
            <Select
              id="interval"
              value={interval}
              onChange={(e) => setInterval(Number(e.target.value))}
            >
              {[5, 10, 15, 30, 60].map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </Select>
            <p className="text-xs text-slate-500">
              Применяется к Dashboard; страницы с фиксированным шагом обновляются раз в 10 с.
              При скрытой вкладке опрос приостанавливается.
            </p>
          </CardContent>
        </Card>

        <Card>
          <CardHeader><CardTitle>Панель</CardTitle></CardHeader>
          <CardContent className="space-y-2 text-sm text-slate-400">
            <p>
              Bind: <span className="text-slate-200">127.0.0.1:3133</span>
            </p>
            <p>
              Версия панели: <span className="text-slate-200">{health.data?.version ?? "—"}</span>
            </p>
            <p>
              Токен панели (DSYNC_WEB_TOKEN):{" "}
              <span className="text-slate-200">
                {health.data?.authRequired ? "включён" : "не требуется для loopback"}
              </span>
            </p>
            <p>
              Тема: <span className="text-slate-200">тёмная (фиксирована)</span>
            </p>
            <p className="text-xs text-slate-500">
              Hub/token значение никогда не отдаётся в браузер.
            </p>
          </CardContent>
        </Card>
      </div>
    </div>
  );
}
