"use client";

import { useState } from "react";

import {
  Alert,
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  PageHeader,
  Spinner,
} from "@/components/ui";
import type { DoctorReport, Job } from "@/lib/types";

function badge(status: string) {
  if (status === "ok") return <Badge variant="success">ok</Badge>;
  if (status === "warn") return <Badge variant="warning">warn</Badge>;
  if (status === "fail") return <Badge variant="destructive">fail</Badge>;
  return <Badge variant="secondary">skip</Badge>;
}

async function waitDoctor(id: string): Promise<Job> {
  for (let i = 0; i < 180; i += 1) {
    await new Promise((r) => setTimeout(r, 1000));
    const res = await fetch(`/api/jobs/${id}`, { cache: "no-store" });
    if (!res.ok) continue;
    const body = (await res.json()) as { job?: Job };
    if (body.job && (body.job.state === "ok" || body.job.state === "failed")) return body.job;
  }
  throw new Error("таймаут doctor");
}

export default function DoctorPage() {
  const [busy, setBusy] = useState(false);
  const [report, setReport] = useState<DoctorReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [when, setWhen] = useState<Date | null>(null);

  async function run() {
    setBusy(true);
    setError(null);
    try {
      const res = await fetch("/api/doctor", { method: "POST" });
      const body = (await res.json()) as { job?: Job; jobId?: string; error?: { message?: string } };
      if (!res.ok && !body.jobId) throw new Error(body.error?.message ?? `HTTP ${res.status}`);
      const id = body.job?.id ?? body.jobId;
      if (!id) throw new Error("нет job id");
      const job = await waitDoctor(id);
      if (job.state !== "ok") {
        setError(job.error ?? job.output.at(-1) ?? "doctor failed");
      }
      const text = job.output.join("\n");
      const start = text.indexOf("{");
      if (start >= 0) setReport(JSON.parse(text.slice(start)) as DoctorReport);
      setWhen(new Date());
    } catch (e) {
      setError(e instanceof Error ? e.message : "ошибка");
    } finally {
      setBusy(false);
    }
  }

  return (
    <div>
      <PageHeader title="Doctor" description="Проверки конфига, хаба, ключей и state.">
        <Button onClick={() => void run()} disabled={busy}>
          {busy ? <Spinner /> : null}
          Запустить
        </Button>
      </PageHeader>

      {error ? <Alert variant="danger" className="mb-4">{error}</Alert> : null}
      {when ? <p className="mb-3 text-xs text-slate-500">запуск: {when.toLocaleString("ru-RU")}</p> : null}

      {!report ? (
        <Alert>Нажми «Запустить», чтобы выполнить `dsync doctor --json`.</Alert>
      ) : (
        <Card className="overflow-hidden">
          <CardHeader>
            <CardTitle>
              {report.checks.filter((c) => c.status === "fail").length === 0
                ? "Провалов нет"
                : "Есть провалы"}
            </CardTitle>
            <p className="text-xs text-slate-500">
              client {report.client_version} · hub {report.hub_version ?? "unknown"}
            </p>
          </CardHeader>
          <CardContent className="space-y-2">
            {report.checks.map((c) => (
              <details key={c.id} className="rounded-md border border-slate-800 px-3 py-2">
                <summary className="flex cursor-pointer list-none items-center justify-between gap-3">
                  <span className="flex items-center gap-2 text-sm text-slate-200">
                    {badge(c.status)} {c.label}
                    <span className="text-xs text-slate-600">{c.id}</span>
                  </span>
                </summary>
                {c.detail || c.error ? (
                  <p className="mt-2 whitespace-pre-wrap text-xs text-slate-400">
                    {c.error ?? c.detail}
                  </p>
                ) : null}
              </details>
            ))}
          </CardContent>
        </Card>
      )}
    </div>
  );
}
