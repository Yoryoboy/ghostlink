import { describe, expect, it, vi } from "vitest";
import { handleRequest, type Env } from "../src/index";

const endpoint = "https://www.bungie.net/platform/app/oauth/token/";
const env: Env = {
  BUNGIE_CLIENT_ID: "id :/+%&é",
  BUNGIE_CLIENT_SECRET: "secret :/+%&é",
  ALLOWED_REDIRECT_URI: "ghostlink://oauth/callback",
  BUNGIE_TOKEN_ENDPOINT: endpoint,
};
const refresh = "grant_type=refresh_token&refresh_token=refresh%2Bvalue";
const code = "grant_type=authorization_code&code=code%2Bvalue&redirect_uri=ghostlink%3A%2F%2Foauth%2Fcallback";
const headers = { "Content-Type": "application/x-www-form-urlencoded" };
function request(body = refresh, extra: Record<string, string> = {}) {
  return new Request("https://broker.example/token", {
    method: "POST", headers: { ...headers, ...extra }, body,
  });
}
function upstream(body = '{"access_token":"upstream-token"}', status = 200) {
  return new Response(body, { status, headers: { "Content-Type": "application/json; charset=utf-8" } });
}
function setup(response = upstream()) {
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(response);
  return { fetch };
}
async function assertError(response: Response, status: number, error: string) {
  expect(response.status).toBe(status);
  expect(await response.text()).toBe(JSON.stringify({ error }));
  assertHeaders(response);
}
function assertHeaders(response: Response) {
  expect(response.headers.get("Content-Type")).toBe("application/json");
  expect(response.headers.get("Cache-Control")).toBe("no-store");
  expect(response.headers.get("Pragma")).toBe("no-cache");
  expect(response.headers.has("Access-Control-Allow-Origin")).toBe(false);
}

describe("routing", () => {
  it.each(["GET", "PUT", "OPTIONS", "DELETE"])("rejects %s on /token", async (method) => {
    const deps = setup();
    const response = await handleRequest(new Request("https://broker.example/token", { method }), env, deps);
    await assertError(response, 405, "invalid_request");
    expect(response.headers.get("Allow")).toBe("POST");
    expect(deps.fetch).not.toHaveBeenCalled();
  });
  it.each(["/", "/refresh", "/token/", "/other"])("rejects path %s", async (path) => {
    await assertError(await handleRequest(new Request(`https://broker.example${path}`), env, setup()), 404, "invalid_request");
  });
});

describe("transparent authenticated grants", () => {
  it.each([code, refresh, "refresh_token=refresh%2Bvalue&grant_type=refresh_token"])("forwards the exact ordered form %s", async (body) => {
    const deps = setup();
    const response = await handleRequest(request(body, { "Content-Type": "application/x-www-form-urlencoded; charset=UTF-8" }), env, deps);
    expect(response.status).toBe(200);
    expect(await response.text()).toBe('{"access_token":"upstream-token"}');
    assertHeaders(response);
    const [url, init] = deps.fetch.mock.calls[0]!;
    expect(url).toBe(endpoint);
    expect(init?.method).toBe("POST");
    expect(init?.body).toBe(body);
    // Literal expected form encoding: space -> +; even colon, slash and UTF-8 are encoded.
    const credentials = "id+%3A%2F%2B%25%26%C3%A9:secret+%3A%2F%2B%25%26%C3%A9";
    const sentHeaders = new Headers(init?.headers);
    expect(sentHeaders.get("Authorization")).toBe(`Basic ${Buffer.from(credentials).toString("base64")}`);
    expect(sentHeaders.get("Content-Type")).toBe(headers["Content-Type"]);
    expect(init?.redirect).toBe("manual");
    expect(init?.signal).toBeInstanceOf(AbortSignal);
    expect(String(init?.body)).not.toContain("client_secret");
  });
  it("does not forward caller authentication or unrelated headers", async () => {
    const deps = setup();
    await handleRequest(request(refresh, { Authorization: "Basic caller-credential", "X-Untrusted": "value" }), env, deps);
    const sent = new Headers(deps.fetch.mock.calls[0]?.[1]?.headers);
    expect(sent.get("Authorization")).not.toBe("Basic caller-credential");
    expect([...sent.keys()].sort()).toEqual(["authorization", "content-type"]);
  });
  it("uses the default endpoint and allows a missing limiter", async () => {
    const { BUNGIE_TOKEN_ENDPOINT: _, ...defaults } = env;
    const deps = setup();
    expect((await handleRequest(request(), defaults, deps)).status).toBe(200);
    expect(deps.fetch.mock.calls[0]?.[0]).toBe(endpoint);
  });
  it("uses an explicitly configured endpoint", async () => {
    const deps = setup();
    await handleRequest(request(), { ...env, BUNGIE_TOKEN_ENDPOINT: "https://upstream.example/token" }, deps);
    expect(deps.fetch.mock.calls[0]?.[0]).toBe("https://upstream.example/token");
  });
});

