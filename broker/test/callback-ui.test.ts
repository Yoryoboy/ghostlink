import { expect, it, vi } from "vitest";
import { runInNewContext } from "node:vm";
import { readFileSync } from "node:fs";
import { handleRequest } from "../src/index";

const base = "https://ghostlink-token-broker.93jads.workers.dev/callback";
const custom = "ghostlink://oauth/callback";
const denied = "The authorization was not approved. Copy the callback URL below; the CLI will report the denial.";

// S7: execute the actual response script, not just assertions on HTML substrings.
it.each([
  ["?code=CANARY%3Cscript%3E&state=CANARY_STATE&extra=%2B", "Authorization received. Copy the callback URL below and paste it into the CLI."],
  ["?error=access_denied&state=CANARY_STATE", denied],
  ["?error=cancelled&state=CANARY_STATE", denied],
  ["", "No authorization result is present in this callback URL."],
])("runs copy/link/status UI for %s", async (query, message) => {
  const fetch = vi.fn<typeof globalThis.fetch>();
  const response = await handleRequest(new Request(base + query), {}, { fetch });
  expect(response.status).toBe(200);
  const html = await response.text();
  expect(html).not.toContain("CANARY");
  expect(fetch).not.toHaveBeenCalled();
  expect(response.headers.get("Cache-Control")).toBe("no-store");
  expect(response.headers.get("Referrer-Policy")).toBe("no-referrer");
  expect(response.headers.get("X-Frame-Options")).toBe("DENY");
  const csp = response.headers.get("Content-Security-Policy")!;
  expect(csp).toContain("default-src 'none'");
  const nonce = /script-src 'nonce-([^']+)'/.exec(csp)![1]!;
  expect(csp).toContain(`style-src 'nonce-${nonce}'`);
  expect(html.match(new RegExp(`nonce="${nonce}"`, "g"))).toHaveLength(2);
  const script = /<script nonce="[^"]+">([\s\S]*?)<\/script>/.exec(html)![1]!;
  const click: Record<string, () => void> = {};
  const field = { value: "", select: vi.fn(), setSelectionRange: vi.fn() };
  const status = { textContent: "" };
  const open = { href: "" };
  const elements = { "callback-url": field, status, open, copy: {
    addEventListener: (event: string, callback: () => void) => { click[event] = callback; },
  } };
  for (const modern of [true, false]) {
    const writeText = vi.fn().mockResolvedValue(undefined);
    const execCommand = vi.fn();
    runInNewContext(script, {
      window: { location: { href: base + query, search: query } },
      URLSearchParams,
      document: { getElementById: (id: keyof typeof elements) => elements[id], execCommand },
      navigator: modern ? { clipboard: { writeText } } : {},
    });
    expect(status.textContent).toBe(message);
    expect(field.value).toBe(base + query);
    expect(open.href).toBe(custom + query);
    click.click!();
    expect(field.select).toHaveBeenCalled();
    expect(field.setSelectionRange).toHaveBeenCalledWith(0, (base + query).length);
    if (modern) expect(writeText).toHaveBeenCalledWith(base + query);
    else expect(execCommand).toHaveBeenCalledWith("copy");
  }
});

// S8: use the deployed allowlist, and verify exact relay/rejection through handleRequest.
it.each([base, custom, "https://evil.test/callback", base + "/", base.replace("/callback", "/%63allback"), base.replace(".dev/", ".dev:443/"), base.replace("https:", "HTTPS:"), custom.replace("://", "://@")])("enforces exact deployment redirect: %s", async (uri) => {
  const allowed = /ALLOWED_REDIRECT_URIS = "([^"]+)"/.exec(readFileSync("wrangler.toml", "utf8"))![1]!;
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValue(new Response('{"access_token":"FAKE"}', { headers: { "Content-Type": "application/json" } }));
  const form = new URLSearchParams({ grant_type: "authorization_code", code: "FAKE", redirect_uri: uri }).toString();
  const response = await handleRequest(new Request(base.replace("/callback", "/token"), {
    method: "POST", headers: { "Content-Type": "application/x-www-form-urlencoded" }, body: form,
  }), { BUNGIE_CLIENT_ID: "fake", BUNGIE_CLIENT_SECRET: "fake", ALLOWED_REDIRECT_URIS: allowed }, { fetch });
  const accepted = uri === base || uri === custom;
  expect(response.status).toBe(accepted ? 200 : 400);
  expect(await response.text()).toBe(accepted ? '{"access_token":"FAKE"}' : '{"error":"invalid_request"}');
  expect(fetch).toHaveBeenCalledTimes(accepted ? 1 : 0);
  if (accepted) expect(fetch.mock.calls[0]![1]!.body).toBe(form);
});

// S7: exact failure body, allowed methods and independent response nonces.
it("checks method boundaries and fresh nonces", async () => {
  const deps = { fetch: vi.fn<typeof globalThis.fetch>() };
  for (const method of ["POST", "PUT", "PATCH", "DELETE", "OPTIONS"]) {
    const response = await handleRequest(new Request(base, { method }), {}, deps);
    expect(response.status).toBe(405);
    expect(response.headers.get("Allow")).toBe("GET, HEAD");
    expect(await response.text()).toBe('{"error":"invalid_request"}');
  }
  const head = await handleRequest(new Request(base, { method: "HEAD" }), {}, deps);
  expect(head.status).toBe(200);
  expect(await head.text()).toBe("");
  const first = await handleRequest(new Request(base), {}, deps);
  const second = await handleRequest(new Request(base), {}, deps);
  expect(first.headers.get("Content-Security-Policy")).not.toBe(second.headers.get("Content-Security-Policy"));
  expect(deps.fetch).not.toHaveBeenCalled();
});
