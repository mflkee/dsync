export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET() {
  return Response.json({
    ok: true,
    pid: process.pid,
    now: Date.now(),
    version: "0.1.0",
    authRequired: !!process.env.DSYNC_WEB_TOKEN,
  });
}
