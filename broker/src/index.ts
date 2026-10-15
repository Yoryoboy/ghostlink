import type { RateLimit } from "@cloudflare/workers-types";

export interface Env {
  BUNGIE_CLIENT_ID?: string;
  BUNGIE_CLIENT_SECRET?: string;
  /** Comma-separated exact redirect URIs accepted for authorization_code grants. */
  ALLOWED_REDIRECT_URIS?: string;
  BUNGIE_TOKEN_ENDPOINT?: string;
  RATE_LIMITER?: RateLimit;
}

export interface Dependencies {
  fetch: typeof globalThis.fetch;
  /** Tests can shorten the total upstream deadline; production uses 10 seconds. */
  timeoutMs?: number;
}

const DEFAULT_ENDPOINT = "https://www.bungie.net/platform/app/oauth/token/";
const FORM = "application/x-www-form-urlencoded";
const JSON_TYPE = "application/json";
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/u;
const RESPONSE_HEADERS = {
  "Content-Type": JSON_TYPE,
  "Cache-Control": "no-store",
  Pragma: "no-cache",
};
const CALLBACK_PATH = "/callback";
const GHOSTLINK_CALLBACK = "ghostlink://oauth/callback";

class BodyTooLarge extends Error {}

function createNonce(): string {
  const bytes = new Uint8Array(16);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

// The page is a static template: request code/state are read by the browser
// from location.href and never reach the server-side HTML or logs.
function callbackPage(nonce: string): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ghostlink authorization</title>
<style nonce="${nonce}">
:root { color-scheme: light dark; }
body { font-family: system-ui, sans-serif; margin: 0; padding: 1.5rem; line-height: 1.5; }
main { max-width: 42rem; margin: 0 auto; }
h1 { font-size: 1.4rem; }
label { display: block; margin: 1rem 0 .25rem; font-weight: 600; }
input { width: 100%; box-sizing: border-box; padding: .6rem; font: inherit; }
button, a.button { display: inline-block; margin-top: .75rem; padding: .6rem .9rem; font: inherit; }
#status { font-weight: 600; }
code { word-break: break-all; }
</style>
</head>
<body>
<main>
<h1>Ghostlink authorization</h1>
<p id="status" role="status"></p>
<label for="callback-url">Callback URL</label>
<input id="callback-url" type="text" readonly autocomplete="off" spellcheck="false">
<button id="copy" type="button">Copy</button>
<p>Paste this URL into <code>ghostlink login complete</code> on the machine where <code>ghostlink login start</code> ran.</p>
<a id="open" class="button" href="${GHOSTLINK_CALLBACK}">Open in Ghostlink on this computer</a>
</main>
<script nonce="${nonce}">
(function () {
  "use strict";
  var status = document.getElementById("status");
  var field = document.getElementById("callback-url");
  var search = window.location.search;
  var params = new URLSearchParams(search);
  if (params.has("error")) {
    status.textContent = "The authorization was not approved. Copy the callback URL below; the CLI will report the denial.";
  } else if (params.has("code")) {
    status.textContent = "Authorization received. Copy the callback URL below and paste it into the CLI.";
  } else {
    status.textContent = "No authorization result is present in this callback URL.";
  }
  field.value = window.location.href;
  document.getElementById("open").href = "${GHOSTLINK_CALLBACK}" + search;
  document.getElementById("copy").addEventListener("click", function () {
    field.select();
    field.setSelectionRange(0, field.value.length);
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(field.value);
    } else {
      document.execCommand("copy");
    }
  });
})();
</script>
</body>
</html>
`;
}

// A static, credential-free page needs no budget from the token-exchange
// limiter; it must keep working even while exchanges are being throttled.
function callbackResponse(request: Request): Response {
  const nonce = createNonce();
  const headers = {
    "Content-Type": "text/html; charset=utf-8",
    "Cache-Control": "no-store",
    Pragma: "no-cache",
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
    "X-Frame-Options": "DENY",
    "Content-Security-Policy": `default-src 'none'; script-src 'nonce-${nonce}'; style-src 'nonce-${nonce}'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'`,
  };
  return new Response(request.method === "HEAD" ? null : callbackPage(nonce), { headers });
}

function allowedRedirectUris(env: Env): string[] {
  return (env.ALLOWED_REDIRECT_URIS ?? "")
    .split(",")
    .map((uri) => uri.trim())
    .filter((uri) => uri.length > 0);
}

function error(status: number, name = "invalid_request", extra: Record<string, string> = {}): Response {
  return new Response(JSON.stringify({ error: name }), {
    status, headers: { ...RESPONSE_HEADERS, ...extra },
  });
}

function hasMediaType(headers: Headers, expected: string): boolean {
  return headers.get("Content-Type")?.split(";", 1)[0]?.trim().toLowerCase() === expected;
}

// Never buffer an unbounded body, even when Content-Length is absent or false.
async function readBounded(
  message: Request | Response,
  maximum: number,
  signal?: AbortSignal,
): Promise<Uint8Array<ArrayBuffer>> {
  const length = message.headers.get("Content-Length");
  if (length !== null && (!/^\d+$/.test(length) || Number(length) > maximum)) {
    void message.body?.cancel().catch(() => {});
    throw new BodyTooLarge();
  }
  if (!message.body) return new Uint8Array();
  const reader = message.body.getReader();
  const cancel = () => { void reader.cancel().catch(() => {}); };
  signal?.addEventListener("abort", cancel, { once: true });
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    if (signal?.aborted) throw new Error();
    while (true) {
      const { done, value } = await reader.read();
      if (signal?.aborted) throw new Error();
      if (done) break;
      size += value.byteLength;
      if (size > maximum) throw new BodyTooLarge();
      chunks.push(value);
    }
    const bytes = new Uint8Array(size);
    let offset = 0;
    for (const chunk of chunks) {
      bytes.set(chunk, offset);
      offset += chunk.byteLength;
    }
    return bytes;
  } catch (failure) {
    cancel();
    throw failure;
  } finally {
    signal?.removeEventListener("abort", cancel);
    reader.releaseLock();
  }
}

