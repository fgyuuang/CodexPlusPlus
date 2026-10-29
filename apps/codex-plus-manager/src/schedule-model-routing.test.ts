import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

async function routingRuntime(supported = ["medium", "high"]) {
  const source = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
  const routesStart = source.indexOf("  function codexRemoteSessionActiveProfile(");
  const routesEnd = source.indexOf("\n  function codexRemoteSessionStartedThreadId(", routesStart);
  const normalizeStart = source.indexOf("  function codexModelRequestMethodSupportsReasoning(");
  const normalizeEnd = source.indexOf("\n  function installCodexModelRequestBridgePatch(", normalizeStart);
  assert.ok(routesStart >= 0 && routesEnd > routesStart && normalizeStart >= 0 && normalizeEnd > normalizeStart);
  const fixed = "codex_plus_official_account2:gpt-6.1-sol";
  const scheduled = "codex_plus_official_schedule:gpt-6.1-sol";
  const catalog = { routeDescriptors: [
    { routingSlug: fixed, providerId: "codex_plus_official_account2" },
    { routingSlug: scheduled, providerId: "codex_plus_official_schedule" },
    { routingSlug: "gpt-6.1-sol", providerId: "openai" },
  ] };
  const create = new Function("codexModelCatalog", "supported", `
    const codexPlusBackendSettings = { officialExperience: { enabled: true } };
    const codexCatalogOfficialModels = () => ["gpt-6.1-sol"];
    const sendCodexPlusDiagnostic = () => {};
    const appServerModelRequestMethod = method => method;
    const codexModelReasoningEffortValues = () => supported;
    const codexPlusModelMetadata = () => ({ defaultReasoningEffort: "medium" });
    const codexBuiltInReasoningMetadata = () => null;
    ${source.slice(routesStart, routesEnd)}
    ${source.slice(normalizeStart, normalizeEnd)}
    return { normalize: normalizeCodexModelReasoningParams, patch: patchCodexModelReasoningRequestMessage };
  `);
  return { ...create(catalog, supported), fixed, scheduled };
}

test("a scheduled turn replaces the stale fixed-account collaboration model before dispatch", async () => {
  const { normalize, fixed, scheduled } = await routingRuntime();
  const collaborationMode = { mode: "plan", settings: {
    model: fixed, reasoning_effort: "ultra", developer_instructions: "keep plan instructions",
  } };
  const input = [{ type: "text", text: "private input" }];
  const params = { threadId: "scheduled", model: scheduled, reasoningEffort: "high", input,
    collaborationMode, cwd: "C:/work", permissions: "workspace" };
  const result = normalize("turn/start", params);
  assert.equal(result.modelProvider, "codex_plus_official_schedule");
  assert.equal(result.collaborationMode.settings.model, scheduled);
  assert.equal(result.collaborationMode.settings.reasoning_effort, "high");
  assert.equal(result.collaborationMode.settings.developer_instructions, "keep plan instructions");
  assert.equal(result.collaborationMode.mode, "plan");
  assert.strictEqual(result.input, input);
  assert.equal(result.cwd, params.cwd);
  assert.equal(result.permissions, params.permissions);
  assert.equal(collaborationMode.settings.model, fixed, "must not mutate the caller's cached mode");
  const inherited = normalize("turn/start", { threadId: "scheduled", input: [], collaborationMode });
  assert.equal(inherited.model, scheduled);
  assert.equal(inherited.collaborationMode.settings.model, scheduled);
  assert.equal(inherited.collaborationMode.settings.reasoning_effort, "medium");
});

test("bridge envelopes keep collaboration models paired with fixed, scheduled, and native sources", async () => {
  const { patch, fixed, scheduled } = await routingRuntime();
  for (const [model, provider] of [[scheduled, "codex_plus_official_schedule"],
    [fixed, "codex_plus_official_account2"], ["gpt-6.1-sol", "openai"]]) {
    for (const type of ["mcp-request", "worker-request", "send-cli-request-for-host", "start-turn-for-host", "rpc"]) {
      const params = { model, reasoningEffort: "ultra", collaborationMode: { mode: "default",
        settings: { model: "stale-account:old-model", reasoning_effort: "ultra" } } };
      const message = ["mcp-request", "worker-request"].includes(type)
        ? { type, request: { method: "turn/start", params } }
        : { type, method: "turn/start", params };
      const result = patch(message);
      const actual = result.request?.params || result.params;
      assert.equal(actual.modelProvider, provider, type);
      assert.equal(actual.collaborationMode.settings.model, model, type);
      assert.equal(actual.reasoningEffort, "medium", type);
      assert.equal(actual.collaborationMode.settings.reasoning_effort, "medium", type);
    }
  }
});

test("fetch envelopes synchronize nested models without guessing unavailable reasoning capabilities", async () => {
  const { patch, scheduled } = await routingRuntime([]);
  const params = { model: scheduled, collaborationMode: { mode: "plan", settings: {
    model: "stale-account:old-model", reasoning_effort: "ultra",
  } } };
  for (const body of [params, { params }]) {
    const result = patch({ type: "fetch", url: "turn/start", body: JSON.stringify(body) });
    const decoded = JSON.parse(result.body);
    const actual = decoded.params || decoded;
    assert.equal(actual.modelProvider, "codex_plus_official_schedule");
    assert.equal(actual.collaborationMode.settings.model, scheduled);
    assert.equal(actual.collaborationMode.settings.reasoning_effort, "ultra");
    assert.equal(actual.reasoningEffort, undefined);
  }
});
