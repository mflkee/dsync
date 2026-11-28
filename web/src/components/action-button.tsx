"use client";

import { useState } from "react";

import { Button, Spinner, type ButtonProps } from "@/components/ui";
import type { Job } from "@/lib/types";

type Props = {
  payload: Record<string, unknown>;
  label: string;
  confirm?: string;
  variant?: ButtonProps["variant"];
  size?: ButtonProps["size"];
  onDone?: () => void;
};

async function waitJob(id: string): Promise<Job> {
  for (let i = 0; i < 600; i += 1) {
    await new Promise((r) => setTimeout(r, 1000));
    const res = await fetch(`/api/jobs/${id}`, { cache: "no-store" });
    if (!res.ok) continue;
    const body = (await res.json()) as { job?: Job };
    if (body.job && (body.job.state === "ok" || body.job.state === "failed")) {
      return body.job;
    }
  }
  throw new Error("таймаут ожидания задачи");
}

export function ActionButton({ payload, label, confirm, variant, size, onDone }: Props) {
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [ok, setOk] = useState(true);

  async function run() {
    if (confirm && !window.confirm(confirm)) return;
    setBusy(true);
    setMessage(null);
    try {
      const res = await fetch("/api/actions", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      const body = (await res.json()) as {
        job?: Job;
        jobId?: string;
        error?: { message?: string };
      };
      if (!res.ok) {
        setOk(false);
        setMessage(body.error?.message ?? `HTTP ${res.status}`);
        return;
      }
      const id = body.job?.id ?? body.jobId;
      if (!id) throw new Error("сервер не вернул job id");
      const job = await waitJob(id);
      setOk(job.state === "ok");
      setMessage(
        job.state === "ok"
          ? (job.output.at(-1) ?? "готово")
          : (job.error ?? job.output.at(-1) ?? "ошибка"),
      );
      onDone?.();
    } catch (e) {
      setOk(false);
      setMessage(e instanceof Error ? e.message : "ошибка");
    } finally {
      setBusy(false);
    }
  }

  return (
    <span className="inline-flex items-center gap-2">
      <Button
        type="button"
        variant={variant}
        size={size}
        disabled={busy}
        onClick={() => void run()}
      >
        {busy ? <Spinner /> : null}
        {label}
      </Button>
      {message ? (
        <span
          title={message}
          className={
            ok
              ? "max-w-[18rem] truncate text-xs text-emerald-300"
              : "max-w-[18rem] truncate text-xs text-red-300"
          }
        >
          {message}
        </span>
      ) : null}
    </span>
  );
}