function parseForm(bytes: Uint8Array): URLSearchParams | undefined {
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    const fields = new URLSearchParams();
    const seen = new Set<string>();
    // URLSearchParams alone tolerates broken percent escapes and UTF-8. Decode
    // strictly first so malformed input cannot silently change token material.
    for (const pair of text.split("&")) {
      const separator = pair.indexOf("=");
      if (separator < 1) return undefined;
      const decode = (value: string) => decodeURIComponent(value.replace(/\+/g, " "));
      const key = decode(pair.slice(0, separator));
      const value = decode(pair.slice(separator + 1));
      if (seen.has(key) || !value || value.length > 2048 || CONTROL.test(value)) return undefined;
      seen.add(key);
      fields.append(key, value);
    }
    return fields;
  } catch {
    return undefined;
  }
}

// URLSearchParams matches Rust's form_urlencoded::byte_serialize, including
// spaces as '+', encoded ':' and UTF-8 bytes. btoa sees ASCII, not raw Unicode.
function formEncode(value: string): string {
  return new URLSearchParams([["", value]]).toString().slice(1);
}

export async function handleRequest(request: Request, env: Env, deps: Dependencies): Promise<Response> {
  const { pathname } = new URL(request.url);
  if (pathname === CALLBACK_PATH) {
    if (request.method !== "GET" && request.method !== "HEAD") {
      return error(405, "invalid_request", { Allow: "GET, HEAD" });
    }
    return callbackResponse(request);
  }
  if (pathname !== "/token") return error(404);
  if (request.method !== "POST") return error(405, "invalid_request", { Allow: "POST" });

  if (env.RATE_LIMITER) {
    try {
      const result = await env.RATE_LIMITER.limit({
        key: request.headers.get("CF-Connecting-IP") || "unknown",
      });
      if (!result.success) return error(429, "rate_limited");
    } catch {
      // Binding outages must not turn into unlimited token exchanges.
      return error(502, "bad_gateway");
    }
  }

  if (!hasMediaType(request.headers, FORM)) return error(415);
  let bytes: Uint8Array;
  try {
    bytes = await readBounded(request, 4096);
  } catch (failure) {
    return error(failure instanceof BodyTooLarge ? 413 : 400);
  }
  const fields = parseForm(bytes);
  if (!fields) return error(400);
  const grant = fields.get("grant_type");
  if (!grant) return error(400);
  let expected: string[];
  if (grant === "authorization_code") expected = ["grant_type", "code", "redirect_uri"];
  else if (grant === "refresh_token") expected = ["grant_type", "refresh_token"];
  else return error(400, "unsupported_grant_type");
  if (fields.size !== expected.length || expected.some((key) => !fields.has(key))) return error(400);

  const clientId = env.BUNGIE_CLIENT_ID;
  const secret = env.BUNGIE_CLIENT_SECRET;
  const redirectUris = allowedRedirectUris(env);
  if (!clientId || !secret || redirectUris.length === 0) return error(500, "server_misconfigured");
  const endpoint = env.BUNGIE_TOKEN_ENDPOINT ?? DEFAULT_ENDPOINT;
  try {
    const url = new URL(endpoint);
    // Only owner-controlled HTTPS endpoints are valid; never follow redirects
    // while carrying client authentication or token material.
    if (url.protocol !== "https:" || url.username || url.password || url.hash) throw new Error();
  } catch {
    return error(500, "server_misconfigured");
  }
  if (grant === "authorization_code") {
    const redirect = fields.get("redirect_uri");
    if (redirect === null || !redirectUris.includes(redirect)) return error(400);
  }

  const controller = new AbortController();
  let timedOut = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const deadline = new Promise<Response>((resolve) => {
    timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
      resolve(error(504, "gateway_timeout"));
    }, deps.timeoutMs ?? 10_000);
  });
  const exchange = async (): Promise<Response> => {
    try {
      const upstream = await deps.fetch(endpoint, {
        method: "POST",
        headers: {
          "Content-Type": FORM,
          Authorization: `Basic ${btoa(`${formEncode(clientId)}:${formEncode(secret)}`)}`,
        },
        body: fields.toString(),
        redirect: "manual",
        signal: controller.signal,
      });
      if (controller.signal.aborted || !hasMediaType(upstream.headers, JSON_TYPE)) {
        void upstream.body?.cancel().catch(() => {});
        return error(timedOut ? 504 : 502, timedOut ? "gateway_timeout" : "bad_gateway");
      }
      const body = await readBounded(upstream, 8192, controller.signal);
      // Keep bytes and status unchanged, but never relay provider headers.
      // Null-body statuses cannot carry a body in the Fetch API.
      return new Response([204, 205, 304].includes(upstream.status) ? null : body, {
        status: upstream.status, headers: RESPONSE_HEADERS,
      });
    } catch {
      // Exception messages may contain credentials or provider bodies. Neither
      // return nor log them; all broker errors are fixed OAuth-style JSON.
      return error(timedOut ? 504 : 502, timedOut ? "gateway_timeout" : "bad_gateway");
    }
  };
  try {
    return await Promise.race([exchange(), deadline]);
  } finally {
    clearTimeout(timer);
  }
}

export default {
  fetch(request: Request, env: Env): Promise<Response> {
    return handleRequest(request, env, { fetch: globalThis.fetch.bind(globalThis) });
  },
};
