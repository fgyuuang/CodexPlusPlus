import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import vm from "node:vm";
import ts from "typescript";

const appText = readFileSync(new URL("./App.tsx", import.meta.url), "utf8");
const app = ts.createSourceFile("App.tsx", appText, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const names = ["RelayScreen", "LocalOfficialAccountsScreen"];
const functions = app.statements.filter((node): node is ts.FunctionDeclaration =>
  ts.isFunctionDeclaration(node) && names.includes(node.name?.text ?? ""));
const source = functions.map((node) => node.getText(app)).join("\n");
const code = ts.transpileModule(source, {
  compilerOptions: { jsx: ts.JsxEmit.React, jsxFactory: "jsx", target: ts.ScriptTarget.ES2022 },
}).outputText;

type Element = { type: string; props: Record<string, any>; children: any[] };
function elements(value: any): Element[] {
  if (!value || typeof value !== "object") return [];
  if (Array.isArray(value)) return value.flatMap(elements);
  if (!value.type || !value.props) return [];
  return [value, ...elements(value.children), ...elements(value.props?.actions)];
}
function harness() {
  const calls: string[] = [], saved: any[] = [];
  const globals: Record<string, any> = {
    jsx: (type: string, props: any, ...children: any[]) => ({ type, props: props ?? {}, children }),
    React: { Fragment: "fragment" },
    useState: (initial: any) => [typeof initial === "function" ? initial() : initial, () => {}],
    useRef: (current: any) => ({ current }), useEffect: () => {},
    normalizeSettings: (settings: any) => settings,
    relayProviderView: (profile: any) => profile.view ?? "direct",
    useCodexPoolLink: () => ({ pending: [], importing: null }),
    t: (text: string) => text,
    tf: (text: string, args: unknown[]) => text.replace(/\{(\d+)\}/g, (_, index) => String(args[index])),
    CODEX_GATEWAY_RELAY_ID: "codex-plus-gateway", DIRECT_POOL_NUDGE_KEY: "fixture",
  };
  // 组件在此只构造节点；交互回调使用实际源码。
  function visit(node: ts.Node) {
    if ((ts.isJsxOpeningElement(node) || ts.isJsxSelfClosingElement(node)) && ts.isIdentifier(node.tagName)) {
      globals[node.tagName.text] ??= node.tagName.text;
    }
    ts.forEachChild(node, visit);
  }
  functions.forEach(visit);
  const context = vm.createContext(globals);
  vm.runInContext(code, context);
  const form = {
    activeRelayId: "codex-plus-gateway", relayProfilesEnabled: true, ccsDbPath: "",
    relayProfiles: [{ id: "codex-plus-gateway" }, { id: "legacy-api", view: "direct" }],
  };
  const actions = {
    relaySwitching: false, refreshRelayFiles: () => calls.push("files"),
    saveSettingsValue: async (settings: any) => { saved.push(settings); return settings; },
  };
  return { context, calls, saved, form, actions };
}

test("网关连接页保留登录和供应商池跳转，打开页面不再查询本地服务", () => {
  const h = harness();
  const tree = h.context.RelayScreen({ form: h.form, actions: h.actions, onFormChange() {},
    connection: "gateway-connection-control", onOpenGateway: () => h.calls.push("pool") });
  const nodes = elements(tree);
  assert.equal(tree.props.className, "mg-page relay-management-page cx-page cx-codex-connection-page");
  assert.ok(nodes.some((node) => node.type === "NativeAuthControl"));
  assert.ok(nodes.some((node) => node.type === "CxEmpty" && node.props.title === "Codex 正由 Codex++ 网关接管"));
  assert.ok(!nodes.some((node) => node.type === "RelayProfileList"));
  assert.ok(!nodes.some((node) => node.props.className?.includes("relay-section-tabs")));
  assert.deepEqual(h.calls, []);
  nodes.find((node) => node.type === "button" && node.children.includes("打开供应商池"))!.props.onClick();
  assert.deepEqual(h.calls, ["pool"]);
  assert.ok(!/CliproxyPanel|NewapiPanel|本地服务旧版兼容入口/.test(appText));
  assert.ok(!/call<[^>]+>\("(?:cliproxy_|newapi_)/.test(appText));
});

test("直连供应商和高级开关仍使用完整设置，关闭配置切换不丢失网关资料", async () => {
  const h = harness();
  const direct = { ...h.form, activeRelayId: "legacy-api" };
  const tree = h.context.RelayScreen({ form: direct, actions: h.actions, onFormChange() {} });
  const nodes = elements(tree), list = nodes.find((node) => node.type === "RelayProfileList")!;
  assert.equal(list.props.form, direct);
  assert.equal(list.props.form.relayProfiles.length, 2);
  assert.ok(nodes.some((node) => node.type === "NativeAuthControl"));
  const toggle = nodes.find((node) => node.type === "input" && node.props["aria-label"] === "启用供应商配置切换")!;
  toggle.props.onChange({ currentTarget: { checked: false } });
  await Promise.resolve();
  assert.equal(h.saved[0].relayProfiles, direct.relayProfiles);
  assert.equal(h.saved[0].activeRelayId, "legacy-api");
  assert.equal(h.saved[0].relayProfilesEnabled, false);
});

test("官方账号库独立管理和返回入口可用，仍由原设置保存动作处理", async () => {
  const h = harness(), changes: any[] = [];
  const result = { accounts: [{ id: "account-reference" }] };
  const tree = h.context.LocalOfficialAccountsScreen({ result, form: h.form, actions: h.actions,
    onFormChange: (next: any) => changes.push(next), onBack: () => h.calls.push("back") });
  const nodes = elements(tree), panel = nodes.find((node) => node.type === "OfficialAccountsPanel")!;
  assert.equal(panel.props.result, result);
  assert.equal(panel.props.actions, h.actions);
  await panel.props.onSettingsChange(h.form);
  assert.equal(changes[0], h.form);
  assert.equal(h.saved[0], h.form);
  nodes.find((node) => node.type === "button" && node.children.includes("返回供应商池"))!.props.onClick();
  assert.deepEqual(h.calls, ["back"]);
  assert.match(appText, /if \(navigation.section === "official"\) await refreshOfficialAccounts\(true\)/);
});

test("供应商池仅在存在本地官方账号引用时提供管理入口", () => {
  const text = readFileSync(new URL("./GatewayPage.tsx", import.meta.url), "utf8");
  const file = ts.createSourceFile("GatewayPage.tsx", text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
  const component = file.statements.find((node) => ts.isFunctionDeclaration(node) && node.name?.text === "GatewayPage")!;
  const js = ts.transpileModule(component.getText(file).replace("export function", "function"), {
    compilerOptions: { jsx: ts.JsxEmit.React, jsxFactory: "jsx", target: ts.ScriptTarget.ES2022 },
  }).outputText;
  const h = harness();
  const globals = h.context;
  const memo = { payload: { pool: [] as any[], agents: [], helperRunning: true }, tab: "providers" };
  Object.assign(globals, { memo, makeRun: () => () => {}, GatewayNavContext: { Provider: "Provider" },
    TopbarActions: "TopbarActions", RefreshCw: "RefreshCw", ProvidersView: "ProvidersView", UsageView: "UsageView" });
  vm.runInContext(js, globals);
  const render = (tab: string, callback: any = () => h.calls.push("library")) => elements(globals.GatewayPage({
    tab, notify() {}, onOpenLocalOfficialAccounts: callback,
  })).find((node) => node.type === "button" && node.children.includes("本地官方账号库"));
  memo.payload.pool = [{ source: undefined }, { source: "api-provider" }];
  assert.equal(render("providers"), undefined);
  memo.payload.pool.push({ source: "official-account:existing-reference" });
  render("providers")!.props.onClick();
  assert.deepEqual(h.calls, ["library"]);
  assert.equal(render("usage"), undefined);
  assert.equal(render("providers", null), undefined);
});
