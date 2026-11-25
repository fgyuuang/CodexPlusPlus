import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import type { GatewayPayload, GatewayPoolEntry, QuotaObservation, UsageCounts, UsageRouteDay, UsageSummary } from "../src/gateway-model";
import type { FlowEntry } from "../src/request-flow-model";

const now = Date.now();
const hour = 3_600_000;
const history: Record<string, QuotaObservation[]> = {
  pro: [
    ...[5, 14, 19, 26].map((usedPercent, index) => ({
      windowMinutes: 300, usedPercent, observedAtMs: now - (13 - index) * hour, resetsAtMs: now - 9 * hour,
    })),
    ...[8, 20, 53, 72].map((usedPercent, index) => ({
      windowMinutes: 300, usedPercent, observedAtMs: now - (7 - index * 1.45) * hour, resetsAtMs: now - 2.5 * hour,
    })),
    ...[12, 22, 31, 43, 60].map((usedPercent, index) => ({
      windowMinutes: 300, usedPercent, observedAtMs: now - (4 - index) * .24 * hour, resetsAtMs: now + 2.5 * hour,
    })),
    ...[8, 12, 17, 20, 23, 26].map((usedPercent, index) => ({
      windowMinutes: 10080, usedPercent, observedAtMs: now - (5 - index) * 6 * hour, resetsAtMs: now + 105 * hour,
    })),
  ],
  plus: [{ windowMinutes: 10080, usedPercent: 9, observedAtMs: now, resetsAtMs: now + 139 * hour }],
  flex: [
    ...[15, 27, 41].map((usedPercent, index) => ({
      windowMinutes: 180, usedPercent, observedAtMs: now - (2 - index) * .3 * hour, resetsAtMs: now + 1.5 * hour,
    })),
    ...[7, 13, 19].map((usedPercent, index) => ({
      windowMinutes: 2880, usedPercent, observedAtMs: now - (2 - index) * 4 * hour, resetsAtMs: now + 20 * hour,
    })),
  ],
};

const account = (id: string, name: string, plan: string): GatewayPoolEntry => ({
  id, name, plan, account: "chatgpt", official: true, keyCount: 0, baseUrl: "https://chatgpt.com",
  protocol: "responses", models: ["gpt-5.6-luna"], quota: {
    updatedAtMs: now,
    windows: history[id].filter(point => point.observedAtMs === now),
  },
});
const pool = [{
  ...account("pro", "Codex · 主账号", "Pro"),
  resetCredits: { observedAtMs: now, credits: [{ id: "preview-reset", status: "available", usable: true, expiresAtMs: now + hour }] },
}, account("plus", "Codex · 备用账号", "Plus"), {
  ...account("flex", "自定义额度示例", "示例套餐"), account: undefined, quotaApi: true, official: false,
  baseUrl: "https://example.invalid", models: [],
}];
const payload: GatewayPayload = {
  gateway: { version: 4, bindings: {}, restoreOnExit: true, keyPolicy: { mode: "roundRobin" } },
  port: 0, helperRunning: false, pool, groups: [{ id: "review", name: "代码审查", mode: "inOrder", members: [] }], agents: [], health: { keys: [], providers: [] },
  activity: { agents: [
    { agent: "codex", requests: 4, rerouted: 1, errors: 0, inFlight: [] },
    { agent: "claude", requests: 1, rerouted: 0, errors: 1, inFlight: [] },
  ] },
  codexProviders: [], claudeProviders: [], grokProfiles: [],
};
const routes = [
  ["codex", "pro", pool[0].name, "gpt-5.6-luna"],
  ["codex", "local-official-preview", "当前登录账号", "gpt-6-sol"],
  ["claude-code", "relay-a", "Cloud Relay", "claude-sonnet"],
];
const zero: UsageCounts = { calls: 0, errors: 0, inputTokens: 0, outputTokens: 0, cachedTokens: 0, reasoningTokens: 0, tokenMeteredCalls: 0, timedCalls: 0, durationMs: 0 };
const add = (left: UsageCounts, right: UsageCounts): UsageCounts => Object.fromEntries(
  Object.keys(zero).map(key => [key, (left[key as keyof UsageCounts] ?? 0) + (right[key as keyof UsageCounts] ?? 0)]),
) as UsageCounts;
const dateAt = (ago: number) => {
  const date = new Date(now);
  date.setDate(date.getDate() - ago);
  return date.toLocaleDateString("sv-SE");
};

