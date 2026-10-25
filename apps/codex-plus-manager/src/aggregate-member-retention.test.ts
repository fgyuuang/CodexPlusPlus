import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import test from "node:test";
import ts from "typescript";
import { clampAggregateRoutePriority } from "./aggregate-routes.ts";

const source = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const names = ["aggregateMemberCandidates", "normalizeAggregateConfig", "normalizeAggregateRelayProfile",
  "isApiRelayProfile", "isOfficialLoginProfile", "isAggregateRelayProfile", "isCliproxyManagedProfile",
  "isNewapiManagedProfile", "clampAggregateWeight"];
const functions = names.map((name) => {
  const declaration = ast.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === name);
  assert.ok(declaration, name);
  return declaration.getText(ast);
});
const compiled = ts.transpileModule(functions.join("\n"), {
  compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS },
}).outputText;
const context = vm.createContext({
  clampAggregateRoutePriority,
  aggregateStrategyOptions: [{ value: "failover" }],
  officialLoginModelList: () => [],
  inferAggregateModelList: (profile: any) => profile.modelList,
  normalizeRelaySessionProvider: () => "custom",
});
vm.runInContext(compiled, context);
const run = (name: string, ...args: unknown[]) => JSON.parse(JSON.stringify(context[name](...args)));
const api = (id: string) => ({ id, name: id, baseUrl: "https://example.test/v1", apiKey: "test-key",
  relayMode: "pureApi", officialMixApiKey: false, integrationType: "", model: "model", modelList: "model" });
const profile = () => ({ ...api("aggregate"), relayMode: "aggregate", aggregate: {
  strategy: "failover", members: [{ profileId: "a", weight: 4 }, { profileId: "b", weight: 7 }],
  modelMappings: [{ codexModel: "model", targets: [{ profileId: "a", targetModel: "model" }] }],
  routes: [{ pattern: "model", profileId: "a", priority: 5 }],
} });

test("saving an aggregate retains API members whose URL or Key is temporarily empty", () => {
  for (const field of ["apiKey", "baseUrl"]) {
    const aggregate = profile();
    const settings = { relayProfiles: [{ ...api("a"), [field]: "" }, api("b"), aggregate] };
    const result = run("normalizeAggregateRelayProfile", aggregate, settings);
    assert.deepEqual(result.aggregate.members, aggregate.aggregate.members, field);
    assert.deepEqual(result.aggregate.modelMappings, aggregate.aggregate.modelMappings, field);
    assert.deepEqual(result.aggregate.routes, aggregate.aggregate.routes, field);
    const saved = JSON.parse(JSON.stringify(result));
    assert.deepEqual(saved.aggregate.members.map((member: any) => member.profileId), ["a", "b"]);
    assert.deepEqual(run("aggregateMemberCandidates", settings, "aggregate", true).map((item: any) => item.id), ["b"]);
  }
});

test("reference retention still excludes official logins, dedicated integrations and nested aggregates", () => {
  const settings = { relayProfiles: [api("a"), { ...api("official"), relayMode: "official" },
    { ...api("cliproxy"), integrationType: "cliproxy" }, { ...api("newapi"), integrationType: "newapi" },
    { ...api("nested"), relayMode: "aggregate" }, profile()] };
  assert.deepEqual(run("aggregateMemberCandidates", settings, "aggregate").map((item: any) => item.id), ["a"]);
});

test("deleting a member is still reflected when a remaining API profile exists", () => {
  const aggregate = profile();
  const settings = { relayProfiles: [api("b"), aggregate] };
  assert.deepEqual(run("normalizeAggregateRelayProfile", aggregate, settings).aggregate.members,
    [{ profileId: "b", weight: 7 }]);
});

test("an empty known candidate set removes excluded references, while missing settings preserve drafts", () => {
  const aggregate = profile();
  const settings = { relayProfiles: [{ ...api("a"), relayMode: "official" }, aggregate] };
  assert.deepEqual(run("normalizeAggregateRelayProfile", aggregate, settings).aggregate.members, []);
  assert.deepEqual(run("normalizeAggregateRelayProfile", aggregate, null).aggregate.members,
    aggregate.aggregate.members);
});
