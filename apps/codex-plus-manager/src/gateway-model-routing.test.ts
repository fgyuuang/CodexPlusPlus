import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const routes = await readFile(new URL("../../../assets/inject/renderer-inject/30-service-tier.js", import.meta.url), "utf8");
const models = await readFile(new URL("../../../assets/inject/renderer-inject/70-model-catalog.js", import.meta.url), "utf8");
const between = (text: string, start: string, end: string) => text.slice(text.indexOf(start), text.indexOf(end, text.indexOf(start)));
const helpers = routes.slice(routes.indexOf("  function codexRemoteSessionActiveProfile("), routes.indexOf("  function codexRemoteSessionStartedThreadId("))
  + between(models, "  function codexModelRequestMethodSupportsReasoning(", "  function installCodexModelRequestBridgePatch(");
const switcher = between(models, "  async function prepareCodexGatewaySourceRequest(", "  function patchCodexMcpRequestTransport(");
const brokerPatch = between(models, "  function patchCodexMcpRequestTransport(", "  function patchCodexVscodeRequestTransport(");
const vscodePatch = between(models, "  function patchCodexVscodeRequestTransport(", "  const appServerModelRequestPatchMaxMisses");
const model = "local-official-account2/gpt-6.1-sol";
const previousProvider = "codex_plus_official_schedule";

function runtime(provider: "openai" | "custom", post = async (_path: string, _payload: any): Promise<any> => { throw new Error("unexpected write"); }) {
  const create = new Function("provider", "model", "postJson", `
    const window = {};
    const codexAppServerModelRequestPatchVersion = "gateway-test";
    const codexPlusBackendSettings = {
      officialExperience: { enabled: true }, officialExperienceActive: false,
      relayProfilesEnabled: true, activeRelayId: "codex-plus-gateway",
      relayProfiles: [{ id: "codex-plus-gateway", relayMode: "pureApi", configContents: 'model_provider = "' + provider + '"' }],
    };
    const codexModelCatalog = { status: "ok", model_provider: "codex-plus-gateway", codex_model_provider: provider, models: [model, "gpt-6.1-sol"] };
    const codexCatalogOfficialModels = () => ["gpt-6.1-sol"];
    const sendCodexPlusDiagnostic = () => {};
    const appServerModelRequestMethod = method => method.replace("vscode://codex/", "");
    const codexModelReasoningEffortValues = () => ["medium", "high"];
    const codexPlusModelMetadata = () => ({ defaultReasoningEffort: "medium" });
    const codexBuiltInReasoningMetadata = () => null;
    const loadBackendSettingsState = async () => true;
    const loadCodexModelCatalog = async () => codexModelCatalog;
    const codexRateLimitUnlockRequestMethod = () => false;
    const neutralizeCodexRateLimitPayload = x => x;
    ${helpers}
    ${switcher}
    ${brokerPatch}
    ${vscodePatch}
    return { normalize: normalizeCodexModelReasoningParams, patchBroker: patchCodexMcpRequestTransport,
      patchVscode: patchCodexVscodeRequestTransport, prepare: prepareCodexGatewaySourceRequest,
      switchProvider: switchCodexOfficialThreadProvider, settings: codexPlusBackendSettings, catalog: codexModelCatalog };
  `);
  return create(provider, model, post);
}