function usage(range: string): UsageSummary {
  const length = range === "today" ? 1 : range === "7d" ? 7 : range === "all" ? 65 : 30;
  const dates = Array.from({ length }, (_, i) => dateAt(length - 1 - i));
  const dailyRoutes: UsageRouteDay[] = dates.flatMap((date, day) => routes.flatMap(([agent, providerId, provider, model], index) => {
    const calls = (day * (index + 5) + 9) % (index + 13);
    return calls ? [{ ...zero, date, agent, providerId, provider, model, calls, tokenMeteredCalls: calls,
      timedCalls: calls, durationMs: calls * 1350, inputTokens: calls * 7400, outputTokens: calls * 820, cachedTokens: calls * 5600 }] : [];
  }));
  const group = (key: "agent" | "providerId" | "model") => [...new Set(dailyRoutes.map(row => row[key]))].map(value => {
    const rows = dailyRoutes.filter(row => row[key] === value);
    return { ...rows.reduce(add, zero), key: value, label: key === "providerId" ? rows[0].provider : value };
  });
  const todayHours = new Map<number, UsageCounts>();
  const lastHour = new Date(now).getHours();
  if (range === "today") for (const row of dailyRoutes) {
    for (let call = 0; call < row.calls; call++) {
      const time = Math.max(0, lastHour - (call % 7));
      todayHours.set(time, add(todayHours.get(time) ?? zero, {
        ...zero, calls: 1, tokenMeteredCalls: 1, timedCalls: 1, durationMs: 1350,
        inputTokens: 7400, outputTokens: 820, cachedTokens: 5600,
      }));
    }
  }
  return {
    totals: dailyRoutes.reduce(add, zero), days: dates.map(date => ({ ...dailyRoutes.filter(row => row.date === date).reduce(add, zero), date })),
    byAgent: group("agent"), byProvider: group("providerId"), byModel: group("model"), costs: [], dailyRoutes,
    hours: range === "today" ? [...todayHours].map(([hour, counts]) => ({ hour, ...counts })) : undefined,
  };
}

const routingEntries: FlowEntry[] = [
  {
    id: "preview-routed", agent: "codex", timestampMs: now,
    path: "/v1/responses", threadId: "preview-session-01", threadTitle: "配额日历与路由验收",
    startedAtMs: now - 1180, durationMs: 1180, firstTokenMs: 1072, stream: true, success: true, statusCode: 200,
    model: "group/review", upstreamModel: "gpt-5.6-luna", relayName: pool[0].name, relayId: "pro",
    usage: { inputTokens: 3570, outputTokens: 610, cachedTokens: 3332, cachedTokensKnown: true },
    contextSummary: {
      parts: [
        { kind: "instructions", items: 1, estimatedTokens: 490 }, { kind: "tools", items: 5, estimatedTokens: 960 },
        { kind: "user", items: 2, estimatedTokens: 430 }, { kind: "assistant", items: 1, estimatedTokens: 280 },
        { kind: "toolCalls", items: 1, estimatedTokens: 150 }, { kind: "toolResults", items: 1, estimatedTokens: 1160 },
      ],
      turns: [
        { number: 1, parts: [{ kind: "user", items: 1, estimatedTokens: 192 }, { kind: "assistant", items: 1, estimatedTokens: 280 },
          { kind: "toolCalls", items: 1, estimatedTokens: 150 }, { kind: "toolResults", items: 1, estimatedTokens: 1160 }] },
        { number: 2, parts: [{ kind: "user", items: 1, estimatedTokens: 238 }] },
      ],
      omittedTurns: 0, partial: false,
    },
    route: {
      requestedModel: "group/review", model: "gpt-5.6-luna", groupId: "review", result: "answered",
      contextWindow: 272000, elapsedMs: 530, finishedAtMs: now - 645,
      planned: [
        { relayId: "relay-a", relayName: "Cloud Relay", model: "gpt-6-sol" },
        { relayId: "pro", relayName: pool[0].name, model: "gpt-5.6-luna" },
      ],
      steps: [
        { relayId: "relay-a", relayName: "Cloud Relay", model: "gpt-6-sol", outcome: "failed", status: 429, failure: "rate-limit", elapsedMs: 98, attemptMs: 96 },
        { relayId: "pro", relayName: pool[0].name, model: "gpt-5.6-luna", outcome: "answered", status: 200, elapsedMs: 524, attemptMs: 422 },
      ],
    },
  },
  {
    id: "preview-routed-earlier", agent: "codex", timestampMs: now - 96_000,
    startedAtMs: now - 98_210, durationMs: 2210, firstTokenMs: 410, stream: true, success: true, statusCode: 200,
    path: "/v1/responses", threadId: "preview-session-01", threadTitle: "配额日历与路由验收",
    model: "group/review", upstreamModel: "gpt-5.6-luna", relayName: pool[0].name, relayId: "pro",
    usage: { inputTokens: 2440, outputTokens: 312, cachedTokens: 0, cachedTokensKnown: true },
    route: { requestedModel: "group/review", model: "gpt-5.6-luna", result: "answered", contextWindow: 272000,
      elapsedMs: 387, finishedAtMs: now - 98_000, planned: [], steps: [] },
  },
  {
    id: "preview-compaction", agent: "codex", timestampMs: now - 26_000,
    startedAtMs: now - 27_210, durationMs: 1210, stream: false, success: true, statusCode: 200,
    path: "/v1/responses/compact", threadId: "preview-session-01", threadTitle: "配额日历与路由验收",
    model: "group/review", upstreamModel: "gpt-5.6-luna", relayName: pool[0].name, relayId: "pro",
    route: { requestedModel: "group/review", model: "gpt-5.6-luna", result: "answered", elapsedMs: 1210,
      finishedAtMs: now - 26_000, planned: [], steps: [] },
  },
  {
    id: "preview-second-thread", agent: "codex", timestampMs: now - 10_000,
    startedAtMs: now - 11_610, durationMs: 1610, stream: true, success: true, statusCode: 200,
    path: "/v1/responses", threadId: "preview-session-02", threadTitle: "网关模型选择",
    model: "gpt-6-sol", upstreamModel: "gpt-6-sol", relayName: "Cloud Relay", relayId: "relay-a",
    usage: { inputTokens: 48120, outputTokens: 810, cachedTokens: 0, cachedTokensKnown: false },
    route: { requestedModel: "gpt-6-sol", model: "gpt-6-sol", result: "answered", contextWindow: 120000,
      elapsedMs: 708, finishedAtMs: now - 10_900, planned: [], steps: [] },
  },
  {
    id: "preview-rate-limited", agent: "claude", timestampMs: now - 52_000,
    startedAtMs: now - 52_325, durationMs: 325, stream: true, success: false, statusCode: 429,
    model: "claude-sonnet", upstreamModel: "claude-sonnet", relayName: "Cloud Relay", relayId: "relay-a",
    route: {
      requestedModel: "claude-sonnet", model: "claude-sonnet", result: "failed",
      elapsedMs: 325, finishedAtMs: now - 52_000,
      planned: [{ relayId: "relay-a", relayName: "Cloud Relay", model: "claude-sonnet" }],
      steps: [{ relayId: "relay-a", relayName: "Cloud Relay", model: "claude-sonnet", outcome: "failed", status: 429, elapsedMs: 310 }],
    },
  },
];