describe("bounded and strict input", () => {
  it.each(["text/plain", "application/json", "", "application/x-www-form-urlencoded-extra"])("rejects media type %s", async (type) => {
    const deps = setup();
    await assertError(await handleRequest(request(refresh, { "Content-Type": type }), env, deps), 415, "invalid_request");
    expect(deps.fetch).not.toHaveBeenCalled();
  });
  it("checks advertised size even with a small actual body", async () => {
    await assertError(await handleRequest(request(refresh, { "Content-Length": "4097" }), env, setup()), 413, "invalid_request");
  });
  it.each([{}, { "Content-Length": "1" }])("checks actual bytes regardless of advertised size", async (extra) => {
    await assertError(await handleRequest(request("é".repeat(2049), extra), env, setup()), 413, "invalid_request");
  });
  it("accepts exactly 4096 request bytes and 2048 value characters", async () => {
    const body = `grant_type=refresh_token&refresh_token=${"%41".repeat(1002)}%C3%A9${"A".repeat(1045)}`;
    expect(Buffer.byteLength(body)).toBe(4096);
    const deps = setup();
    expect((await handleRequest(request(body), env, deps)).status).toBe(200);
    expect(deps.fetch.mock.calls[0]?.[1]?.body).toBe(`grant_type=refresh_token&refresh_token=${"A".repeat(1002)}%C3%A9${"A".repeat(1045)}`);
  });
  it.each([
    "", "grant_type=refresh_token", "refresh_token=value", "grant_type=&refresh_token=value",
    "grant_type=refresh_token&refresh_token=", "grant_type=authorization_code&code=x",
    `${code}&extra=value`, `${refresh}&client_id=id`, `${refresh}&client_secret=secret`,
    `${refresh}&refresh_token=other`, `${refresh}&grant_type=refresh_token`,
    `${refresh}&code=x`, `${code}&refresh_token=x`,
    "grant_type=refresh_token&refresh_token=%00", "grant_type=refresh_token&refresh_token=%0A",
    "grant_type=refresh_token&refresh_token=%7F", "grant_type=refresh_token&refresh_token=%C2%85",
    `grant_type=refresh_token&refresh_token=${"a".repeat(2049)}`,
    "grant_type=refresh_token&refresh_token=%ZZ", "grant_type=refresh_token&refresh_token=%FF",
    code.replace("ghostlink%3A%2F%2Foauth%2Fcallback", "https%3A%2F%2Fevil.example"),
  ])("rejects invalid form %s", async (body) => {
    const deps = setup();
    await assertError(await handleRequest(request(body), env, deps), 400, "invalid_request");
    expect(deps.fetch).not.toHaveBeenCalled();
  });
  it("rejects unsupported grants", async () => {
    await assertError(await handleRequest(request("grant_type=client_credentials"), env, setup()), 400, "unsupported_grant_type");
  });
});

describe("configuration and rate limiting", () => {
  it.each(["BUNGIE_CLIENT_ID", "BUNGIE_CLIENT_SECRET", "ALLOWED_REDIRECT_URI"] as const)("requires %s", async (key) => {
    for (const value of [undefined, ""]) {
      const configured = { ...env };
      if (value === undefined) delete configured[key];
      else configured[key] = value;
      const deps = setup();
      await assertError(await handleRequest(request(), configured, deps), 500, "server_misconfigured");
      expect(deps.fetch).not.toHaveBeenCalled();
    }
  });
  it.each(["http://upstream.example/token", "not a URL", "https://user:password@upstream.example/token", ""])("rejects unsafe endpoint %s", async (BUNGIE_TOKEN_ENDPOINT) => {
    await assertError(await handleRequest(request(), { ...env, BUNGIE_TOKEN_ENDPOINT }, setup()), 500, "server_misconfigured");
  });
  it.each(["198.51.100.1", undefined])("limits by trusted IP or fallback: %s", async (ip) => {
    const limit = vi.fn().mockResolvedValue({ success: false });
    const deps = setup();
    const req = request(refresh, ip ? { "CF-Connecting-IP": ip } : {});
    await assertError(await handleRequest(req, { ...env, RATE_LIMITER: { limit } }, deps), 429, "rate_limited");
    expect(limit).toHaveBeenCalledWith({ key: ip ?? "unknown" });
    expect(deps.fetch).not.toHaveBeenCalled();
  });
  it("forwards when the limiter permits it", async () => {
    const limit = vi.fn().mockResolvedValue({ success: true });
    expect((await handleRequest(request(), { ...env, RATE_LIMITER: { limit } }, setup())).status).toBe(200);
  });
  it("fails closed without exposing limiter errors", async () => {
    const limit = vi.fn().mockRejectedValue(new Error(env.BUNGIE_CLIENT_SECRET));
    await assertError(await handleRequest(request(), { ...env, RATE_LIMITER: { limit } }, setup()), 502, "bad_gateway");
  });
});

