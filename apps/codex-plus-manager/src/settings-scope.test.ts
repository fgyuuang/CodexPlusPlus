import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import ts from "typescript";
import { appDeclarationSource, readAppSource } from "./app-source.test-helper.ts";

const text = readAppSource();
const app = ts.createSourceFile("App.tsx", text, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
// 侧栏路由表在 routes.ts：全局导航那几张表从 globalNavigationRoutes 排到设置模式的路由表之前。
const routesSource = readFileSync(new URL("./routes.ts", import.meta.url), "utf8");

function functionSource(name: string) {
  const declaration = app.statements.find(
    (statement): statement is ts.FunctionDeclaration => ts.isFunctionDeclaration(statement) && statement.name?.text === name,
  );
  assert.ok(declaration, `${name} should exist`);
  return declaration.getText(app).replace(/^export /, "");
}

test("整体设置只放应用级选项，Agent 设置承担工具级选项", () => {
  const global = functionSource("GlobalSettingsScreen");
  const agent = functionSource("AgentSettingsScreen");

  for (const copy of ["界面主题", "配置备份与迁移", "配置文件路径"]) {
    assert.ok(global.includes(`t("${copy}")`), `整体设置应包含 ${copy}`);
  }
  assert.doesNotMatch(global, /Codex 供应商设置|Codex 启动参数/);
  for (const action of ["actions.setThemePreference", "actions.exportSettings", "actions.importSettings"]) {
    assert.ok(global.includes(action), `整体设置应保留 ${action}`);
    assert.ok(!agent.includes(action), `Agent 设置不应包含 ${action}`);
  }

  for (const copy of ["Codex 启动参数"]) {
    assert.ok(agent.includes(`t("${copy}")`), `Agent 设置应包含 ${copy}`);
  }
  // 测试模型已删除：「测试供应商」直接用供应商自己的默认模型。
  assert.doesNotMatch(agent, /供应商测试模型|relayTestModel/);
});

test("Codex 设置与整体设置都进齿轮的设置模式，不占 Agent 的页签", () => {
  const sections = text.slice(text.indexOf("const navigationSections"), text.indexOf("const globalNavigationRoutes"));
  assert.doesNotMatch(sections, /"agentSettings"/);
  assert.match(text, /const settingsModeRoutes = new Set<Route>\(\["settings", "agentSettings",/);

  const globalNavigation = routesSource.slice(routesSource.indexOf("const globalNavigationRoutes"), routesSource.indexOf("const settingsModeRoutes"));
  assert.match(globalNavigation, /"settings"/);
  assert.doesNotMatch(globalNavigation, /"agentSettings"/);
});

test("设置菜单只属于 Codex，Claude / Grok 不再显示只有跳转按钮的空设置页", () => {
  assert.match(text, /\{ id: "agentSettings",[^\n]*tool: "codex" \}/);
  const agent = functionSource("AgentSettingsScreen");
  assert.doesNotMatch(agent, /没有独立设置项|前往 Grok 配置|前往 Claude 增强/);
});

test("Computer Use 放在设置模式的 Beta 分组，不再出现在每个 Agent 的菜单里", () => {
  const sections = text.slice(text.indexOf("const navigationSections"), text.indexOf("const globalNavigationRoutes"));
  assert.doesNotMatch(sections, /"computerUse"/);

  const globalNavigation = routesSource.slice(routesSource.indexOf("const globalNavigationRoutes"), routesSource.indexOf("const settingsModeRoutes"));
  assert.match(globalNavigation, /"computerUse"/);
  // Computer Use、Agent 协作、缓存清理都在设置模式里，⌘K 经设置分类搜到；本地服务的几页在 ⌘K 里自己一组。
  assert.match(functionSource("App"), /toolNavigationRoutes\.filter\(\(routeId\) => !settingsEntries\.some\(\(entry\) => entry\.route === routeId\)\)/);
  assert.match(globalNavigation, /const gatewayNavigationRoutes: Route\[\] = \["gatewayProviders", "gateway", "gatewayRouting",/);
  const entries = appDeclarationSource("settingsNavEntries");
  assert.match(entries, /\{ key: "computerUse", label: "Computer Use", icon: MousePointerClick, route: "computerUse", group: "beta"/);
  // 设置分类只有 Codex 一项看 Codex 可不可用，其余始终显示。
  assert.match(entries, /entry\.key !== "codex" \|\| codexAvailable/);
});

test("标题：设置模式用分类名，Agent 自己的页面和其余页面都用页面名", () => {
  const entry = functionSource("App");
  assert.match(entry, /settingsEntryKey\r?\n\s*\? settingsEntries\.find\(\(entry\) => entry\.key === settingsEntryKey\)\?\.label/);
  // Agent 自己的页面：Agent 名字在侧栏，标题直接是页面名（概览、内置拓展…）。
  assert.match(entry, /onAgentPage\r?\n\s*\? routeTitle\(route\)/);
  assert.match(entry, /<h1 title=\{[^\n]*\}>\{pageTitle\}<\/h1>/);
  assert.match(entry, /const activeToolName = toolEntries\.find/);
});

test("保留使用模式：侧栏、页签和命令面板共享路由约束，增强模式先确认才关闭直连配置写入", () => {
  const sidebar = functionSource("AppSidebar");
  assert.match(sidebar, /general\.filter\(\(item\) => routeVisible\(focus, item\.route\)\)/);
  assert.match(sidebar, /\.flatMap\(\(entry\) => visibleRoutes\(focus, entry\.routes\)\)/);
  assert.match(text, /commitAgentFocusSelection/);
  assert.match(text, /relayProfilesEnabled: false/);
  const modes = readFileSync(new URL("./agent-focus-model.ts", import.meta.url), "utf8");
  assert.match(modes, /if \(mode === "enhance" && !\(await actions\.disableRelaySwitching\(\)\)\) return false;/);
});
