import type { RateLimit } from "@cloudflare/workers-types";

export interface Env {
  BUNGIE_CLIENT_ID?: string;
  BUNGIE_CLIENT_SECRET?: string;
  ALLOWED_REDIRECT_URI?: string;
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

class BodyTooLarge extends Error {}

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
  if (new URL(request.url).pathname !== "/token") return error(404);
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
  if (!clientId || !secret || !env.ALLOWED_REDIRECT_URI) return error(500, "server_misconfigured");
  const endpoint = env.BUNGIE_TOKEN_ENDPOINT ?? DEFAULT_ENDPOINT;
  try {
    const url = new URL(endpoint);
    // Only owner-controlled HTTPS endpoints are valid; never follow redirects
    // while carrying client authentication or token material.
    if (url.protocol !== "https:" || url.username || url.password || url.hash) throw new Error();
  } catch {
    return error(500, "server_misconfigured");
  }
  if (grant === "authorization_code" && fields.get("redirect_uri") !== env.ALLOWED_REDIRECT_URI) {
    return error(400);
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