describe("bounded upstream responses", () => {
  it.each([400, 401])("relays upstream %s JSON unchanged but not upstream headers", async (status) => {
    const body = '{"error":"invalid_grant","error_description":"provider message"}';
    const source = upstream(body, status);
    source.headers.set("Set-Cookie", "unwanted=value");
    source.headers.set("Access-Control-Allow-Origin", "*");
    const response = await handleRequest(request(), env, setup(source));
    expect(response.status).toBe(status);
    expect(await response.text()).toBe(body);
    expect(response.headers.has("Set-Cookie")).toBe(false);
    assertHeaders(response);
  });
  it.each(["text/html", "application/jsonp", "", "application/problem+json"])("rejects upstream type %s", async (type) => {
    await assertError(await handleRequest(request(), env, setup(new Response("sensitive upstream body", { headers: { "Content-Type": type } }))), 502, "bad_gateway");
  });
  it.each([{}, { "Content-Length": "1" }])("rejects actual upstream bodies over 8192 bytes", async (extra) => {
    const source = new Response("é".repeat(4097), { headers: { "Content-Type": "application/json", ...extra } });
    await assertError(await handleRequest(request(), env, setup(source)), 502, "bad_gateway");
  });
  it("rejects an oversized advertised upstream body", async () => {
    const source = upstream();
    source.headers.set("Content-Length", "8193");
    await assertError(await handleRequest(request(), env, setup(source)), 502, "bad_gateway");
  });
  it("accepts exactly 8192 upstream bytes", async () => {
    const body = `"${"x".repeat(8190)}"`;
    const response = await handleRequest(request(), env, setup(upstream(body)));
    expect(response.status).toBe(200);
    expect(await response.text()).toBe(body);
  });
  it("maps network failure to a fixed error without secret disclosure", async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockRejectedValue(new Error(env.BUNGIE_CLIENT_SECRET));
    await assertError(await handleRequest(request(), env, { fetch }), 502, "bad_gateway");
  });
  it("times out and aborts an upstream fetch", async () => {
    let signal: AbortSignal | null | undefined;
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async (_, init) => {
      signal = init?.signal;
      return await new Promise<Response>(() => {});
    });
    await assertError(await handleRequest(request(), env, { fetch, timeoutMs: 5 }), 504, "gateway_timeout");
    expect(signal?.aborted).toBe(true);
  });
  it("covers upstream body reads with the same timeout", async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({ cancel() { cancelled = true; } });
    const deps = setup(new Response(body, { headers: { "Content-Type": "application/json" } }));
    await assertError(await handleRequest(request(), env, { ...deps, timeoutMs: 5 }), 504, "gateway_timeout");
    expect(cancelled).toBe(true);
  });
  it("cancels an oversized streamed response without waiting for EOF", async () => {
    let cancelled = false;
    const body = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new Uint8Array(4096));
        controller.enqueue(new Uint8Array(4097));
        // Deliberately never close: the cap must stop the read.
      },
      cancel() { cancelled = true; },
    });
    await assertError(await handleRequest(request(), env, setup(new Response(body, {
      headers: { "Content-Type": "application/json" },
    }))), 502, "bad_gateway");
    expect(cancelled).toBe(true);
  });
  it("redacts failures during upstream body transfer", async () => {
    const body = new ReadableStream<Uint8Array>({
      start(controller) { controller.error(new Error(env.BUNGIE_CLIENT_SECRET)); },
    });
    await assertError(await handleRequest(request(), env, setup(new Response(body, {
      headers: { "Content-Type": "application/json" },
    }))), 502, "bad_gateway");
  });
  it("never logs credentials, forms or upstream bodies", async () => {
    const spies = ["log", "error", "warn", "info", "debug"] as const;
    const mocks = spies.map((method) => vi.spyOn(console, method).mockImplementation(() => {}));
    try {
      await handleRequest(request(code), env, setup());
      await handleRequest(request(), env, { fetch: vi.fn().mockRejectedValue(new Error(env.BUNGIE_CLIENT_SECRET)) });
      for (const mock of mocks) expect(mock).not.toHaveBeenCalled();
    } finally {
      for (const mock of mocks) mock.mockRestore();
    }
  });
  it("does not leak secrets on successful responses", async () => {
    const response = await handleRequest(request(), env, setup());
    expect(await response.text()).not.toContain(env.BUNGIE_CLIENT_SECRET);
    expect(JSON.stringify([...response.headers])).not.toContain(env.BUNGIE_CLIENT_SECRET);
  });
});
