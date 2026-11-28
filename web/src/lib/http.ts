export function checkAccess(req: Request): string | null {
  const token = process.env.DSYNC_WEB_TOKEN;
  if (token) {
    const got = req.headers.get("authorization");
    if (got !== `Bearer ${token}`) return "unauthorized";
  }
  if (req.method !== "GET" && req.method !== "HEAD") {
    const origin = req.headers.get("origin");
    if (origin) {
      try {
        const host = new URL(origin).hostname;
        if (!["127.0.0.1", "localhost", "::1"].includes(host)) {
          return "cross-origin request rejected";
        }
      } catch {
        return "bad origin";
      }
    }
  }
  return null;
}

export type JsonRead =
  | { ok: true; body: unknown }
  | { ok: false; status: number; message: string };

export async function readJson(req: Request, maxBytes = 16 * 1024): Promise<JsonRead> {
  const ct = (req.headers.get("content-type") ?? "").toLowerCase();
  if (!ct.includes("application/json")) {
    return { ok: false, status: 415, message: "Content-Type must be application/json" };
  }
  const len = Number(req.headers.get("content-length") ?? 0);
  if (Number.isFinite(len) && len > maxBytes) {
    return { ok: false, status: 413, message: "request body too large" };
  }
  const text = await req.text();
  if (new TextEncoder().encode(text).length > maxBytes) {
    return { ok: false, status: 413, message: "request body too large" };
  }
  try {
    return { ok: true, body: JSON.parse(text) };
  } catch {
    return { ok: false, status: 400, message: "invalid JSON body" };
  }
}

export function errorJson(message: string, status = 400) {
  return Response.json({ error: { message } }, { status });
}
