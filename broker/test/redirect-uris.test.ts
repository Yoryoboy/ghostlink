import { describe, expect, it, vi } from "vitest";
import { handleRequest, type Env } from "../src/index";

const endpoint = "https://www.bungie.net/platform/app/oauth/token/";
const httpsCallback = "https://ghostlink-token-broker.93jads.workers.dev/callback";
const customCallback = "ghostlink://oauth/callback";

function configured(redirectUris?: string): Env {
  const env: Env = {
    BUNGIE_CLIENT_ID: "id",
    BUNGIE_CLIENT_SECRET: "secret",
    BUNGIE_TOKEN_ENDPOINT: endpoint,
  };
  if (redirectUris !== undefined) env.ALLOWED_REDIRECT_URIS = redirectUris;
  return env;
}

function form(uri: string): string {
  return `grant_type=authorization_code&code=code%2Bvalue&redirect_uri=${encodeURIComponent(uri)}`;
}

function request(body: string) {
  return new Request("https://broker.example/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body,
  });
}

function setup() {
  return {
    fetch: vi.fn<typeof globalThis.fetch>().mockResolvedValue(
      new Response('{"access_token":"upstream-token"}', {
        headers: { "Content-Type": "application/json; charset=utf-8" },
      }),
    ),
  };
}

describe("ALLOWED_REDIRECT_URIS", () => {
  it.each([httpsCallback, customCallback])("accepts registered callback %s", async (uri) => {
    const deps = setup();
    const response = await handleRequest(
      request(form(uri)),
      configured(`${httpsCallback},${customCallback}`),
      deps,
    );
    expect(response.status).toBe(200);
    expect(deps.fetch).toHaveBeenCalledOnce();
  });

  it("rejects an unregistered callback", async () => {
    const deps = setup();
    const response = await handleRequest(
      request(form("https://evil.example/callback")),
      configured(`${httpsCallback},${customCallback}`),
      deps,
    );
    expect(response.status).toBe(400);
    expect(await response.text()).toBe(JSON.stringify({ error: "invalid_request" }));
    expect(deps.fetch).not.toHaveBeenCalled();
  });

  it("trims whitespace and ignores empty entries", async () => {
    const deps = setup();
    const response = await handleRequest(
      request(form(httpsCallback)),
      configured(` ${httpsCallback} , ,${customCallback}, `),
      deps,
    );
    expect(response.status).toBe(200);
    expect(deps.fetch).toHaveBeenCalledOnce();
  });

  it.each([undefined, "", " ", ",", " , "])("fails closed when the list is missing or empty (%s)", async (value) => {
    const deps = setup();
    const response = await handleRequest(request(form(httpsCallback)), configured(value), deps);
    expect(response.status).toBe(500);
    expect(await response.text()).toBe(JSON.stringify({ error: "server_misconfigured" }));
    expect(deps.fetch).not.toHaveBeenCalled();
  });

  it("still allows refresh grants without a redirect_uri", async () => {
    const deps = setup();
    const response = await handleRequest(
      request("grant_type=refresh_token&refresh_token=refresh%2Bvalue"),
      configured(customCallback),
      deps,
    );
    expect(response.status).toBe(200);
    expect(deps.fetch).toHaveBeenCalledOnce();
  });
});
