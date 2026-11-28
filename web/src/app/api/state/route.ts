import { stateJson } from "@/lib/dsync";

export const runtime = "nodejs";
export const dynamic = "force-dynamic";

export async function GET() {
  try {
    const state = await stateJson();
    return Response.json({ updatedAt: Date.now(), state });
  } catch (e) {
    return Response.json(
      { error: { message: e instanceof Error ? e.message : "state list failed" } },
      { status: 502 },
    );
  }
}