export function installUsagePreviewMocks() {
  mockWindows("main");
  let capturingSession: string | undefined;
  const inventory = { enabled: true, claude_enabled: true, scripts: [] };
  let settings: Record<string, unknown> = { managerAgentFocus: "all" };
  const settingsResult = () => ({ status: "ok", revision: "preview", settings, user_scripts: inventory });
  mockIPC((command, args) => {
    const input = args as {
      action?: string;
      id?: string;
      range?: string;
      settings?: Record<string, unknown>;
      request?: { action: string; session: string; requestId?: string };
    } | undefined;
    if (command === "consume_pending_manager_navigation") return null;
    if (command === "load_settings" || command === "refresh_user_script_inventory") return settingsResult();
    if (command === "save_settings" && input?.settings) {
      settings = { ...input.settings };
      return settingsResult();
    }
    if (command === "config_doctor_request" && input?.action === "status") {
      return { status: "ok", message: "", settings: { enabled: false, source: "current" }, presets: [] };
    }
    if (command === "refresh_script_market") return { status: "ok", user_scripts: inventory,
      market: { status: "ok", message: "", indexUrl: "", updatedAt: "", scripts: [] } };
    if (command === "load_overview") return { status: "ok", codex_app: { status: "not_checked" }, management_shortcut: { status: "not_checked" } };
    if (command === "list_official_accounts") return { status: "ok", accounts: [] };
    if (command === "claude_page_extensions") return { status: "ok", extensions: [] };
    if (command === "claude_page_extension_market") return { status: "ok", entries: [] };
    if (command === "claude_enhance_status") return { status: "ok", delivery: "full", claudeExtensions: true,
      bashTimeout: { enabled: false, defaultEnabled: false, defaultSeconds: 120, maxEnabled: false, maxSeconds: 300 } };
    if (command === "get_official_model_catalog_status") return { status: "ok", visibleModels: [] };
    if (command === "check_env_conflicts") return { status: "ok", conflicts: [] };
    if (command === "load_gateway") return { status: "ok", ...payload };
    if (command === "load_gateway_usage") return { status: "ok", ...usage(input?.range ?? "30d") };
    if (command === "load_gateway_quota_history") return history[input?.id ?? ""] ?? [];
    if (command === "load_gateway_balances") return { status: "ok", balances: [] };
    if (command === "load_request_flow" || command === "load_request_logs") return { status: "ok", entries: routingEntries };
    if (command === "request_context_preview" && input?.request) {
      const { action, session, requestId } = input.request;
      if (action === "enable") capturingSession = session;
      if (action === "stop" && capturingSession === session) capturingSession = undefined;
      const active = capturingSession === session;
      const snippets = active && requestId === "preview-routed" ? [
        { role: "assistant", text: "（示例）上一次测试已覆盖模型选择与配额显示。", truncated: false },
        { role: "user", text: "（示例）请对比请求路由，并标出上下文窗口还剩多少空间。", truncated: false },
      ] : [];
      return { active, snippets };
    }
    return { status: "unsupported", message: "Browser preview" };
  }, { shouldMockEvents: true });
}
