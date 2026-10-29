import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import vm from "node:vm";
import ts from "typescript";

import { providerAvatarText, resolveProviderName, tomlSectionStringValue } from "./provider-name.ts";

// 执行实际编辑器 helper，避免只测名称解析器而遗漏写回时的覆盖。
function editorRuntime() {
  const source = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
  const ast = ts.createSourceFile("App.tsx", source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const names = new Set([
    "tomlStructureMask", "tomlQuotedValue", "tomlDottedKeyPath", "tomlKeyPathName", "codexProviderSectionName", "tomlKeyAssignment", "tomlInlineComment",
    "setCodexProviderStringKey", "ensureCodexProviderDefaults", "setTomlSectionBoolKey",
    "setTomlSectionStringKey", "setTomlSectionRawKey", "tomlSectionName", "tomlString",
    "ensureTrailingNewline", "joinTomlSections", "rootTomlStringValue", "tomlStringAssignmentValue",
    "setRootTomlStringKey", "setRootTomlLine", "removeRootTomlKey", "splitTomlRootAndTables",
  ]);
  const functions = ast.statements.filter((node) => ts.isFunctionDeclaration(node) && names.has(node.name?.text || ""));
  const code = functions.map((node) => node.getText(ast)).join("\n");
  const context = vm.createContext({ resolveProviderName });
  vm.runInContext(ts.transpileModule(code, { compilerOptions: { target: ts.ScriptTarget.ES2022 } }).outputText, context);
  return context;
}

test("editing provider URL preserves its custom label and requested auth policy", () => {
  const editor = editorRuntime();
  const original = 'model_provider = "custom"\n[model_providers.custom]\nname = "My relay"\nbase_url = "https://old.invalid/v1"\n';
  for (const requiresOpenAiAuth of [false, true]) {
    const updated = editor.setCodexProviderStringKey(original, "base_url", "https://new.invalid/v1", { requiresOpenAiAuth });
    assert.equal(tomlSectionStringValue(updated, "model_providers.custom", "name"), "My relay");
    assert.equal(tomlSectionStringValue(updated, "model_providers.custom", "base_url"), "https://new.invalid/v1");
    assert.match(updated, new RegExp(`requires_openai_auth = ${requiresOpenAiAuth}`));
  }
});

test("editor fills missing or blank names while retaining local default auth isolation", () => {
  const editor = editorRuntime();
  for (const name of ["", 'name = "   "\n']) {
    const updated = editor.ensureCodexProviderDefaults(`[model_providers.custom]\n${name}`, "custom");
    assert.equal(tomlSectionStringValue(updated, "model_providers.custom", "name"), "custom");
    assert.match(updated, /requires_openai_auth = false/);
  }
});

// issue #1886：用户把 name 改成 OpenAi，保存/重启后不能被改回 custom
test("keeps a user-customized provider name", () => {
  const contents = `model_provider = "openai"

[model_providers.custom]
name = "OpenAi"
wire_api = "responses"
`;
  assert.equal(resolveProviderName(contents, "custom"), "OpenAi");
});

test("falls back to the table id when name is missing", () => {
  const contents = `model_provider = "custom"

[model_providers.custom]
wire_api = "responses"
`;
  assert.equal(resolveProviderName(contents, "custom"), "custom");
});

test("falls back to the table id when name is blank", () => {
  const contents = `[model_providers.custom]
name = "   "
`;
  assert.equal(resolveProviderName(contents, "custom"), "custom");
});

test("falls back when the provider table does not exist yet", () => {
  assert.equal(resolveProviderName("model = \"gpt-5.6\"\n", "custom"), "custom");
});

test("only reads the name from the requested section", () => {
  const contents = `[model_providers.other]
name = "Other"

[model_providers.custom]
name = "Mine"
`;
  assert.equal(resolveProviderName(contents, "custom"), "Mine");
  assert.equal(resolveProviderName(contents, "other"), "Other");
});

test("does not leak a root-level name into the section lookup", () => {
  const contents = `name = "root-level"

[model_providers.custom]
wire_api = "responses"
`;
  assert.equal(tomlSectionStringValue(contents, "model_providers.custom", "name"), "");
  assert.equal(resolveProviderName(contents, "custom"), "custom");
});

test("handles single quotes and escaped characters", () => {
  const contents = `[model_providers.custom]
name = 'My \\'Relay\\''
`;
  assert.equal(resolveProviderName(contents, "custom"), "My 'Relay'");
});

test("provider avatar prefers two characters of different kinds", () => {
  const cases: Array<[string, string]> = [
    ["哈基米 国模 kimi", "哈K"],
    ["供应商 4", "供4"],
    ["a 哥国模", "A哥"],
    ["聚合供应商 1", "聚1"],
    ["哈基米 国模", "哈国"],
    ["Max JOJO Code（ccswitch）", "MJ"],
    ["DeepSeek", "DS"],
    ["jojocodemax", "JO"],
    ["智谱", "智谱"],
    ["x", "X"],
    ["  ", "供"],
    ["!!!", "供"],
  ];
  for (const [name, expected] of cases) assert.equal(providerAvatarText(name), expected, name);
});