for (const provider of ["openai", "custom"] as const) {
  test(`field-free native legacy resume reads its saved model before migrating to ${provider}`, async () => {
    const threadId = "019f6000-0000-7000-8000-000000000001";
    const writes: string[] = [];
    const r = runtime(provider, async (path, payload) => {
      writes.push(path);
      if (path === "/thread-resume-model-metadata") {
        assert.equal(payload.thread_id, threadId);
        assert.equal(payload.host_id, "local");
        return { status: "ok", threadId, model: "gpt-6.1-sol", reasoningEffort: "high" };
      }
      assert.equal(path, "/session/gateway-provider/migrate");
      assert.equal(payload.model, model);
      return { status: "ok", threadId, modelProvider: provider };
    });
    r.catalog.legacyProviderRoutes = { [`${previousProvider}:gpt-6.1-sol`]: model };
    const input = { threadId, excludeTurns: true };
    const result = await r.prepare(async () => ({ thread: {
      id: threadId, modelProvider: previousProvider, status: { type: "notLoaded" },
    } }), "thread/resume", input);
    assert.equal(result.model, model);
    assert.equal(result.modelProvider, provider);
    assert.deepEqual(input, { threadId, excludeTurns: true });
    assert.deepEqual(writes, ["/thread-resume-model-metadata", "/session/gateway-provider/migrate"]);
    const bad = runtime(provider, async () => ({ status: "ok", threadId: "other", model: "gpt-6.1-sol" }));
    await assert.rejects(bad.prepare(async () => ({ thread: {
      id: threadId, modelProvider: previousProvider, status: { type: "notLoaded" },
    } }), "thread/resume", input), /无法读取/);
  });

  test(`legacy branch inherits the exact gateway route with ${provider}, preserving the parent and permissions`, async () => {
    const r = runtime(provider);
    r.catalog.legacyProviderRoutes = { [`${previousProvider}:gpt-6.1-sol`]: model };
    const params = { threadId: "parent-thread", cwd: "C:/work", permissions: "strict", config: { keep: true } };
    const sent: string[] = [];
    const result = await r.prepare(async (method: string, body: any) => {
      sent.push(method); assert.equal(body.includeTurns, false);
      return { thread: { id: params.threadId, model: "gpt-6.1-sol", modelProvider: previousProvider } };
    }, "thread/fork", params);
    assert.deepEqual(sent, ["thread/read"]);
    assert.equal(result.model, model); assert.equal(result.modelProvider, provider);
    assert.equal(result.config.model_provider, provider); assert.equal(result.config.keep, true);
    assert.equal(result.permissions, "strict"); assert.equal(result.threadId, params.threadId);
    assert.equal((params as any).modelProvider, undefined);
    const prediction = { ...params, ephemeral: true, experimentalPredictionMode: true };
    assert.strictEqual(await r.prepare(() => { throw new Error("prediction must inherit natively"); }, "thread/fork", prediction), prediction);
    assert.strictEqual(r.normalize("thread/fork", prediction), prediction);
    r.catalog.legacyProviderRoutes = {};
    await assert.rejects(r.prepare(async () => ({ thread: { id: params.threadId, model: "unknown", modelProvider: previousProvider } }), "thread/fork", params), /尚无唯一/);
  });

  test(`native provider guard resumes one unloaded legacy chat with ${provider} without replay or fork`, async () => {
    let storedProvider = previousProvider;
    let writes = 0;
    const r = runtime(provider, async (path, payload) => {
      assert.equal(path, "/session/gateway-provider/migrate");
      assert.equal(payload.sourceProvider, previousProvider); assert.equal(payload.targetProvider, provider);
      assert.equal(payload.nativeUnloaded, true); writes++; storedProvider = provider;
      return { status: "ok", threadId: payload.threadId, modelProvider: provider };
    });
    const threadId = "019f6000-0000-7000-8000-000000000001";
    const methods: string[] = [];
    const input: any[] = [];
    const params = { threadId, model, permissions: "strict", cwd: "C:/work", input };
    const result = await r.switchProvider(async (method: string, body: any) => {
      methods.push(method); assert.equal(body.threadId, threadId);
      if (method === "thread/read") return { thread: { id: threadId, modelProvider: storedProvider, status: { type: "notLoaded" } } };
      assert.equal(method, "thread/resume");
      if (storedProvider !== provider) throw new Error(`This chat uses ${storedProvider}, but your current provider is ${provider}. Switch to ${storedProvider} to continue.`);
      assert.equal(body.permissions, "strict"); assert.equal(body.cwd, "C:/work");
      return { thread: { id: threadId }, modelProvider: provider };
    }, "turn/start", params, provider);
    assert.equal(result, provider); assert.equal(writes, 1);
    assert.deepEqual(methods, ["thread/resume", "thread/read", "thread/resume"]);
    assert.strictEqual(params.input, input);
  });
}

test("legacy recovery refuses active/still-loaded chats and failed migrations before dispatch", async () => {
  let writes = 0;
  const r = runtime("openai", async () => { writes++; return { status: "error", message: "backup failed" }; });
  const params = { threadId: "019f6000-0000-7000-8000-000000000001", model };
  for (const status of ["active", "idle", "notLoaded"]) {
    await assert.rejects(r.prepare(async (method: string) => method === "thread/unsubscribe" ? {} : ({ thread: {
      id: params.threadId, modelProvider: previousProvider, status: { type: status },
    } }), "thread/resume", params), status === "active" ? /回合和审批/ : status === "idle" ? /仍被原生运行时占用/ : /backup failed/);
  }
  assert.equal(writes, 1);
});

