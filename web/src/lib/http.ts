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

export function errorJson(message: string, status = 400) {
  return Response.json({ error: { message } }, { status });
}
