import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readFile } from "node:fs/promises";

type Diagnostics = Array<{ event: string; detail: Record<string, unknown> }>;

type RateLimitRuntime = {
  neutralizeCodexRateLimitValue: (value: unknown) => boolean;
  neutralizeCodexRateLimitPayload: (payload: unknown) => unknown;
  rememberCodexRateLimitRequest: (message: unknown) => void;
  patchCodexRateLimitResponseData: (data: unknown) => boolean;
  installCodexRateLimitResponsePatch: () => void;
  codexRateLimitUnlockRequestMethod: (value: string) => string;
  patchCodexUsageHttpTransport: (module: unknown) => boolean;
  refreshCodexUsageLimitQueryCache: () => void;
};

async function rateLimitUnlockRuntime(quotaResume = true, documentValue: unknown = {}) {
  const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
  const start = renderer.indexOf("  const codexRateLimitUnlockMaxTrackedRequests = ");
  const end = renderer.indexOf("\n  // Recent Codex builds send app-server requests", start);
  assert.ok(start >= 0 && end > start, "rate limit unlock block not found");
  const source = renderer.slice(start, end);
  const diagnostics: Diagnostics = [];
  const listeners: Array<(event: { data: unknown }) => void> = [];
  const sent: unknown[] = [];
  const bridge = {
    sendMessageFromView(message: unknown) {
      sent.push(message);
      return message;
    },
  };
  const windowValue: Record<string, unknown> = {
    electronBridge: bridge,
    addEventListener(type: string, handler: (event: { data: unknown }) => void) {
      if (type === "message") listeners.push(handler);
    },
  };
  const create = new Function(
    "window",
    "codexPlusSettings",
    "appServerModelRequestMethod",
    "sendCodexPlusDiagnostic",
    "codexRateLimitUnlockVersion",
    "document",
    "Headers",
    "Response",
    "structuredClone",
    `${source}\nreturn {
      neutralizeCodexRateLimitValue,
      neutralizeCodexRateLimitPayload,
      rememberCodexRateLimitRequest,
      patchCodexRateLimitResponseData,
      installCodexRateLimitResponsePatch,
      codexRateLimitUnlockRequestMethod,
      patchCodexUsageHttpTransport,
      refreshCodexUsageLimitQueryCache,
    };`,
  ) as (
    windowValue: Record<string, unknown>,
    settings: () => { quotaResume: boolean },
    requestMethod: (value: string) => string,
    diagnostic: (event: string, detail: Record<string, unknown>) => void,
    version: string,
    documentValue: unknown,
    headers: typeof Headers,
    response: typeof Response,
    clone: typeof structuredClone,
  ) => RateLimitRuntime;
  const runtime = create(
    windowValue,
    () => ({ quotaResume }),
    (value: string) => {
      const text = String(value || "");
      return text.startsWith("vscode://codex/") ? text.slice("vscode://codex/".length) : text;
    },
    (event, detail) => diagnostics.push({ event, detail }),
    "1",
    documentValue,
    Headers,
    Response,
    structuredClone,
  );
  return { runtime, diagnostics, listeners, sent, windowValue, bridge };
}

function limitedRateLimitPayload() {
  return {
    rate_limits: {
      allowed: false,
      limit_reached: true,
      rate_limit_reached_type: { type: "rate_limit_reached" },
      spend_control: { reached: true },
      plan_type: "plus",
      primary: { usedPercent: 100, windowMinutes: 10080, resetsAt: 1790420353 },
      credits: { has_credits: false, unlimited: false, overage_limit_reached: true },
      additional_rate_limits: [
        { limit_name: "gpt-6-astra", rate_limit: { allowed: false, limit_reached: true }, blocked: true },
      ],
    },
  };
}

