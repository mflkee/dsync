import { checkAccess, errorJson } from "@/lib/http";
import { createJob } from "@/lib/jobs";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function POST(req: Request) {
  const denied = checkAccess(req);
  if (denied) return errorJson(denied, 403);
  const result = createJob("doctor", "doctor", ["doctor", "--json"], 120_000);
  if ("conflict" in result) {
    return Response.json(
      { error: { message: "doctor is already running" }, jobId: result.conflict },
      { status: 409 },
    );
  }
  return Response.json({ job: result.job }, { status: 202 });
}
