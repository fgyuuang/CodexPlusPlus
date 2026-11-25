import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";

const shard = (name: string) => readFileSync(new URL(`../../../assets/inject/renderer-inject/${name}`, import.meta.url), "utf8");
const navigation = shard("50-navigation.js");
const marketplace = shard("60-plugin-marketplace.js");
const adapter = shard("62-plugin-market-adapter.js");
const clone = (value: any) => JSON.parse(JSON.stringify(value));
const authError = "ChatGPT authentication required for remote plugin catalog; API key auth is not supported";

function fixture() {
  let enabled = true;
  const diagnostics: any[] = [], backend: any[] = [];
  const cache = [{ name: "fixture-local", path: "C:/local/marketplace.json", plugins: [
    { name: "shared", installed: false }, { name: "cached" },
  ] }];
  const window: any = { __CODEX_PLUS_PLUGIN_MARKETPLACES__: clone(cache) };
  const context = vm.createContext({
    window, document: { documentElement: { toggleAttribute() {} } },
    codexPlusBackendSettingsLoaded: true, codexPluginMarketplaceUnlockVersion: "fixture",
    codexPlusSettings: () => ({ pluginMarketplaceUnlock: enabled }),
    codexPluginUsesBroadCatalogKinds: () => true, codexPlusIsWindowsPlatform: true,
    sendCodexPlusDiagnostic: (event: string, detail: any) => diagnostics.push({ event, detail }),
    appServerModelRequestMethod: (method: string, params: any) => method === "send-cli-request-for-host" ? params.method : method.split("/").at(-1),
    postJson: async (route: string, request: any) => {
      backend.push({ route, request });
      return { marketplaces: [{ name: "codex-plus-index-public", path: "C:/codex-plus-plugin-market/virtual-public/.agents/plugins/marketplace.json", plugins: [
        { id: "native-index@public", name: "native-index", codexPlusIndexId: "index-id", installed: true },
      ] }], marketplaceLoadErrors: [], featuredPluginIds: ["native-index@public"] };
    },
  });
  const end = marketplace.indexOf("  if (window.__CODEX_PLUS_TEST_PLUGIN_MARKETPLACE__)");
  const gate = marketplace.match(/^  function codexPluginMarketplacePatchEnabled\([^]*?^  \}/m)?.[0];
  assert.ok(end > 0 && gate);
  vm.runInContext(`${navigation}\n${marketplace.slice(0, end)}\n${gate}\n${adapter}`, context);
  return {
    context, window, cache, diagnostics, backend,
    enable: (value: boolean) => { enabled = value; },
    result: (result: any, options?: any) => context.patchPluginMarketplaceResult("list-plugins", result, options),
    client: (sendRequest: (...args: any[]) => any, hostId?: string) => {
      const client = { hostId, sendRequest };
      context.patchPluginMarketplaceRequestClient(client);
      return client;
    },
  };
}

test("local cache fills missing plugins once and preserves live installation state and source cache", () => {
  const f = fixture();
  const result = { marketplaces: [{ name: "fixture-local", plugins: [{ name: "shared", installed: true }] }], featuredPluginIds: ["shared"] };
  assert.equal(f.result(result), result);
  f.result(result);
  assert.deepEqual(result.marketplaces[0].plugins.map((plugin) => plugin.name), ["shared", "cached"]);
  assert.equal(result.marketplaces[0].plugins[0].installed, true);
  assert.deepEqual(result.featuredPluginIds, ["shared"]);
  assert.deepEqual(f.window.__CODEX_PLUS_PLUGIN_MARKETPLACES__, f.cache);
  assert.equal(f.diagnostics.filter((event) => event.event === "plugin_marketplace_local_merged").length, 1);
});

test("backend readiness and the unlock switch gate local cache projection", () => {
  const f = fixture();
  f.context.codexPlusBackendSettingsLoaded = false;
  assert.equal(f.result({ marketplaces: [] }).marketplaces.length, 0);
  f.context.codexPlusBackendSettingsLoaded = true;
  f.enable(false);
  assert.equal(f.context.localPluginMarketplaceFallbackResult().marketplaces.length, 0);
  f.enable(true);
  assert.equal(f.context.localPluginMarketplaceFallbackResult().marketplaces[0].name, "fixture-local");
  assert.equal(f.context.remoteOnlyPluginMarketplaceFallbackResult().marketplaces.length, 0);
});