for (const provider of ["openai", "custom"] as const) {
  test(`gateway ${provider} pairs slash models with its transport and replaces stale collaboration models`, () => {
    const r = runtime(provider);
    const collaborationMode = { mode: "plan", settings: { model: `${previousProvider}:gpt-6.1-sol`, reasoning_effort: "ultra", developer_instructions: "keep" } };
    const params = { threadId: "same-thread", model, modelProvider: previousProvider, collaborationMode, input: [] };
    const normalized = r.normalize("turn/start", params);
    assert.equal(normalized.modelProvider, provider);
    assert.equal(normalized.collaborationMode.settings.model, model);
    assert.equal(normalized.collaborationMode.settings.reasoning_effort, "medium");
    assert.equal(normalized.collaborationMode.settings.developer_instructions, "keep");
    assert.equal(collaborationMode.settings.model, `${previousProvider}:gpt-6.1-sol`);
    assert.strictEqual(normalized.input, params.input);
    const next = r.normalize("turn/start", { threadId: "same-thread", input: [] });
    assert.equal(next.model, model);
    assert.equal(next.modelProvider, provider);
    assert.equal(r.normalize("thread/resume", { threadId: "same-thread", model, modelProvider: previousProvider }).modelProvider, provider);
    assert.throws(() => r.normalize("turn/start", { model: "local-official-unknown/unknown" }), /没有可用的来源路由/);
    r.catalog.model_provider = "old-direct-profile";
    assert.throws(() => r.normalize("turn/start", { model }), /没有可用的来源路由/);
  });

  for (const transport of ["broker", "vscode"] as const) {
    test(`${transport} switches the original scheduled chat to gateway ${provider} before dispatch`, { timeout: 10000 }, async () => {
      const r = runtime(provider);
      const sent: { method: string; params: Record<string, any> }[] = [];
      let activeProvider = previousProvider;
      let unsubscribed = false;
      const respond = (method: string, params: Record<string, any>) => {
        sent.push({ method, params });
        if (method === "thread/read") return { thread: { id: "same-thread", modelProvider: activeProvider, status: { type: "idle" } } };
        if (method === "thread/unsubscribe") { unsubscribed = true; return {}; }
        if (method === "thread/resume") {
          if (unsubscribed) activeProvider = params.modelProvider;
          unsubscribed = false;
          return { thread: { id: "same-thread", status: { type: "idle" } }, modelProvider: activeProvider,
            cwd: "C:/work", approvalPolicy: "never", sandbox: { type: "readOnly" }, reasoningEffort: "high" };
        }
        if (method === "thread/settings/update") return {};
        assert.equal(method, "turn/start");
        assert.equal(activeProvider, provider);
        assert.equal(params.modelProvider, provider);
        assert.equal(params.collaborationMode.settings.model, model);
        return { turn: { status: "inProgress" } };
      };
      const params = { threadId: "same-thread", model, modelProvider: previousProvider, input: [],
        collaborationMode: { mode: "default", settings: { model: `${previousProvider}:old` } } };
      if (transport === "vscode") {
        const native = { pendingRequests: new Map(), async sendRequest(_method: string, url: string, options: { body: string }) {
          if (!url.startsWith("vscode://codex/") || !this.pendingRequests) throw new Error("invalid transport");
          return respond(url.replace("vscode://codex/", ""), JSON.parse(options.body));
        } };
        assert.equal(r.patchVscode({ Transport: class { static getInstance() { return native; } } }), true);
        await native.sendRequest("POST", "vscode://codex/turn/start", { body: JSON.stringify(params) });
      } else {
        const handlers = new Map<string, Set<(message: any) => void>>();
        let done: () => void;
        const completed = new Promise<void>((resolve) => { done = resolve; });
        const broker = {
          handlers, bufferedMessages: new Map(),
          subscribe(type: string, listener: (message: any) => void) {
            const set = handlers.get(type) || new Set(); set.add(listener); handlers.set(type, set);
            return () => set.delete(listener);
          },
          deliverOrBufferMessage(event: any) { for (const listener of handlers.get(event.type) || []) listener(event); },
          dispatchMessage(_type: string, detail: any) {
            const { id, method, params } = detail.request;
            const result = respond(method, params);
            queueMicrotask(() => {
              broker.deliverOrBufferMessage({ type: "mcp-response", hostId: "local", message: { id, result } });
              if (method === "turn/start") done();
            });
          },
        };
        assert.equal(r.patchBroker({ Broker: broker }), true);
        broker.dispatchMessage("mcp-request", { hostId: "local", request: { id: "user-request", method: "turn/start", params } });
        await completed;
      }
      assert.deepEqual(sent.map((request) => request.method), ["thread/read", "thread/resume", "thread/unsubscribe", "thread/resume", "thread/settings/update", "turn/start"]);
      assert.equal(sent.filter((request) => request.method === "turn/start").length, 1);
      assert.equal(sent[3].params.threadId, "same-thread");
      assert.equal(sent[3].params.config.model_provider, provider);
      assert.equal(sent[3].params.cwd, "C:/work");
      assert.equal(sent[3].params.approvalPolicy, "never");
    });
  }
}