describe("official rate limit unlock patch", () => {
  it("clears the blocking flags the desktop uses to lock the composer", async () => {
    const { runtime } = await rateLimitUnlockRuntime(true);
    const payload = limitedRateLimitPayload();

    assert.equal(runtime.neutralizeCodexRateLimitValue(payload), true);
    const limits = payload.rate_limits;
    assert.equal(limits.rate_limit_reached_type, null);
    assert.equal(limits.allowed, true);
    assert.equal(limits.limit_reached, false);
    assert.equal(limits.spend_control.reached, false);
    assert.equal(limits.credits.overage_limit_reached, false);
    // has_credits=false 且 unlimited=false 会被桌面端当成“额度耗尽”兜底条件。
    assert.equal(limits.credits.has_credits, true);
    assert.equal(limits.credits.unlimited, false);
    assert.equal(limits.additional_rate_limits[0].rate_limit.allowed, true);
    assert.equal(limits.additional_rate_limits[0].rate_limit.limit_reached, false);
    assert.equal(limits.additional_rate_limits[0].blocked, false);
    // 真实用量必须保留，否则侧栏用量显示会失真。
    assert.equal(limits.primary.usedPercent, 100);
  });

  it("is idempotent so repeated scans stop reporting changes", async () => {
    const { runtime } = await rateLimitUnlockRuntime(true);
    const payload = limitedRateLimitPayload();

    assert.equal(runtime.neutralizeCodexRateLimitValue(payload), true);
    assert.equal(runtime.neutralizeCodexRateLimitValue(payload), false);
  });

  it("stays inert when the quota resume setting is off", async () => {
    const { runtime } = await rateLimitUnlockRuntime(false);
    const payload = limitedRateLimitPayload();

    runtime.neutralizeCodexRateLimitPayload(payload);

    assert.equal(payload.rate_limits.limit_reached, true);
    assert.equal(payload.rate_limits.rate_limit_reached_type?.type, "rate_limit_reached");
  });

  it("rewrites the tracked fetch response body and ignores unrelated responses", async () => {
    const { runtime } = await rateLimitUnlockRuntime(true);

    runtime.rememberCodexRateLimitRequest({
      type: "fetch",
      requestId: "rl-1",
      method: "POST",
      url: "vscode://codex/account/rateLimits/read",
    });
    runtime.rememberCodexRateLimitRequest({ type: "fetch", requestId: "other-1", url: "vscode://codex/thread/list" });

    const unrelated = { type: "fetch-response", requestId: "other-1", bodyJsonString: JSON.stringify(limitedRateLimitPayload()) };
    assert.equal(runtime.patchCodexRateLimitResponseData(unrelated), false);

    const tracked = { type: "fetch-response", requestId: "rl-1", bodyJsonString: JSON.stringify(limitedRateLimitPayload()) };
    assert.equal(runtime.patchCodexRateLimitResponseData(tracked), true);
    const parsed = JSON.parse(tracked.bodyJsonString);
    assert.equal(parsed.rate_limits.limit_reached, false);
    assert.equal(parsed.rate_limits.rate_limit_reached_type, null);
    // 同一 requestId 只处理一次。
    assert.equal(runtime.patchCodexRateLimitResponseData(tracked), false);
  });

  it("installs a bridge wrapper and message listener that neutralizes rate limit replies", async () => {
    const { runtime, listeners, windowValue, bridge } = await rateLimitUnlockRuntime(true);

    runtime.installCodexRateLimitResponsePatch();
    assert.equal(listeners.length, 1);
    assert.equal(windowValue.__codexPlusRateLimitResponsePatch, "1");
    assert.notEqual(bridge.sendMessageFromView, undefined);

    bridge.sendMessageFromView({ type: "fetch", requestId: "rl-2", url: "vscode://codex/account/rateLimits/read" });
    const data = { type: "fetch-response", requestId: "rl-2", bodyJsonString: JSON.stringify(limitedRateLimitPayload()) };
    listeners[0]({ data });

    assert.equal(JSON.parse(data.bodyJsonString).rate_limits.allowed, true);
  });

  it("only treats official rate limit reads as unlock targets", async () => {
    const { runtime } = await rateLimitUnlockRuntime(true);

    assert.equal(runtime.codexRateLimitUnlockRequestMethod("vscode://codex/account/rateLimits/read"), "account/rateLimits/read");
    assert.equal(runtime.codexRateLimitUnlockRequestMethod("account/usage/read"), "account/usage/read");
    assert.equal(runtime.codexRateLimitUnlockRequestMethod("vscode://codex/turn/start"), "");
  });

  it("rewrites only the real usage HTTP response and usage snapshot stream", async () => {
    const { runtime, diagnostics } = await rateLimitUnlockRuntime(true);
    const calls: Array<{ method: string; url: string; options: Record<string, unknown> }> = [];
    const transport = {
      streamControllers: new Map(),
      async fetch(url: string) {
        return new Response(JSON.stringify(limitedRateLimitPayload()), {
          status: 200,
          headers: { "content-type": "application/json", "content-length": "999" },
        });
      },
      stream(method: string, url: string, options: Record<string, unknown>) {
        const upstreamSignature = "fetch-stream-event streamControllers";
        void upstreamSignature;
        calls.push({ method, url, options });
        return "stream-1";
      },
    };
    class UsageTransport {
      static getInstance() { return transport; }
    }
    assert.match(Function.prototype.toString.call(transport.stream), /fetch-stream-event/);
    assert.match(Function.prototype.toString.call(transport.stream), /streamControllers/);
    assert.equal(runtime.patchCodexUsageHttpTransport({ UsageTransport }), true);
    assert.equal(runtime.patchCodexUsageHttpTransport({ UsageTransport }), true);
    const usage = await (await transport.fetch("/wham/usage")).json();
    assert.equal(usage.rate_limits.allowed, true);
    assert.equal(usage.rate_limits.primary.usedPercent, 100);
    const unrelated = await (await transport.fetch("/wham/usage/credits/estimate")).json();
    assert.equal(unrelated.rate_limits.allowed, false);
    const events: unknown[] = [];
    transport.stream("GET", "/wham/usage/stream", { onEvent: (event: unknown) => events.push(event) });
    const snapshot = { event: "usage.snapshot", data: { usage: limitedRateLimitPayload() } };
    (calls[0].options.onEvent as (event: unknown) => void)(snapshot);
    assert.equal((events[0] as typeof snapshot).data.usage.rate_limits.limit_reached, false);
    assert.ok(diagnostics.some((item) => item.event === "usage_limit_http_payload_neutralized"));
    assert.ok(diagnostics.some((item) => item.event === "usage_limit_stream_payload_neutralized"));
  });

  it("clears an exhausted usage snapshot already held by React Query", async () => {
    const query = { queryKey: ["rate-limit-status", "user", "account"], state: { data: limitedRateLimitPayload() } };
    const unrelated = { queryKey: ["other"], state: { data: limitedRateLimitPayload() } };
    const writes: Array<{ key: string[]; data: typeof query.state.data }> = [];
    const client = {
      getQueryCache: () => ({ findAll: () => [query, unrelated] }),
      setQueryData(key: string[], data: typeof query.state.data) {
        writes.push({ key, data });
        query.state.data = data;
      },
    };
    const fiber = { memoizedProps: { client } };
    const root = { "__reactContainer$test": { current: fiber } };
    const documentValue = {
      getElementById: () => root,
      body: null,
      documentElement: null,
    };
    const { runtime } = await rateLimitUnlockRuntime(true, documentValue);
    runtime.refreshCodexUsageLimitQueryCache();
    runtime.refreshCodexUsageLimitQueryCache();
    assert.equal(writes.length, 1);
    assert.equal(query.state.data.rate_limits.allowed, true);
    assert.equal(query.state.data.rate_limits.primary.usedPercent, 100);
    assert.equal(unrelated.state.data.rate_limits.allowed, false);
  });
});