test("the native client list keeps its index and also restores local cache without extra requests", async () => {
  const f = fixture();
  let calls = 0;
  const client = f.client(async () => { calls += 1; return { marketplaces: [{ name: "native", plugins: [] }], marketplaceLoadErrors: [] }; }, "local");
  const result = await client.sendRequest("list-plugins", {});
  assert.equal(result.__codexPlusIndexMerged, true);
  assert.deepEqual(Array.from(result.marketplaces, (market: any) => market.name), ["native", "codex-plus-index-public", "fixture-local"]);
  assert.equal(result.marketplaces[1].plugins[0].installed, true);
  assert.equal(result.marketplaces[1].plugins[0].codexPlusIndexId, "index-id");
  assert.equal(calls, 1);
  assert.equal(f.backend.length, 1);
  assert.equal(f.backend[0].route, "/plugin-market/native");
});

test("remote-only and remote-host clients never receive local caches or native indexes", async () => {
  for (const [host, method, params] of [
    ["local", "list-plugins", { marketplaceKinds: ["shared-with-me"] }],
    ["ssh:fixture", "list-plugins", {}],
    ["local", "send-cli-request-for-host", { hostId: "ssh:fixture", method: "list-plugins", params: {} }],
    ["local", "send-cli-request-for-host", { method: "list-plugins", params: { marketplaceKinds: ["created-by-me-remote"] } }],
  ] as const) {
    const f = fixture();
    const client = f.client(async () => ({ marketplaces: [{ name: "remote", plugins: [] }] }), host);
    const result = await client.sendRequest(method, params);
    assert.deepEqual(Array.from(result.marketplaces, (market: any) => market.name), ["remote"]);
    assert.equal(f.backend.length, 0);
  }
});

test("API-key catalog auth errors restore local lists while remote-only fallback stays empty", async () => {
  const f = fixture();
  const client = f.client(async () => { throw new Error(authError); }, "local");
  const local = await client.sendRequest("list-plugins", {});
  assert.ok(local.marketplaces.some((market: any) => market.name === "fixture-local"));
  const remote = await client.sendRequest("list-plugins", { marketplaceKinds: ["shared-with-me"] });
  assert.equal(remote.marketplaces.length, 0);
  const unrelated = f.client(async () => { throw new Error("unrelated request failure"); }, "local");
  await assert.rejects(unrelated.sendRequest("list-plugins", {}), /unrelated request failure/);
});

test("fetch, JSON-RPC and MCP responses keep request scope when merging cached lists", () => {
  for (const transport of ["fetch", "mcp"]) {
    for (const scope of ["local", "remote-only", "remote-host"]) {
      const f = fixture();
      const hostId = scope === "remote-host" ? "ssh:fixture" : "local";
      const params = scope === "remote-only" ? { marketplaceKinds: ["shared-with-me"] } : {};
      const request = transport === "fetch"
        ? { type: "fetch", requestId: "fixture", hostId, url: "list-plugins", body: JSON.stringify(params) }
        : { type: "mcp-request", hostId, request: { id: "fixture", method: "list-plugins", params } };
      f.context.patchPluginMarketplaceRequestMessage(request);
      const response = transport === "fetch"
        ? { type: "fetch-response", requestId: "fixture", bodyJsonString: JSON.stringify({ id: 1, result: { marketplaces: [] } }) }
        : { type: "mcp-response", hostId, message: { id: "fixture", result: { marketplaces: [] } } };
      assert.equal(f.context.patchPluginMarketplaceResponseData(response), true);
      const result = transport === "fetch" ? JSON.parse((response as any).bodyJsonString).result : (response as any).message.result;
      assert.equal(result.marketplaces.length, scope === "local" ? 1 : 0, `${transport} ${scope}`);
      if (scope === "local") assert.equal(result.marketplaces[0].plugins[1].id, "cached@fixture-local");
    }
  }
});
