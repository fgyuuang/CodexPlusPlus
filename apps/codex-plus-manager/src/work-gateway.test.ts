import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { runInNewContext } from "node:vm";

const source = readFileSync(new URL("../../../assets/inject/renderer-inject/72-work-gateway.js", import.meta.url), "utf8");
const cwd = "D:\\project";
const model = "group/mock";
const autoModel = "codex-plus-auto/gpt-official";
const id = "12345678-1234-1234-1234-123456789abc";

function fixture() {
  const settings = { enabled: true, active: true, provider: "custom", reportedModel: model, requestError: null as Error | null };
  const catalog = {
    models: [model, autoModel, "vendor:alias", "vendor/pin", "gpt-official", "bare/namespace",
      "codex-plus-auto/", "group/", "codex-plus-auto/not-routable", "group/not-routable"],
    modelMetadata: {
      "vendor/pin": { codex_plus_group_id: "vendor" },
      "bare/namespace": { codex_plus_group_id: "other-vendor" },
    },
  };
  const run = { effectiveLocation: "cloud", canChooseLocation: false, unavailableReasons: { local: null as string | null } };
  const composer = { conversationOrigin: "tpp", workspaceAgent: null, targetGizmoId: null };
  const state = { composerProps: composer, isTemporaryChat: false, runLocationState: run, pendingFileAttachments: ["private"] };
  const projectProps = { isWorkHome: true, isLocalWorkComposer: false, cwd, hostId: "local" };
  const project = { isConnected: true, form: { __reactFiber$mock: { memoizedProps: { state } } },
    __reactFiber$mock: { memoizedProps: projectProps } };
  const calls: unknown[] = [];
  const navigation: unknown[] = [];
  const api = runInNewContext(source + "\n({codexPlusWorkGatewayDirectory,codexPlusWorkGatewayModels,codexPlusOpenWorkGatewayDraft})", {
    codexGatewaySessionRoutingEnabled: () => settings.enabled,
    codexModelCatalog: catalog,
    codexRemoteSessionTargetProvider: (name: string) => name.endsWith("/not-routable") ? "" : settings.provider,
    codexPlusAppServerClient: async (host: string) => {
      assert.equal(host, "local");
      return { sendRequest: async (method: string, params: unknown, options: { timeoutMs: number }) => {
        assert.equal(options.timeoutMs, 20000);
        calls.push({ method, params });
        if (settings.requestError) throw settings.requestError;
        return { thread: { id }, model: settings.reportedModel, modelProvider: settings.provider };
      } };
    },
    codexGatewayConnectionGeneration: async () => ({ active: settings.active }),
    window: { dispatchEvent: (event: { data: unknown }) => navigation.push(event.data) },
    MessageEvent,
  });
  return { api, settings, catalog, project, projectProps, composer, state, run, calls, navigation };
}

test("Work offers only routable gateway entries and opens an empty local chat", async () => {
  const { api, settings, project, calls, navigation } = fixture();
  assert.deepEqual(Array.from(api.codexPlusWorkGatewayModels()), [model, autoModel, "vendor:alias", "vendor/pin"]);
  settings.reportedModel = autoModel;
  assert.equal(await api.codexPlusOpenWorkGatewayDraft(project, autoModel), true);
  assert.deepEqual(JSON.parse(JSON.stringify(calls)), [{ method: "thread/start", params: { cwd, model: autoModel, modelProvider: "custom" } }]);
  assert.deepEqual(JSON.parse(JSON.stringify(navigation)), [{ type: "navigate-to-route", path: "/local/" + id }]);
});

test("local Work and a switchable cloud Work project can use the gateway; other composers cannot", () => {
  const { api, project, projectProps, composer, state, run } = fixture();
  assert.equal(api.codexPlusWorkGatewayDirectory(project), cwd);
  run.canChooseLocation = true;
  assert.equal(api.codexPlusWorkGatewayDirectory(project), cwd);
  run.effectiveLocation = "local";
  projectProps.isLocalWorkComposer = true;
  assert.equal(api.codexPlusWorkGatewayDirectory(project), cwd);
  run.effectiveLocation = "cloud";
  projectProps.isLocalWorkComposer = false;
  run.canChooseLocation = false;
  for (const [obj, key, value] of [
    [projectProps, "isWorkHome", false], [projectProps, "isLocalWorkComposer", true],
    [projectProps, "hostId", "ssh"], [projectProps, "cwd", "relative"],
    [state, "isTemporaryChat", true], [composer, "workspaceAgent", {}], [composer, "conversationOrigin", "chat"],
    [run, "effectiveLocation", "local"], [run.unavailableReasons, "local", "policy-disabled"],
  ] as const) {
    const data = obj as Record<string, unknown>;
    const previous = data[key];
    data[key] = value;
    assert.equal(api.codexPlusWorkGatewayDirectory(project), "", key);
    data[key] = previous;
  }
});

test("disabled gateway, stale choice and mismatched server result fail closed", async () => {
  const { api, project, settings, catalog, calls, navigation } = fixture();
  settings.enabled = false;
  assert.equal(await api.codexPlusOpenWorkGatewayDraft(project, autoModel), false);
  settings.enabled = true;
  settings.active = false;
  assert.equal(await api.codexPlusOpenWorkGatewayDraft(project, autoModel), false);
  settings.active = true;
  catalog.models = catalog.models.filter((entry) => entry !== autoModel);
  assert.equal(await api.codexPlusOpenWorkGatewayDraft(project, autoModel), false);
  assert.equal(calls.length, 0);
  catalog.models.push(autoModel);
  settings.reportedModel = "gpt-incorrect";
  assert.equal(await api.codexPlusOpenWorkGatewayDraft(project, autoModel), false);
  assert.equal(calls.length, 1);
  assert.equal(navigation.length, 0);
});

test("a failed local start is not replayed or redirected", async () => {
  const { api, project, settings, calls, navigation } = fixture();
  settings.requestError = new Error("response lost");
  await assert.rejects(api.codexPlusOpenWorkGatewayDraft(project, autoModel), /response lost/);
  assert.equal(calls.length, 1);
  assert.equal(navigation.length, 0);
});
