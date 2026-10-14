import { describe, expect, it, vi } from "vitest";
import { handleRequest, type Env } from "../src/index";

// The static callback page must not require token-exchange configuration.
const unconfigured = (): Env => ({});
const deps = () => ({ fetch: vi.fn<typeof globalThis.fetch>() });

function callback(query = "", method = "GET") {
  return new Request(`https://broker.example/callback${query}`, { method });
}

describe("callback page", () => {
  it("serves GET /callback with hardened static headers", async () => {
    const response = await handleRequest(callback("?code=abc&state=xyz"), unconfigured(), deps());
    expect(response.status).toBe(200);
    expect(response.headers.get("Content-Type")).toBe("text/html; charset=utf-8");
    expect(response.headers.get("Cache-Control")).toBe("no-store");
    expect(response.headers.get("Pragma")).toBe("no-cache");
    expect(response.headers.get("Referrer-Policy")).toBe("no-referrer");
    expect(response.headers.get("X-Content-Type-Options")).toBe("nosniff");
    expect(response.headers.get("X-Frame-Options")).toBe("DENY");
    const csp = response.headers.get("Content-Security-Policy") ?? "";
    expect(csp).toContain("default-src 'none'");
    expect(csp).toContain("frame-ancestors 'none'");
    expect(csp).toContain("base-uri 'none'");
    expect(csp).toContain("form-action 'none'");
    expect(csp).not.toContain("unsafe-inline");
    expect(csp).not.toContain("unsafe-eval");
  });

  it("allows only the inline script and style through a per-response nonce", async () => {
    const response = await handleRequest(callback(), unconfigured(), deps());
    const html = await response.text();
    const csp = response.headers.get("Content-Security-Policy") ?? "";
    const nonce = /'nonce-([A-Za-z0-9+/=_-]+)'/.exec(csp)?.[1];
    expect(nonce).toBeTruthy();
    expect(csp).toContain(`script-src 'nonce-${nonce}'`);
    expect(csp).toContain(`style-src 'nonce-${nonce}'`);
    const attributes = html.match(/nonce="[^"]+"/g) ?? [];
    expect(attributes).toHaveLength(2);
    for (const attribute of attributes) expect(attribute).toBe(`nonce="${nonce}"`);
  });

  it("never echoes the request code or state into the page", async () => {
    const code = "CANARY_CODE_7f3a";
    const state = "CANARY_STATE_9b1c";
    const response = await handleRequest(
      callback(`?code=${code}&state=${state}&extra=CANARY_EXTRA`),
      unconfigured(),
      deps(),
    );
    expect(response.status).toBe(200);
    const html = await response.text();
    expect(html).not.toContain(code);
    expect(html).not.toContain(state);
    expect(html).not.toContain("CANARY");
  });

  it("renders the copy UI, CLI instruction and desktop link", async () => {
    const response = await handleRequest(callback(), unconfigured(), deps());
    expect(response.status).toBe(200);
    const html = await response.text();
    expect(html).toContain('id="callback-url"');
    expect(html).toContain("readonly");
    expect(html).toContain("Copy");
    expect(html).toContain("ghostlink login complete");
    expect(html).toContain("ghostlink login start");
    expect(html).toContain("ghostlink://oauth/callback");
    expect(html).toContain("Open in Ghostlink on this computer");
  });

  it("describes denied and missing results client-side", async () => {
    const response = await handleRequest(callback(), unconfigured(), deps());
    expect(response.status).toBe(200);
    const html = await response.text();
    expect(html).toContain("not approved");
    expect(html).toContain("No authorization result");
  });

  it("loads no external resources", async () => {
    const response = await handleRequest(callback(), unconfigured(), deps());
    expect(response.status).toBe(200);
    const html = await response.text();
    expect(html).not.toMatch(/\b(?:src|href)="https?:/u);
  });

  it("serves HEAD /callback with the same headers and no body", async () => {
    const response = await handleRequest(callback("?code=x", "HEAD"), unconfigured(), deps());
    expect(response.status).toBe(200);
    expect(response.headers.get("Content-Type")).toBe("text/html; charset=utf-8");
    expect(await response.text()).toBe("");
  });

  it.each(["POST", "PUT", "DELETE", "OPTIONS", "PATCH"])("rejects %s on /callback", async (method) => {
    const response = await handleRequest(callback("?code=x", method), unconfigured(), deps());
    expect(response.status).toBe(405);
    expect(response.headers.get("Allow")).toBe("GET, HEAD");
  });

  it("does not consume the token-exchange rate limit", async () => {
    const limit = vi.fn().mockResolvedValue({ success: false });
    const response = await handleRequest(callback("?code=x"), { RATE_LIMITER: { limit } }, deps());
    expect(response.status).toBe(200);
    expect(limit).not.toHaveBeenCalled();
  });
});
