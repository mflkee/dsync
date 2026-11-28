import { statusJson } from "@/lib/dsync";
import { checkAccess, errorJson } from "@/lib/http";
import { createJob } from "@/lib/jobs";
import { resolveAction } from "@/lib/allowlist";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function POST(req: Request) {
  const denied = checkAccess(req);
  if (denied) return errorJson(denied, 403);

  let body: unknown;
  try {
    body = await req.json();
  } catch {
    return errorJson("invalid JSON body", 400);
  }

  const status = await statusJson().catch(() => null);
  const resolved = resolveAction((body ?? {}) as Record<string, unknown>, status);
  if ("error" in resolved) return errorJson(resolved.error, 400);

  const result = createJob(resolved.action, resolved.target, resolved.args, resolved.timeout);
  if ("conflict" in result) {
    return Response.json(
      {
        error: { message: "already running for this target" },
        jobId: result.conflict,
      },
      { status: 409 },
    );
  }
  return Response.json({ job: result.job }, { status: 202 });
}
