import { API_BASE } from "./config.js";

export async function requestJson(ctx, path, query = undefined) {
    const response = await ctx.http.request({
        method: "GET",
        url: `${API_BASE}${path}`,
        headers: { Accept: "application/json" },
        query,
    });
    let payload;
    try {
        payload = JSON.parse(response.body);
    } catch (_error) {
        ctx.fail("INVALID_RESPONSE", `MangaDex returned invalid JSON for ${path}`, { status: response.status });
    }
    if (response.status < 200 || response.status >= 300 || payload.result === "error") {
        const apiError = Array.isArray(payload.errors) ? payload.errors[0] : undefined;
        const message = apiError?.detail || apiError?.title || `HTTP ${response.status}`;
        ctx.fail("HTTP_ERROR", `MangaDex request failed: ${message}`, { status: response.status, path });
    }
    return payload;
}
