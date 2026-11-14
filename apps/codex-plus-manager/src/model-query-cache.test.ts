import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(new URL("../../../assets/inject/renderer-inject/70-model-catalog.js", import.meta.url), "utf8");
const between = (start: string, end: string) => source.slice(source.indexOf(start), source.indexOf(end, source.indexOf(start)));
const functions = between("  function codexPlusModelDisplayName(", "  function modelJsonResponseLooksPatchable(")
  + between("  function refreshCodexModelQueryCache(", "  function refreshCodexUsageLimitQueryCache(");

type Model = { model: string; hidden: boolean; displayName?: string; modelProvider?: string };
type Query = { queryKey: unknown[]; state: { data: { data: Model[]; nextCursor?: string | null } } };
function fixture() {
  const route = "codex_plus_official_schedule:gpt-6.1-sol";
  const local: Query = { queryKey: ["models", "list", "local", "chatgpt"], state: { data: { data: [
    { model: "gpt-6.1-sol", hidden: false }, { model: route, hidden: true },
  ], nextCursor: null } } };
  const remote = structuredClone(local);
  remote.queryKey[2] = "ssh:other";
  const unrelated = structuredClone(local);
  unrelated.queryKey = ["thread", "list", "local"];
  const queries = [local, remote, unrelated];
  const listeners = new Set<(event: { type: string; query: Query }) => void>();
  let writes = 0;
  const cache = {
    findAll: () => queries,
    subscribe: (listener: (event: { type: string; query: Query }) => void) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
  const client = {
    getQueryCache: () => cache,
    setQueryData: (key: unknown[], data: Query["state"]["data"]) => {
      assert.ok(++writes < 10, "cache recovery must not feed its own updates forever");
      const query = queries.find((query) => query.queryKey === key)!;
      query.state.data = data;
      listeners.forEach((listener) => listener({ type: "updated", query }));
    },
  };
  const catalog = { status: "ok", model: "gpt-6.1-sol", default_model: "gpt-6.1-sol", models: ["gpt-6.1-sol", route] };
  const metadata = { [route]: { displayName: "GPT-6.1 Sol(官方顺序调度)", providerId: "codex_plus_official_schedule", sourceKind: "directOfficial" } };
  const window = {};
  const create = new Function("window", "client", "codexModelCatalog", "metadata", `
    const codexPlusModelCollectionSnapshots = new WeakMap(), codexPlusModelDefaultSnapshots = new WeakMap();
    const codexPlusBackendSettingsLoaded = true;
    const codexPlusBackendSettings = {};
    const codexPlusModelUnlockEnabled = () => true;
    const codexPlusModelNames = () => codexModelCatalog.models;
    const codexPlusModelMetadata = (name) => metadata[name];
    const modelReasoningEfforts = () => [];
    const codexRemoteSessionActiveProfile = () => null;
    const findCodexUsageQueryClient = () => client;
    const sendCodexPlusDiagnostic = () => {};
    ${functions}
    return { refresh: () => refreshCodexModelQueryCache(client), install: installCodexModelQueryCachePatch };
  `) as (window: object, client: unknown, catalog: unknown, metadata: unknown) => { refresh: () => number; install: () => void };
  return { local, remote, unrelated, catalog, listeners, writes: () => writes, create: () => create(window, client, catalog, metadata) };
}

test("warm local model cache exposes source models and preserves routing IDs without mutating the old snapshot", () => {
  const f = fixture(), runtime = f.create(), original = f.local.state.data;
  assert.equal(runtime.refresh(), 1);
  assert.equal(original.data[1].hidden, true);
  assert.equal(f.local.state.data.data[1].hidden, false);
  assert.equal(f.local.state.data.data[1].model, "codex_plus_official_schedule:gpt-6.1-sol");
  assert.equal(f.local.state.data.data[1].modelProvider, "codex_plus_official_schedule");
  assert.equal(f.local.state.data.data[1].displayName, "GPT-6.1 Sol(官方顺序调度)");
  assert.equal(f.local.state.data.nextCursor, null);
  assert.equal(f.remote.state.data.data[1].hidden, true);
  assert.equal(f.unrelated.state.data.data[1].hidden, true);
  assert.equal(runtime.refresh(), 0);
});

test("failed catalog leaves native cached models alone", () => {
  const f = fixture(); f.catalog.status = "failed";
  assert.equal(f.create().refresh(), 0);
  assert.equal(f.writes(), 0);
});

test("native refetches are recovered once and reinjection replaces the old subscriber", () => {
  const f = fixture(), first = f.create();
  first.install(); first.install();
  assert.equal(f.listeners.size, 1);
  assert.equal(f.writes(), 1);
  f.local.state.data = { data: [{ model: "gpt-6.1-sol", hidden: false }] };
  f.listeners.forEach((listener) => listener({ type: "updated", query: f.local }));
  assert.equal(f.local.state.data.data.length, 2);
  assert.equal(f.writes(), 2);
  f.create().install();
  assert.equal(f.listeners.size, 1);
  assert.equal(f.writes(), 2);
});
