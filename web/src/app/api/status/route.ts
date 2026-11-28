import { statusJson } from "@/lib/dsync";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET() {
  try {
    const status = await statusJson();
    return Response.json({ updatedAt: Date.now(), status });
  } catch (e) {
    return Response.json(
      { error: { message: e instanceof Error ? e.message : "status failed" } },
      { status: 502 },
    );
  }
}
