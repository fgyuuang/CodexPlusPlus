import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readFile } from "node:fs/promises";

function providerSwitchHelperSource(renderer: string) {
  const start = renderer.indexOf("  async function switchCodexOfficialThreadProvider(");
  return start < 0 ? "" : renderer.slice(start, renderer.indexOf("  function patchCodexMcpRequestTransport(", start));
}

const STEPWISE_FRAGMENT_PATHS = [
  "floating-panel/runtime/state.js",
  "floating-panel/core/appearance-runtime.js",
  "floating-panel/runtime/dom.js",
  "floating-panel/runtime/bridge-client.js",
  "floating-panel/runtime/answer-context.js",
  "floating-panel/stepwise/suggestions.js",
  "floating-panel/stepwise/generation.js",
  "floating-panel/runtime/lifecycle.js",
  "floating-panel/runtime/settings.js",
  "floating-panel/core/appearance.js",
  "floating-panel/core/host.js",
  "floating-panel/core/geometry.js",
  "floating-panel/core/interaction.js",
  "floating-panel/core/views.js",
  "floating-panel/outline/parser.js",
  "floating-panel/outline/navigation.js",
  "floating-panel/outline/feature.js",
  "floating-panel/outline/view.js",
  "floating-panel/core/scroll-state.js",
  "floating-panel-inject.js",
].map((name) => new URL(`../../../assets/inject/${name}`, import.meta.url));

async function readStepwiseSource() {
  const fragments = await Promise.all(STEPWISE_FRAGMENT_PATHS.map((url) => readFile(url, "utf8")));
  return `(() => {\n${fragments.join("\n")}\n})();\n`;
}

it("only reveals floating-panel content after the shell has settled open", async () => {
  const source = await readFile(
    new URL("../../../assets/inject/floating-panel/core/appearance.js", import.meta.url),
    "utf8",
  );
  const panelRules = Array.from(
    source.replace(/\$\{[^}]+\}/g, "0").matchAll(/(?:^|\n)\s*([^{}\n]*\.csw-panel)\s*\{([^}]+)\}/g),
    ([, selector, declarations]) => ({ selector: selector.trim(), declarations }),
  );
  const hidden = panelRules.find((rule) => rule.selector === ".csw-panel");
  assert.ok(hidden, "panel needs a hidden default for collapsed and interrupted states");
  assert.match(hidden.declarations, /opacity:\s*0\s*;/);
  assert.match(hidden.declarations, /visibility:\s*hidden\s*;/);
  assert.match(hidden.declarations, /transition:\s*none\s*!important\s*;/);

  const visible = panelRules.filter((rule) =>
    /opacity:\s*1\s*;|visibility:\s*visible\s*;/.test(rule.declarations),
  );
  assert.ok(visible.length > 0, "settled content must remain visible");
  for (const rule of visible) {
    assert.ok(rule.selector.includes('[data-open="true"]'), rule.selector);
    assert.ok(rule.selector.includes('[data-morphing="false"]'), rule.selector);
  }
});

type UsageLimitNodeInit = {
  text?: string;
  position?: string;
  dismissLabel?: string;
  inert?: boolean;
  pointerEvents?: string;
};

class UsageLimitNode {
  attributes = new Map<string, string>();
  children: UsageLimitNode[] = [];
  parentElement: UsageLimitNode | null = null;
  style: Record<string, string> = {};
  innerText: string;
  clickCount = 0;
  disabled = false;
  readonly positionValue: string;
  private readonly dismissLabel: string;

  constructor(init: UsageLimitNodeInit = {}) {
    this.innerText = init.text ?? "";
    this.positionValue = init.position ?? "";
    this.dismissLabel = init.dismissLabel ?? "";
    if (init.inert) this.attributes.set("inert", "");
    if (init.pointerEvents) this.style.pointerEvents = init.pointerEvents;
  }

  get textContent() {
    return this.innerText;
  }

  appendChild(child: UsageLimitNode) {
    child.parentElement = this;
    this.children.push(child);
    return child;
  }

  closest() {
    return null;
  }

  getAttribute(name: string) {
    if (name === "aria-label") return this.dismissLabel || null;
    return this.attributes.get(name) ?? null;
  }

  setAttribute(name: string, value: string) {
    this.attributes.set(name, value);
  }

  hasAttribute(name: string) {
    return this.attributes.has(name);
  }

  removeAttribute(name: string) {
    this.attributes.delete(name);
  }

  click() {
    this.clickCount += 1;
  }

  querySelectorAll(selector: string) {
    if (selector.startsWith("button")) return this.children.filter((child) => child.dismissLabel);
    return [];
  }
}

type UsageLimitUnblockRuntime = {
  refreshCodexUsageLimitUnblock: () => void;
  refreshCodexUsageLimitComposerUnblock: () => void;
};

function usageLimitUnblockRuntime(
  renderer: string,
  dialogs: UsageLimitNode[],
  composer: UsageLimitNode | null,
  sendButton: UsageLimitNode | null,
) {
  const start = renderer.indexOf("  const codexUsageLimitDialogAttribute = ");
  const end = renderer.indexOf("\n  let zedRemoteStatusPromise", start);
  assert.ok(start >= 0 && end > start, "usage limit unblock block not found");
  const source = renderer.slice(start, end);
  const diagnostics: Array<{ event: string; detail: Record<string, unknown> }> = [];
  const body = new UsageLimitNode();
  const documentElement = new UsageLimitNode();
  const documentValue = {
    body,
    documentElement,
    querySelectorAll(selector: string) {
      return selector.includes("aria-modal") ? dialogs : [];
    },
  };
  const windowValue: Record<string, unknown> = {
    getComputedStyle: (node: UsageLimitNode) => ({ position: node.positionValue }),
  };
  const runtimeState = { active: new Set<string>(), pending: new Set<string>() };
  const create = new Function(
    "window",
    "document",
    "HTMLElement",
    "sendCodexPlusDiagnostic",
    "visibleElement",
    "codexQuotaResumeTextHasMarker",
    "codexServiceTierFindComposerEl",
    "codexQuotaResumeNativeSendButton",
    "codexQuotaResumeEditorIsEmpty",
    "validThreadScrollSessionKey",
    "currentSessionRef",
    "codexQuotaResumeRuntime",
    "codexPlusSettings",
    `${source}\nreturn { refreshCodexUsageLimitUnblock, refreshCodexUsageLimitComposerUnblock };`,
  ) as (
    windowValue: Record<string, unknown>,
    documentValue: unknown,
    elementType: unknown,
    diagnostic: (event: string, detail: Record<string, unknown>) => void,
    visible: () => boolean,
    hasMarker: (text: string) => boolean,
    findComposer: () => UsageLimitNode | null,
    findSendButton: () => UsageLimitNode | null,
    editorIsEmpty: () => boolean,
    normalizeThreadId: (value: string) => string,
    currentSession: () => { session_id: string },
    quotaRuntime: () => typeof runtimeState,
    settings: () => { quotaResume: boolean },
  ) => UsageLimitUnblockRuntime;
  const runtime = create(
    windowValue,
    documentValue,
    UsageLimitNode,
    (event, detail) => diagnostics.push({ event, detail }),
    () => true,
    (text: string) => /usage\s*limit|quota|insufficient|额度|配额|限流/.test(String(text).toLowerCase()),
    () => composer,
    () => sendButton,
    () => false,
    (value: string) => value,
    () => ({ session_id: "thread-1" }),
    () => runtimeState,
    () => ({ quotaResume: true }),
  );
  return { runtime, diagnostics, body, runtimeState };
}

function codexAppToolsProbeRuntime(renderer: string) {
  const start = renderer.indexOf("  const codexAppToolsProbeSchemaVersion = ");
  const end = renderer.indexOf("\n  installCodexAppToolsProbe();", start);
  assert.ok(start >= 0 && end > start, "codex_app probe block not found");
  const source = renderer.slice(start, end);
  const diagnostics: Array<{ event: string; detail: Record<string, unknown> }> = [];
  const factory = new Function(
    "window",
    "sendCodexPlusDiagnostic",
    `${source}\nreturn codexAppToolsProbeEvent;`,
  ) as (
    windowValue: Record<string, unknown>,
    diagnostic: (event: string, detail: Record<string, unknown>) => void,
  ) => (direction: string, value: unknown) => void;
  const event = factory({}, (name, detail) => diagnostics.push({ event: name, detail }));
  return { diagnostics, event };
}

function installRendererStyle(renderer: string) {
  const start = renderer.indexOf("  function installStyle()");
  const end = renderer.indexOf("\n  function defaultCodexPlusSettings", start);
  assert.ok(start >= 0 && end > start);
  const source = renderer.slice(start, end);
  const requiredNames = new Set([
    "styleId",
    "codexDeleteStyleVersion",
    ...Array.from(source.matchAll(/\$\{([A-Za-z_$][A-Za-z0-9_$]*)/g), (match) => match[1]),
  ]);
  const declarations = Array.from(requiredNames, (name) => {
    const declaration = renderer.match(new RegExp(`^  const ${name} = .+;$`, "m"))
      ?? renderer.match(new RegExp(`^  const ${name} = [\\s\\S]*?^  };$`, "m"));
    assert.ok(declaration, `missing renderer declaration for ${name}`);
    return declaration[0];
  }).join("\n");
  const appended: Array<{ dataset: Record<string, string>; id?: string; textContent?: string }> = [];
  const document = {
    getElementById() {
      return null;
    },
    createElement() {
      return { dataset: {} };
    },
    documentElement: {
      appendChild(node: (typeof appended)[number]) {
        appended.push(node);
      },
    },
  };
  const install = new Function("document", `${declarations}\n${source}\ninstallStyle();`) as (documentValue: typeof document) => void;

  install(document);
  return appended;
}

describe("renderer injection header compatibility", () => {
  it("keeps source labels and route identities separate from the capability template", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function codexPlusModelDisplayName(");
    const end = renderer.indexOf("\n  function modelArrayLooksPatchable", start);
    assert.ok(start >= 0 && end > start);
    const routes = new Map([
      ["codex_plus_official_account_2:gpt-5.4", {
        displayName: "gpt-5.4(官方账号 2)", displaySuffix: "（模板）",
        capabilitySlug: "gpt-5.4", sourceKind: "directOfficial", providerId: "codex_plus_official_account_2", priority: 1000,
      }],
      ["gpt-5.4(NEW|ECNU:ecnu-reasoner)", {
        displayName: "gpt-5.4(NEW|ECNU:ecnu-reasoner)", displaySuffix: "（模板）",
        capabilitySlug: "gpt-5.4", sourceKind: "aggregate", providerId: "codex_plus_aggregate_new",
      }],
      ["CLIProxyAPI:qwen3", {
        displayName: "CLIProxyAPI:qwen3", displaySuffix: "（模板）",
        capabilitySlug: "gpt-5.4", sourceKind: "cliGeneral", providerId: "codex_plus_cli_general",
      }],
      ["CLIProxyAPI:gpt-5.4", {
        displayName: "CLIProxyAPI:gpt-5.4", displaySuffix: "（模板）",
        capabilitySlug: "gpt-5.4", sourceKind: "cliOfficial", providerId: "codex_plus_cli_official", priority: 1001,
      }],
    ]);
    const create = new Function(
      "codexPlusModelMetadata", "modelReasoningEfforts", "codexModelCatalog",
      `${renderer.slice(start, end)}\nreturn { applyCodexPlusModelMetadata, codexPlusModelDescriptor };`,
    ) as (
      metadata: (model: string) => Record<string, string | number | undefined> | undefined,
      efforts: () => Array<{ reasoningEffort: string }>,
      catalog: Record<string, string>,
    ) => {
      applyCodexPlusModelMetadata: (descriptor: Record<string, unknown>, model: string) => boolean;
      codexPlusModelDescriptor: (model: string, available: Array<Record<string, unknown>>) => Record<string, unknown>;
    };
    const runtime = create((model) => routes.get(model), () => [{ reasoningEffort: "high" }], {});
    const officialTemplate = { model: "gpt-5.4", displayName: "GPT-5.4", contextWindow: 272000 };
    for (const [routingSlug, metadata] of routes) {
      const descriptor = runtime.codexPlusModelDescriptor(routingSlug, [officialTemplate]);
      assert.equal(descriptor.model, routingSlug);
      assert.equal(descriptor.slug, routingSlug);
      assert.equal(descriptor.displayName, metadata.displayName);
      assert.equal(descriptor.modelProvider, metadata.providerId);
      assert.equal(descriptor.contextWindow, 272000);
      if (metadata.priority !== undefined) assert.equal(descriptor.priority, metadata.priority);

      const existing: Record<string, unknown> = { model: routingSlug, displayName: "GPT-5.4", modelProvider: "openai" };
      assert.equal(runtime.applyCodexPlusModelMetadata(existing, routingSlug), true);
      assert.equal(existing.model, routingSlug);
      assert.equal(existing.displayName, metadata.displayName);
      assert.equal(existing.modelProvider, metadata.providerId);
      if (metadata.priority !== undefined) assert.equal(existing.priority, metadata.priority);
    }
  });

  it("patches current model/list MCP responses without a writable Electron bridge", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function patchMcpModelResponseData(data) {");
    const end = renderer.indexOf("\n  function appServerModelRequestMethod", start);
    assert.ok(start >= 0 && end > start);
    const patch = new Function(
      "codexPlusModelUnlockEnabled", "codexPlusModelListRequestIds", "patchModelArray",
      `${renderer.slice(start, end)}\nreturn patchMcpModelResponseData;`,
    ) as (enabled: () => boolean, ids: Set<string>, patchArray: (models: unknown[], allowEmpty: boolean) => boolean) => (data: unknown) => boolean;
    const ids = new Set<string>();
    const patchArray = (models: unknown[] | undefined, allowEmpty: boolean) => {
      assert.equal(allowEmpty, true);
      if (!Array.isArray(models)) return false;
      models.push({ model: "CLIProxyAPI:deepseek-chat" });
      return true;
    };
    const handle = patch(() => true, ids, patchArray);
    const response = { type: "mcp-response", requestMethod: "model/list", message: { id: 17, result: { data: [{ model: "gpt-5.4" }] } } };
    assert.equal(handle(response), true);
    assert.equal(response.message.result.data.length, 2);
    assert.equal(handle({ type: "mcp-response", requestMethod: "thread/list", message: { id: 18, result: { data: [] } } }), false);
  });

  it("patches message data before an already registered Codex listener reads it", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function patchAppServerModelMessages() {");
    const end = renderer.indexOf("\n  function patchMcpModelResponseData", start);
    assert.ok(start >= 0 && end > start);
    class FakeMessageEvent {
      private readonly payload: { patched?: boolean };
      constructor(payload: { patched?: boolean }) { this.payload = payload; }
      get data() { return this.payload; }
    }
    const fakeWindow = { addEventListener() {}, __codexPlusModelMessagePatchInstalled: false };
    const install = new Function(
      "window", "MessageEvent", "patchMcpModelResponseData",
      `${renderer.slice(start, end)}\nreturn patchAppServerModelMessages;`,
    ) as (windowValue: typeof fakeWindow, eventType: typeof FakeMessageEvent, patch: (data: { patched?: boolean }) => void) => () => void;
    install(fakeWindow, FakeMessageEvent, (data) => { data.patched = true; })();
    const event = new FakeMessageEvent({});
    assert.equal(event.data.patched, true);
  });

  it("纯 API 会话使用当前真实 provider，不强行改成 custom", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.doesNotMatch(
      renderer,
      /if \(String\(profile\?\.relayMode \|\| ""\) === "pureApi"\) return "custom";/,
    );
    assert.match(renderer, /codexPlusBackendSettings\.activeRelayCodexProvider/);
    assert.match(renderer, /codexModelCatalog\?\.codex_model_provider/);
  });

  it("裸 JSON-RPC 形态的 turn/start 也要被套上第三方 provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    // 新版桌面端把 app-server 请求以 { method, params } 派发，不带 send-cli-request-for-host
    // / start-turn-for-host 这类 type。缺少兜底分支时 provider 覆盖永不生效，
    // 第三方模型会留在内置 openai provider 上并被 ChatGPT 账号侧拒绝。
    assert.match(renderer, /String\(message\.method \|\| ""\)\.trim\(\)/);
    assert.match(renderer, /codexRemoteSessionProviderRequestMethod\(directMethod\)/);
    // provider 覆盖不能只挂在“模型白名单解锁”这一个开关上。
    assert.match(renderer, /codexPlusModelUnlockEnabled\(\) \|\| codexRemoteSessionProviderOverrideEnabled\(\)/);
  });

  it("adds the session copy shortcut through the native fork action", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /原地复制会话 - Codex\+\+/);
    assert.match(renderer, /createSessionMoreMenuItem\("原地复制会话 - Codex\+\+"/);
    assert.match(renderer, /getAttribute\("aria-label"\)[\s\S]*聊天操作/);
    assert.match(renderer, /从这里创建聊天分支/);
    assert.match(renderer, /data-app-action-sidebar-thread-selected/);
    assert.match(renderer, /sessionCopyMenuActivationTimeoutMs/);
    assert.doesNotMatch(renderer, /\n\s*refreshSessionCopyMenuItems\(\);/);
  });

  it("adds an encrypted session sharing button to the active Codex conversation", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /sessionShareButtonClass\s*=\s*"codex-session-share-button"/);
    assert.match(renderer, /function installSessionShareButton\(\)/);
    assert.match(renderer, /function sessionShareMarkdown\(\)/);
    assert.match(renderer, /crypto\.subtle\.generateKey\(\{ name: "AES-GCM", length: 256 \}/);
    assert.match(renderer, /https:\/\/share\.codexpp\.cc/);
    assert.match(renderer, /postJson\("\/share\/create", payload\)/);
    assert.match(renderer, /postJson\("\/session\/export"/);
    assert.match(renderer, /postJson\("\/session\/import"/);
    assert.match(renderer, /codex-rollout/);
    assert.match(renderer, /function sessionImportMarkdown\(session\)/);
    assert.match(renderer, /codexpp-import-session/);
    assert.match(renderer, /nativeShare\?\.closest\?\.\("\.ms-auto"\)/);
    assert.match(renderer, /#k=\$\{encrypted\.key\}/);
    assert.match(renderer, /navigator\.clipboard\.writeText\(shareUrl\)/);
    assert.match(renderer, /data-testid\*=\"message\"/);
    assert.match(renderer, /function sessionActionTrigger\(row\)/);
    assert.match(renderer, /const sessionMenuEnabled = codexPlusBackendSettings\.enhancementsEnabled !== false/);
    assert.doesNotMatch(renderer, /window\.location\.(?:href|assign)\s*=\s*[^;]*markdown/);
  });

  it("automatically renames a session through the native title suggestion", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /自动重命名当前会话/);
    assert.match(renderer, /activateSessionAutoRenameMenuItem/);
    assert.match(renderer, /input\[aria-label="聊天标题"\], input\[aria-label="Chat title"\]/);
    assert.match(renderer, /button\.classList\.contains\("text-info"\)/);
    assert.match(renderer, /\^\(保存\|Save\)\$/);
    assert.match(renderer, /Codex 未能生成新名称/);
  });

  it("removes the legacy Codex++ top-bar entry", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.doesNotMatch(renderer, /function installCodexPlusMenu\(\)/);
    assert.doesNotMatch(renderer, /function findNativeMenuInsertionPoint\(\)/);
    assert.doesNotMatch(renderer, /codex-plus-trigger/);
  });

  it("places Codex++ in the native sidebar and opens a main-content page", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /codexPlusSidebarNavId\s*=\s*"codex-plus-sidebar-nav"/);
    assert.match(renderer, /function installCodexPlusSidebarNavigation\(\)/);
    assert.match(renderer, /aside\.app-shell-left-panel nav\[role="navigation"\]/);
    // 新版把 role="navigation" 挪去了缩略图面板/演示目录，兜底要限定在 aside 内；
    // 而图标栏本身也是 aside 里的 <nav> 且文档顺序在前，必须显式排除，
    // 否则 "点导航就关页面" 的监听会挂到图标栏上，点自己的入口就把页面关掉。
    assert.doesNotMatch(renderer, /aside\.app-shell-left-panel nav\[role="navigation"\], nav\[role="navigation"\]/);
    assert.match(renderer, /aside\.app-shell-left-panel nav\[role="navigation"\]'\)/);
    assert.match(renderer, /!nav\.hasAttribute\("data-app-navigation-rail"\)/);
    assert.match(renderer, /const insertionButton = pluginButton \|\| navButtons\.find/);
    assert.match(renderer, /selectors\.pluginNavButton/);
    assert.match(renderer, /button\.querySelector\(selectors\.pluginSvgPath\)/);
    assert.match(renderer, /\^\(插件\|Plugins\)\$/);
    assert.match(renderer, /openCodexPlusPage\(\)/);
    assert.match(renderer, /codex-plus-page-overlay/);
    assert.match(renderer, /positionCodexPlusPage/);
    assert.match(renderer, /overlay\.remove\(\);\s*if \(pageMode\) setCodexPlusSidebarNavActive\(false\);/);
    assert.match(renderer, /function closeCodexPlusPage\(\)/);
    assert.match(renderer, /function installCodexPlusPageNavigationCloseHandler\(\)/);
    assert.match(renderer, /target\?\.closest\(selectors\.sidebarThread\)/);
    assert.match(renderer, /closeCodexPlusPageAfterNativeNavigation\(\)/);
    assert.match(renderer, /setTimeout\(\(\) => \{\s*window\.__codexPlusPageNavigationCloseTimer = null;\s*closeCodexPlusPage\(\);/);
    assert.match(renderer, /installCodexPlusNavigationEntries\(\);/);
    assert.match(renderer, /document\.querySelectorAll\(`#\$\{codexPlusMenuId\}/);
  });


  it("renders the official multi-account quota monitor through safe bridge routes", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /data-codex-open-official-accounts="true"/);
    assert.match(renderer, /function renderOfficialAccountWindow\(window, fallback\)/);
    assert.match(renderer, /function renderOfficialAccount\(account\)/);
    assert.match(renderer, /\/official-accounts\/list/);
    assert.match(renderer, /\/official-accounts\/refresh/);
    assert.match(renderer, /100 - used/);
    assert.match(renderer, /请求账号/);
    assert.match(renderer, /本机登录/);
    assert.match(renderer, /\.codex-plus-official-email \{[^}]*overflow-wrap: anywhere/);
    assert.match(renderer, /openManagerFromCodex\(\{ page: "relay", section: "official" \}\)/);
    assert.doesNotMatch(renderer, /accessToken|refreshToken|authorizationHeader/);

  });

  it("mounts Codex++ and 拓展 entries into the new navigation rail", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /codexPlusRailNavId\s*=\s*"codex-plus-rail-nav"/);
    assert.match(renderer, /codexPlusRailExtensionsId\s*=\s*"codex-plus-rail-extensions"/);
    assert.match(renderer, /codexPlusRailSelector\s*=\s*"nav\[data-app-navigation-rail\]"/);
    assert.match(renderer, /function installCodexPlusRailNavigation\(\)/);
    // 模板按钮取自原生 destination，clone 后必须清掉这几个属性，
    // 否则会被 Codex 的自定义/排序逻辑当成真 destination。
    assert.match(renderer, /codexPlusRailDestinationSelector\s*=\s*"\[data-sidebar-destination\]"/);
    assert.match(renderer, /button\.removeAttribute\("data-sidebar-destination"\)/);
    assert.match(renderer, /button\.removeAttribute\("aria-current"\)/);
    // 图标栏是纯图标，不塞文字标签。
    assert.match(renderer, /class="codex-plus-rail-icon"/);
    assert.doesNotMatch(renderer, /codex-plus-rail-icon"[^]*?<span class="truncate">/);
  });

  it("falls back to the legacy sidebar entry when the rail is absent", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /function installCodexPlusNavigationEntries\(\)/);
    // 两条路径互斥：有 rail 就移除旧入口，没有就移除 rail 入口。
    assert.match(renderer, /if \(installCodexPlusRailNavigation\(\)\) \{\s*detachCodexPlusSidebarNavigation\(\);\s*return;\s*\}/);
    assert.match(renderer, /removeCodexPlusRailNavigation\(\);\s*installCodexPlusSidebarNavigation\(\);/);
    assert.match(renderer, /function detachCodexPlusSidebarNavigation\(\)/);
    // 启动补扫要认两条路径任意一条已装上。
    assert.match(renderer, /const installed = document\.getElementById\(codexPlusSidebarNavId\)\s*\|\| document\.getElementById\(codexPlusRailNavId\);/);
  });

  it("opens 拓展 as a standalone page instead of a Codex++ tab", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /codexPlusExtensionsTab\s*=\s*"extensions"/);
    assert.match(renderer, /function openCodexPlusExtensions\(\)/);
    assert.match(renderer, /openCodexPlusModal\(\{ page: true, tab: codexPlusExtensionsTab \}\)/);
    // 旧名 userScripts 仍要能映射过去，避免存量调用点失效。
    assert.match(renderer, /if \(tab === "extensions" \|\| tab === "userScripts"\) return codexPlusExtensionsTab;/);
    // 弹窗必须尊重传入的初始 tab，而不是硬编码 home。
    assert.match(renderer, /selectCodexPlusTab\(initialTab\);/);
    // 激活态要靠 data-codex-plus-active-tab 区分页面，必须排在 selectCodexPlusTab 之后。
    assert.match(renderer, /selectCodexPlusTab\(initialTab\);\s*\/\/[^]*?if \(pageMode\) setCodexPlusSidebarNavActive\(true, codexPlusActiveEntry\(\) \|\| "home"\);/);
  });

  it("gives the page mode a two-column layout with its own left panel", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /function installCodexPlusPageLayout\(overlay, tab\)/);
    assert.match(renderer, /function refreshCodexPlusPageNav\(tab\)/);
    assert.match(renderer, /function renderCodexPlusPageNavItems\(tab\)/);
    // 三栏容器是 createElement + className 赋值的，不是 markup 字面量。
    assert.match(renderer, /layout\.className = "codex-plus-page-layout"/);
    assert.match(renderer, /main\.className = "codex-plus-page-main"/);
    // 只搬动已有的 modal-body，不重建里面的 data-codex-* 挂载点，
    // 否则 renderUserScripts / 各 toggle 的 querySelector 会找不到目标。
    assert.match(renderer, /main\.appendChild\(body\)/);
    // 左面板只有「拓展」需要（它是脚本列表）；主页与推荐内容是单栏内容页，
    // 页面切换交给图标栏那三个入口，所以导航容器要在条件分支里创建。
    assert.match(renderer, /if \(tab === codexPlusExtensionsTab\) \{[\s\S]{0,400}nav\.className = "codex-plus-page-nav"/);
    assert.match(renderer, /data-codex-plus-page-nav-body="true"/);
    // 布局必须在 selectCodexPlusTab 之前建好，否则刷新左面板时找不到容器。
    assert.match(renderer, /installCodexPlusPageLayout\(overlay, initialTab\);[\s\S]{0,900}selectCodexPlusTab\(initialTab\);/);
    // 左面板分组导航走同一个选中函数。
    assert.match(renderer, /const pageNav = target\?\.closest\("\[data-codex-plus-page-nav\]"\)/);
    assert.match(renderer, /selectCodexPlusTab\(pageNav\.getAttribute\("data-codex-plus-page-nav"\)\)/);
  });

  it("侧边栏仅保留主页与拓展入口，并跟随 Codex 的界面缩放", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    // 已发布的推荐 ID 保持兼容，但不再注册侧边栏入口，并清理旧节点。
    assert.match(renderer, /const codexPlusRailSponsorId = "codex-plus-rail-sponsor"/);
    assert.match(renderer, /const codexPlusSponsorTab = "sponsor"/);
    assert.match(renderer, /function openCodexPlusSponsor\(\)/);
    assert.doesNotMatch(renderer, /id: codexPlusRailSponsorId, label: "推荐内容"/);
    assert.match(renderer, /document\.getElementById\(codexPlusRailSponsorId\)\?\.remove\(\);/);
    assert.match(renderer, /id: codexPlusRailNavId, label: "Codex\+\+"/);
    assert.match(renderer, /id: codexPlusRailExtensionsId, label: "拓展"/);
    const manager = await readFile(new URL("./App.tsx", import.meta.url), "utf8");
    assert.match(manager, /id: "recommendations", label: t\("推荐内容"\)/);
    assert.match(manager, /function RecommendationsScreen\(/);
    // 保留旧页面的选中态兼容映射。
    assert.match(renderer, /\[codexPlusRailSponsorId, "sponsor"\],/);
    assert.match(renderer, /if \(tab === codexPlusSponsorTab\) return "sponsor";/);

    // 弹窗里的 tab 条已随入口外移删除，不该再有残留。
    assert.doesNotMatch(renderer, /codex-plus-tab-button/);

    // 「拓展」入口用 VSCode 的扩展字形，和列表默认图标同一份 path。
    assert.match(renderer, /extensions: `<svg viewBox="0 0 16 16" fill="currentColor"><path d="\$\{codexPlusDefaultExtensionIconPath\}"/);

    // 界面缩放：读 Codex 的 zoom 变量，套到 overlay 上，尺寸用 calc 反向抵消。
    assert.match(renderer, /const codexPlusWindowZoomVar = "--codex-window-zoom"/);
    assert.match(renderer, /function codexPlusWindowZoom\(\)/);
    assert.match(renderer, /function applyCodexPlusZoom\(overlay\)/);
    assert.match(renderer, /overlay\.style\.setProperty\("zoom", String\(zoom\)\)/);
    assert.match(renderer, /width: calc\(100vw \/ var\(--codex-plus-zoom, 1\)\)/);
    // 页面模式的 left 偏移要按 zoom 折算回布局坐标，CSS 里的 width 用的是同一个空间，
    // 两边量纲不一致会把右边缘算短（曾出现 62 + 1652 = 1714，视口 1727）。
    assert.match(renderer, /const layoutLeft = zoom === 1 \? left : left \/ zoom;/);
    assert.match(renderer, /overlay\.style\.setProperty\("--codex-plus-page-left", `\$\{layoutLeft\}px`\)/);
  });

  it("颜色走 Codex 的语义令牌，不写死调色板", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    // applyCodexPlusTheme 读宿主算好的令牌，取不到才回落。
    assert.match(renderer, /function applyCodexPlusTheme\(overlay\)/);
    assert.match(renderer, /--color-token-text-primary/);
    assert.match(renderer, /--color-token-text-secondary/);
    assert.match(renderer, /--color-token-bg-primary/);
    // 早先写死的 zinc 调色板不能再出现在主题函数里。
    const themeBody = renderer.slice(
      renderer.indexOf("function applyCodexPlusTheme(overlay)"),
      renderer.indexOf("function codexPlusModalTab(tab)"),
    );
    assert.ok(themeBody.length > 0, "找不到 applyCodexPlusTheme 的函数体");
    assert.ok(!themeBody.includes('text: "#f3f4f6"'), "主题函数里残留硬编码的正文色");
    assert.ok(!themeBody.includes('textSecondary: "#a1a1aa"'), "主题函数里残留硬编码的次要色");
  });

  it("lists user scripts in the 拓展 page left panel", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /function renderCodexPlusExtensionsNav\(\)/);
    // 点左面板的行 = 选中（右侧看详情），不直接切开关——
    // 开关移到详情页，避免点行就误触发启停。
    assert.match(renderer, /data-codex-extensions-select="installed:\$\{escapeHtml\(entry\.key\)\}"/);
    assert.match(renderer, /data-codex-extensions-select="market:\$\{escapeHtml\(entry\.key\)\}"/);
    assert.match(renderer, /codex-plus-page-nav-item-state/);
    // 每个条目都有图标，没有自带图标的用默认字形。
    assert.match(renderer, /codex-plus-page-nav-item-icon/);
    assert.match(renderer, /codexPlusDefaultExtensionIconPath/);
    // 脚本启停/状态变化要同步到左面板与详情。
    assert.match(renderer, /if \(codexPlusActiveEntry\(\) === "extensions"\) refreshCodexPlusExtensionsView\(\);/);
  });

  it("shows a VSCode-style detail pane for the selected 拓展", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /function renderCodexPlusExtensionsDetail\(\)/);
    assert.match(renderer, /data-codex-plus-extensions-detail="true"/);
    assert.match(renderer, /function codexPlusExtensionsSelectionDetail\(\)/);
    // 详情里给已安装的开关（复用 user-script-key 委托）与卸载入口。
    assert.match(renderer, /data-codex-extensions-uninstall=/);
    assert.match(renderer, /function uninstallUserScript\(key\)/);
    assert.match(renderer, /postJson\("\/user-scripts\/delete", \{ key \}\)/);
    // 内置脚本在只读目录里，只给用户脚本提供卸载。
    assert.match(renderer, /if \(local\?\.source === "user"\)/);
    // 市场条目在详情里也能直接安装。
    assert.match(renderer, /codex-plus-extensions-detail-primary/);
    // 选中项要高亮。
    assert.match(renderer, /data-active="\$\{String\(selected\)\}"/);
  });

  it("gives 拓展 a searchable 已安装 / 市场 分组视图", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /data-codex-extensions-search="true"/);
    assert.match(renderer, /function refreshCodexPlusExtensionsView\(\)/);
    assert.match(renderer, /function codexPlusExtensionsEntries\(\)/);
    assert.match(renderer, /function loadScriptMarket\(/);
    // 市场走 bridge 的两个新路由。
    assert.match(renderer, /postJson\("\/script-market\/list", \{\}\)/);
    assert.match(renderer, /postJson\("\/script-market\/install", \{ id \}\)/);
    // 空分组要留着显示占位（加载中/加载失败），只在搜索无匹配时才省略——
    // 否则「正在读取脚本市场…」和错误提示会被一起藏掉，面板全空。
    assert.match(renderer, /if \(!entries\.length && searching\) return "";/);
    assert.doesNotMatch(renderer, /if \(!entries\.length && \(searching \|\| !count\)\) return "";/);
    // 搜索重绘后要把焦点放回输入框，否则每敲一个字就断。
    assert.match(renderer, /next\.setSelectionRange\(next\.value\.length, next\.value\.length\)/);

  });

  it("does not install Codex++ UI in embedded browser documents", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /window\.top\s*!==\s*window/);
    assert.match(renderer, /!window\.electronBridge/);
    assert.ok(renderer.includes("/^app:\\\/\\\/\\-\\//i.test(window.location.href)"));
    assert.match(renderer, /codexPlusIsNodeTestHarness/);
  });

  it("initializes renderer styles without unresolved template identifiers", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    const appended = installRendererStyle(renderer);

    assert.equal(appended.length, 1);
    assert.match(appended[0].textContent ?? "", /#codex-plus-sidebar-nav/);
  });

  it("does not override the host document root typography or foreground", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const appended = installRendererStyle(renderer);
    const css = appended[0].textContent ?? "";
    const rootRule = css.match(/:root\s*\{([^}]*)\}/)?.[1] ?? "";

    assert.doesNotMatch(rootRule, /(?:^|;)\s*font(?:-family)?\s*:/);
    assert.doesNotMatch(rootRule, /(?:^|;)\s*color\s*:/);
    assert.match(css, /:where\([^)]*codex-plus-modal-overlay[^)]*\)\s*\{[^}]*font-family:\s*inherit;/s);
  });

  it("rewrites official usage status at the cache boundary instead of scanning alerts", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const scanStart = renderer.indexOf("  function scanLightweight()");
    const scanEnd = renderer.indexOf("  function officialUsagePolicy()", scanStart);
    assert.ok(scanStart >= 0 && scanEnd > scanStart);
    const scan = renderer.slice(scanStart, scanEnd);

    assert.doesNotMatch(scan, /syncOfficialUsagePolicy|refreshOfficialUsageAlert/);
    assert.match(renderer, /typeof nextStatus\.hideOfficialUsageAlert === "boolean"/);
    assert.match(renderer, /window\.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = nextStatus\.hideOfficialUsageAlert/);
    assert.match(renderer, /function syncOfficialUsagePolicy\(\)/);
    assert.match(renderer, /queryKey\[1\] !== "image-generation"/);
    assert.match(renderer, /image_generation_limit_reached/);
    assert.match(renderer, /if \(loaded\) syncOfficialUsagePolicy\(\);/);
    assert.doesNotMatch(renderer, /officialUsageAlertCards|refreshOfficialUsageAlertVisibility|codex-plus-hide-usage-alert/);
    assert.doesNotMatch(renderer, /mutationTouchesUsageAlert/);
    assert.match(renderer, /function isOfficialLowQuotaSidebarCard/);
    assert.match(renderer, /function isOfficialLowQuotaComposerBanner/);
    assert.match(renderer, /upsell-banner-title-/);
    assert.match(renderer, /function isOfficialLowQuotaComposerAside/);
    assert.match(renderer, /tagName !== "ASIDE"/);
    assert.match(renderer, /rounded-3xl/);
    assert.match(renderer, /function syncOfficialUsageWindowMode/);
    assert.match(renderer, /key === "official-hide"/);
    assert.match(renderer, /officialUsageWindowObserver\?\.disconnect\(\)/);
    assert.match(renderer, /officialUsageWindowMarker\}="hidden"/);
    assert.doesNotMatch(scan, /startOfficialUsageWindowBlock|hideOfficialUsageWindowsWithin/);
    const policyStart = renderer.indexOf("  function syncOfficialUsagePolicy()");
    const policyEnd = renderer.indexOf("  if (window.__CODEX_PLUS_TEST_RATE_LIMIT_UNLOCK__)", policyStart);
    assert.ok(policyStart >= 0 && policyEnd > policyStart);
    const policy = renderer.slice(policyStart, policyEnd);
    const sameKey = policy.slice(policy.indexOf("if (key === officialUsagePolicyApplied)"), policy.indexOf("const previous"));
    assert.match(sameKey, /rewriteCachedOfficialUsage/);
    assert.doesNotMatch(sameKey, /hideOfficialUsageWindowsWithin|querySelectorAll/);
    assert.match(renderer, /function codexPlusPublishUsageData/);
    assert.match(renderer, /unlockSend: mixed/);
    assert.match(renderer, /limit_reached: false/);
    assert.doesNotMatch(renderer, /__codexPlusApiQuotaGate/);
    assert.doesNotMatch(renderer, /installExternalApiQuotaGate|__codexPlusApiQuotaBreakpoint|refreshComposers/);
  });

  // issue #2169：HTTP 回落成功不得掩盖桥接通道故障。桥接失败计数独立于后端状态，
  // 连续失败时状态灯降级呈现；回落路径绝不能清零计数或刷新 bridge 健康时间戳，
  // 否则启动器侧健康检查失去修复动力，重注入风暴对用户完全静默。
  it("surfaces persistent bridge degradation while the http fallback keeps the backend reachable", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /const CODEX_PLUS_BRIDGE_FAILURE_THRESHOLD = 3;/);
    assert.match(
      renderer,
      /function recordCodexPlusBridgeFailure\(\) \{\s*codexPlusBridgeFailureCount \+= 1;/,
    );
    // 降级渲染：桥接连续失败 + 后端 ok → degraded
    assert.match(renderer, /bridgeDegraded && rawStatus === "ok" \? "degraded" : rawStatus/);
    assert.match(renderer, /桥接降级，自动修复中/);
    // 回落分支只记失败，不得触碰成功路径
    const fallbackBlock = renderer.match(
      /const fallback = await fetchBackendStatusFromHelper\(path, payload\);[\s\S]*?return fallback;\s*\}/,
    );
    assert.ok(fallbackBlock, "http fallback block not found in postJson");
    assert.doesNotMatch(fallbackBlock[0], /recordCodexPlusBridgeSuccess\(\)/);
    // 降级状态有专属样式（指示灯与文本）
    assert.match(renderer, /\.codex-plus-backend-indicator\[data-status="degraded"\]/);
    assert.match(renderer, /\.codex-plus-backend-label\[data-status="degraded"\]/);
  });

  it("marks repeated bridge failures unhealthy despite fresh attempts, while preserving grace periods", async () => {
    const source = await readFile(new URL("../../../crates/codex-plus-core/src/bridge.rs", import.meta.url), "utf8");
    const script = source.match(/pub fn bridge_health_check_script\(\)[\s\S]*?r#"([\s\S]*?)"#/);
    assert.ok(script);
    const evaluate = new Function("window", "document", "Date", `return (${script[1].trim()});`);
    const probe = (health: Record<string, number>, hidden = false, missingBridge = false) => evaluate(
      { __codexSessionDeleteBridge: missingBridge ? undefined : () => {}, __codexPlusBridgeHealth: health },
      { visibilityState: hidden ? "hidden" : "visible" },
      { now: () => 60_000 },
    );
    const stalled = { lastSuccessAt: 40_000, lastAttemptAt: 59_999, failures: 3 };
    assert.equal(probe(stalled), false);
    assert.equal(probe(stalled, true), true);
    assert.equal(probe(stalled, true, true), false);
    assert.equal(probe({ ...stalled, lastInjectionAt: 58_000 }), true);
    assert.equal(probe({ ...stalled, lastSuccessAt: 50_000 }), true);
    assert.equal(probe({ lastAttemptAt: 59_999, failures: 2 }), true);
    assert.equal(probe({ lastAttemptAt: 59_999, failures: 3 }), false);
    assert.equal(probe({ lastAttemptAt: 40_000 }), false);
  });

  it("shares bridge failure health with the watchdog without stale generation writes", async () => {
    const source = await readFile(new URL("../../../assets/inject/renderer-inject/40-backend-settings.js", import.meta.url), "utf8");
    const start = source.indexOf("  function recordCodexPlusBridgeHealth(");
    const end = source.indexOf("  function renderBackendStatus()", start);
    assert.ok(start >= 0 && end > start);
    const health: Record<string, number> = {};
    const runtimeWindow = { __codexPlusBackendGeneration: 2, __codexPlusBridgeHealth: health };
    const runtime = new Function("window", "Date", `
      let codexPlusBridgeFailureCount = 0;
      const codexPlusBackendGeneration = 2;
      ${source.slice(start, end)}
      return { fail: recordCodexPlusBridgeFailure, succeed: recordCodexPlusBridgeSuccess };
    `)(runtimeWindow, { now: () => 60_000 });
    runtime.fail(); runtime.fail(); runtime.fail();
    assert.equal(health.failures, 3);
    assert.equal(health.lastFailureAt, 60_000);
    runtime.succeed();
    assert.equal(health.failures, 0);
    assert.equal(health.lastSuccessAt, 60_000);
    runtimeWindow.__codexPlusBackendGeneration = 3;
    Object.assign(health, { failures: 7, lastFailureAt: 70_000, lastSuccessAt: 70_000 });
    runtime.fail(); runtime.succeed();
    assert.deepEqual(health, { failures: 7, lastFailureAt: 70_000, lastSuccessAt: 70_000 });
  });

  it("dismisses the upstream usage-limit dialog and leaves other dialogs alone", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const quotaDialog = new UsageLimitNode({
      text: "You've reached your usage limit. Your limit resets at 3:00 PM.",
    });
    const dismissButton = new UsageLimitNode({ dismissLabel: "Close" });
    quotaDialog.appendChild(dismissButton);
    const unrelatedDialog = new UsageLimitNode({ text: "Delete this conversation?" });
    const sendButton = new UsageLimitNode({ dismissLabel: "Send message" });
    sendButton.disabled = true;
    const composer = new UsageLimitNode({ inert: true, pointerEvents: "none" });
    const { runtime, diagnostics } = usageLimitUnblockRuntime(
      renderer,
      [quotaDialog, unrelatedDialog],
      composer,
      sendButton,
    );

    runtime.refreshCodexUsageLimitUnblock();

    assert.equal(quotaDialog.getAttribute("data-codex-plus-usage-limit-hidden"), "true");
    assert.equal(unrelatedDialog.getAttribute("data-codex-plus-usage-limit-hidden"), null);
    assert.equal(dismissButton.clickCount, 1);
    assert.equal(diagnostics[diagnostics.length - 1]?.event, "usage_limit_dialog_dismissed");

    runtime.refreshCodexUsageLimitComposerUnblock();

    assert.equal(sendButton.disabled, false);
    assert.equal(composer.hasAttribute("inert"), false);
    assert.equal(composer.style.pointerEvents, "");
  });

  it("keeps Windows Dream Skin compatible with the modern Codex main surface", async () => {
    const dreamSkinRenderer = await readFile(
      new URL("../../../assets/inject/upstream/dream-skin/windows/renderer-inject.js", import.meta.url),
      "utf8",
    );
    const cidalaRenderer = await readFile(
      new URL("../../../assets/inject/upstream/cidala-tiger/windows/renderer-inject.js", import.meta.url),
      "utf8",
    );

    assert.match(dreamSkinRenderer, /codex-dream-skin-selectors\/1/);
    assert.match(dreamSkinRenderer, /MainContentSurface/);
    assert.match(dreamSkinRenderer, /data-ds-part/);
    assert.match(cidalaRenderer, /MainContentSurface/);
    assert.match(cidalaRenderer, /data-codex-plus-dream-surface/);
    assert.match(cidalaRenderer, /ensureShellMain/);
  });
});

describe("renderer injection codex_app tool probe", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  it("resets accumulated tool evidence when a new task scope starts", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const eventStart = renderer.indexOf("  function codexAppToolsProbeEvent(");
    const eventEnd = renderer.indexOf("\n  function installCodexAppToolsProbe()", eventStart);
    assert.ok(eventStart >= 0 && eventEnd > eventStart, "codex_app probe event block not found");
    const eventSource = renderer.slice(eventStart, eventEnd);

    const reset = eventSource.indexOf("if (startsNewThreadScope) resetCodexAppToolsProbeState();");
    const turnReset = eventSource.indexOf("else if (startsNewTurnScope) resetCodexAppToolsProbeState(true);");
    const accumulate = eventSource.indexOf("codexAppToolsProbeState.mcpSeen =");
    assert.ok(reset >= 0 && turnReset > reset && accumulate > turnReset);
  });

  it("counts tools only after the event is identified as codex_app MCP", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const eventStart = renderer.indexOf("  function codexAppToolsProbeEvent(");
    const eventEnd = renderer.indexOf("\n  function installCodexAppToolsProbe()", eventStart);
    assert.ok(eventStart >= 0 && eventEnd > eventStart, "codex_app probe event block not found");
    const eventSource = renderer.slice(eventStart, eventEnd);

    assert.match(eventSource, /const mcpToolNames = mcpSeen \? toolNames : null;/);
    assert.match(eventSource, /if \(mcpToolNames\) \{[\s\S]*?mcpToolCount = mcpToolNames\.length;/);
  });

  it("does not log unrelated thread traffic without tool or model evidence", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const eventStart = renderer.indexOf("  function codexAppToolsProbeEvent(");
    const eventEnd = renderer.indexOf("\n  function installCodexAppToolsProbe()", eventStart);
    assert.ok(eventStart >= 0 && eventEnd > eventStart, "codex_app probe event block not found");
    const eventSource = renderer.slice(eventStart, eventEnd);

    assert.match(eventSource, /const hasContextEvidence = Boolean\(/);
    assert.match(
      eventSource,
      /if \(!mcpSeen && !legacyDynamicCodexApp && !mcpToolNames && !hasContextEvidence\) return;/,
    );
    assert.doesNotMatch(eventSource, /const isTurnOrThread =/);
  });

  it("recognizes a nested mcp__codex_app read_thread tool event", async () => {
    const runtime = codexAppToolsProbeRuntime(await readFile(rendererPath, "utf8"));

    runtime.event("incoming", {
      method: "item/completed",
      params: {
        item: {
          namespace: "mcp__codex_app",
          tool: "read_thread",
        },
      },
    });

    assert.equal(runtime.diagnostics.length, 1);
    assert.equal(runtime.diagnostics[0]?.event, "codex_app_tools_probe");
    assert.equal(runtime.diagnostics[0]?.detail.mcpSeen, true);
    assert.equal(runtime.diagnostics[0]?.detail.mcpHasReadThread, true);
  });
});

/** 从注入脚本里取出 `shouldScheduleScan`，配上可控的依赖来跑。 */
function shouldScheduleScanRuntime(renderer: string) {
  const start = renderer.indexOf("  function shouldScheduleScan(");
  const end = renderer.indexOf("\n  function runScheduledScan(", start);
  assert.ok(start >= 0 && end > start, "shouldScheduleScan not found in renderer-inject.js");
  const source = renderer.slice(start, end);
  const factory = new Function(
    "isChatContentMutation",
    "isExtensionUiNode",
    "nodeSelfOrAncestorMatchesScanRelevance",
    "isScanRelevantNode",
    `${source}\nreturn shouldScheduleScan;`,
  );
  return factory(
    () => false,
    (node: { extension?: boolean }) => Boolean(node?.extension),
    // Codex 的容器（header / 侧栏 nav）本身就是 scan-relevant，这是自喂循环的关键前提。
    (node: { relevant?: boolean }) => Boolean(node?.relevant),
    (node: { relevant?: boolean; extension?: boolean }) =>
      Boolean(node?.relevant) && !node?.extension,
  ) as (mutations: unknown[]) => boolean;
}

const codexContainer = { nodeType: 1, relevant: true };

function mutation(addedNodes: unknown[] = [], removedNodes: unknown[] = []) {
  return { target: codexContainer, addedNodes, removedNodes };
}

describe("renderer injection scan scheduling", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：我们把自己的节点挂进 Codex 的容器，容器是 scan-relevant，
  // 于是每次写入都会再排一次 scan，scan 又重新写入，空闲时 CPU 被吃满。
  it("ignores mutations that only move the extension's own nodes", async () => {
    const shouldScheduleScan = shouldScheduleScanRuntime(await readFile(rendererPath, "utf8"));
    const ownNode = { nodeType: 1, extension: true };

    assert.equal(shouldScheduleScan([mutation([ownNode])]), false);
    // appendChild 一个已经在位的子节点会同时报 removed + added。
    assert.equal(shouldScheduleScan([mutation([ownNode], [ownNode])]), false);
  });

  it("still scans when Codex itself changes the same container", async () => {
    const shouldScheduleScan = shouldScheduleScanRuntime(await readFile(rendererPath, "utf8"));
    const codexNode = { nodeType: 1, relevant: true };
    const ownNode = { nodeType: 1, extension: true };

    assert.equal(shouldScheduleScan([mutation([codexNode])]), true);
    // 混合变更里只要有一个不是我们的，就不能跳过。
    assert.equal(shouldScheduleScan([mutation([ownNode, codexNode])]), true);
    // 属性变更没有 added/removed 节点，仍按容器相关性判定。
    assert.equal(shouldScheduleScan([mutation()]), true);
  });
});

interface MarketplacePatchHarness {
  install: () => void;
  sweeps: () => number;
  diagnostics: () => string[];
  settle: () => Promise<void>;
}

function marketplacePatchRuntime(renderer: string, patchSucceeds: boolean): MarketplacePatchHarness {
  const start = renderer.indexOf("  const pluginMarketplaceRequestPatchMaxMisses = ");
  const end = renderer.indexOf("\n  function pluginPatchDisabledInRelayMode(", start);
  assert.ok(start >= 0 && end > start, "marketplace patch block not found in renderer-inject.js");
  const source = renderer.slice(start, end);

  let sweeps = 0;
  let pending: Array<() => void> = [];
  const diagnostics: string[] = [];
  const fakeWindow: Record<string, unknown> = {};

  const factory = new Function(
    "window",
    "codexPluginMarketplaceUnlockVersion",
    "pluginPatchDisabledInRelayMode",
    "codexPlusSettings",
    "loadAppServerRequestCandidates",
    "patchPluginMarketplaceRequestClient",
    "sendCodexPlusDiagnostic",
    "__note",
    `${source}\nreturn installPluginMarketplaceRequestPatch;`,
  );

  const install = factory(
    fakeWindow,
    1,
    () => false,
    () => ({ pluginMarketplaceUnlock: true }),
    // 每轮 sweep 在真实实现里会 fetch 全部 app asset，这里只计数并挂起，
    // 好让测试能在「上一轮尚未结束」的时刻再次调用 install。
    () =>
      new Promise((resolve) => {
        sweeps += 1;
        pending.push(() => resolve({ modules: [{}], candidates: [{}], sources: [], discovery: "fallback" }));
      }),
    () => patchSucceeds,
    (event: string) => diagnostics.push(event),
  ) as () => void;

  const settle = async () => {
    // 放行所有挂起的 sweep，并把微任务队列排空。
    while (pending.length) {
      const flush = pending;
      pending = [];
      flush.forEach((resolve) => resolve());
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    }
  };

  return { install, sweeps: () => sweeps, diagnostics: () => diagnostics, settle };
}

/** 取出共用的 module loader，用假时钟驱动它的失败冷却。 */
function moduleLoaderRuntime(renderer: string) {
  const start = renderer.indexOf("  // issue #1960：失败必须被记住。");
  const end = renderer.indexOf("\n  async function loadOptionalCodexAppModule(", start);
  assert.ok(start >= 0 && end > start, "loadCodexAppModule not found in renderer-inject.js");
  const source = renderer.slice(start, end);

  let sweeps = 0;
  let clock = 1_000_000;
  const factory = new Function(
    "codexServiceTierModulePromises",
    "codexAppModuleFailures",
    "codexAppModuleRetryCooldownMs",
    "codexAppModuleMaxAttempts",
    "codexAppAssetUrl",
    "codexAppAssetUrlFromScriptText",
    "Date",
    `${source}\nreturn loadCodexAppModule;`,
  );
  const load = factory(
    new Map(),
    new Map(),
    30000,
    8,
    () => "",
    // 真实实现在这里会把全部 app asset 拉一遍；这里只计数并同样返回“没找到”。
    async () => {
      sweeps += 1;
      return "";
    },
    { now: () => clock },
  ) as (namePart: string) => Promise<unknown>;

  const attempt = async (namePart = "vscode-api-") => {
    try {
      await load(namePart);
    } catch {
      /* 预期失败 */
    }
  };
  return { attempt, sweeps: () => sweeps, advance: (ms: number) => { clock += ms; } };
}

describe("renderer injection codex app module loader", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：失败以前只是把 promise 删掉，等于没有负缓存，
  // 调用方一重试就重新 fetch 全部 app asset（实测 301 次请求/秒）。
  it("does not re-sweep every asset while the failure is still in cooldown", async () => {
    const loader = moduleLoaderRuntime(await readFile(rendererPath, "utf8"));

    for (let i = 0; i < 20; i += 1) await loader.attempt();

    assert.equal(loader.sweeps(), 1);
  });

  it("retries once per cooldown window, then gives up for good", async () => {
    const loader = moduleLoaderRuntime(await readFile(rendererPath, "utf8"));

    // 冷却期满就允许再试一次，避免 Codex 更新后 asset 回来了却永远发现不了。
    for (let i = 0; i < 30; i += 1) {
      await loader.attempt();
      loader.advance(30001);
    }

    // 连续失败达到上限(8)后彻底停手，而不是每个冷却窗口都再扫一遍。
    assert.equal(loader.sweeps(), 8);
  });

  it("keeps failures separate per asset prefix", async () => {
    const loader = moduleLoaderRuntime(await readFile(rendererPath, "utf8"));

    await loader.attempt("vscode-api-");
    await loader.attempt("app-initial-");
    await loader.attempt("vscode-api-");

    // 两个前缀各自试了一次；第三次命中 vscode-api- 自己的冷却。
    assert.equal(loader.sweeps(), 2);
  });
});

interface DispatcherPatchHarness {
  install: () => void;
  attempts: () => number;
  diagnostics: () => string[];
  settle: () => Promise<void>;
}

function dispatcherPatchRuntime(renderer: string, dispatcherFound: boolean): DispatcherPatchHarness {
  const start = renderer.indexOf("  const serviceTierDispatcherPatchMaxMisses = ");
  const end = renderer.indexOf("\n  async function loadBackendSettingsState(", start);
  assert.ok(start >= 0 && end > start, "service tier dispatcher patch block not found");
  const source = renderer.slice(start, end);

  let attempts = 0;
  let pending: Array<() => void> = [];
  const diagnostics: string[] = [];
  const fakeWindow: Record<string, unknown> = {};

  const factory = new Function(
    "window",
    "codexServiceTierRequestOverrideVersion",
    "loadCodexAppModule",
    "codexServiceTierDispatcherFromModule",
    "dispatchCodexPlusMessage",
    "installCodexRemoteSessionDispatcherSubscription",
    "installCodexQuotaResumeDispatcherSubscription",
    "sendCodexPlusDiagnostic",
    `${source}\nreturn installCodexServiceTierDispatcherPatch;`,
  );

  const install = factory(
    fakeWindow,
    1,
    // 真实实现每轮会依次试三个前缀，每个 miss 都触发一轮全量 asset 扫描。
    () =>
      new Promise((resolve, reject) => {
        attempts += 1;
        pending.push(() => (dispatcherFound ? resolve({}) : reject(new Error("未找到 Codex App asset"))));
      }),
    () => (dispatcherFound ? { dispatchMessage() {}, subscribe() {} } : null),
    () => undefined,
    () => undefined,
    () => undefined,
    (event: string) => diagnostics.push(event),
  ) as () => void;

  const settle = async () => {
    while (pending.length) {
      const flush = pending;
      pending = [];
      flush.forEach((resolve) => resolve());
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    }
  };

  return { install, attempts: () => attempts, diagnostics: () => diagnostics, settle };
}

describe("renderer injection service tier dispatcher patch", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：这个补丁挂在 scanLightweight() 里每轮都跑，是 #1324 同一缺陷的第三个实例。
  it("does not start a new sweep while the previous one is still running", async () => {
    const harness = dispatcherPatchRuntime(await readFile(rendererPath, "utf8"), false);

    for (let i = 0; i < 20; i += 1) harness.install();

    assert.equal(harness.attempts(), 1);
    await harness.settle();
  });

  it("stops retrying and stops re-reporting once the dispatcher is clearly gone", async () => {
    const harness = dispatcherPatchRuntime(await readFile(rendererPath, "utf8"), false);

    for (let i = 0; i < 40; i += 1) {
      harness.install();
      await harness.settle();
    }

    // 三个前缀里第一个就抛，loadDispatcher 会继续试下一个，所以每轮不止一次尝试；
    // 关键是达到 maxMisses(8) 之后彻底停手。
    assert.equal(harness.diagnostics().filter((e) => e === "service_tier_dispatcher_patch_failed").length, 1);
    assert.deepEqual(harness.diagnostics().at(-1), "service_tier_dispatcher_patch_skipped");
    const settled = harness.attempts();
    harness.install();
    await harness.settle();
    assert.equal(harness.attempts(), settled);
  });

  it("keeps working normally when the dispatcher is found", async () => {
    const harness = dispatcherPatchRuntime(await readFile(rendererPath, "utf8"), true);

    harness.install();
    await harness.settle();
    for (let i = 0; i < 10; i += 1) harness.install();

    assert.equal(harness.attempts(), 1);
    assert.deepEqual(harness.diagnostics(), ["service_tier_dispatcher_patch_installed"]);
  });
});

describe("renderer injection plugin marketplace patch", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // issue #1960：scanDeferred() 每轮都调用这个补丁，而早退守卫只在打上补丁后才写入。
  // Codex 侧 asset 改名后这层永远成功不了，过去既不去重也不放弃，
  // 于是每轮 scan 都把全部 app asset 重新 fetch 一遍（实测 530 次 fetch/秒）。
  it("does not start a new sweep while the previous one is still running", async () => {
    const harness = marketplacePatchRuntime(await readFile(rendererPath, "utf8"), false);

    // 模拟连续多轮 scan：上一轮还挂着，后续调用必须被 in-flight 守卫挡掉。
    for (let i = 0; i < 20; i += 1) harness.install();

    assert.equal(harness.sweeps(), 1);
    await harness.settle();
  });

  it("stops retrying once the asset is clearly unavailable", async () => {
    const harness = marketplacePatchRuntime(await readFile(rendererPath, "utf8"), false);

    // 每次都跑完再发起下一轮，模拟长时间运行中的反复 scan。
    for (let i = 0; i < 40; i += 1) {
      harness.install();
      await harness.settle();
    }

    // 达到 maxMisses(8) 之后必须彻底停掉，而不是无限重试。
    assert.equal(harness.sweeps(), 8);
    // 首次 miss 上报一次，停用时再报一次，中间保持噤声。
    assert.deepEqual(harness.diagnostics(), [
      "plugin_marketplace_request_patch_not_found",
      "plugin_marketplace_request_patch_skipped",
    ]);
  });

  it("keeps working normally when the patch actually lands", async () => {
    const harness = marketplacePatchRuntime(await readFile(rendererPath, "utf8"), true);

    harness.install();
    await harness.settle();
    // 打上补丁后守卫生效，后续 scan 不再重复扫描。
    for (let i = 0; i < 10; i += 1) harness.install();

    assert.equal(harness.sweeps(), 1);
    assert.deepEqual(harness.diagnostics(), ["plugin_marketplace_request_patch_installed"]);
  });

  // 26.928.31416 起 Codex 又把过滤器里的标识符换了一轮名（!Mj(e.marketplaceName)||e.marketplaceName===n）。
  // 过去按压缩字面量匹配，发版即失效：补丁照装，但 plugin_build_flavor_filter_bypassed 从不触发，
  // 插件解锁静默失灵。现在改用结构正则，下面把历史形态与新形态一起钉住。
  function extractFilterPattern(renderer: string, name: string): RegExp {
    const match = renderer.match(new RegExp(`const ${name}\\s*=\\s*\\/([\\s\\S]*?)\\/;`));
    assert.ok(match, `${name} 未在产物中找到`);
    return new RegExp(match![1]);
  }

  it("matches build-flavor filter shapes across Codex builds", async () => {
    const pattern = extractFilterPattern(await readFile(rendererPath, "utf8"), "codexPluginBuildFlavorFilterSourcePattern");
    const shapes = [
      "function EHr({buildFlavor:e,plugins:t}){let n=Gpt(e);return t.filter(e=>!Mj(e.marketplaceName)||e.marketplaceName===n)}", // 26.928.31416
      "e.filter(e=>!u(e.marketplaceName)||e.marketplaceName===r)",
      "e.filter(e=>!ne(e.marketplaceName)||e.marketplaceName===n)",
      "e.filter(e=>!Eu(e.marketplaceName)||e.marketplaceName===n)",
    ];
    for (const source of shapes) assert.ok(pattern.test(source), `应命中: ${source}`);
    assert.ok(!pattern.test("e.filter(x=>x.enabled&&x.name.length>3)"), "不应命中无关过滤器");
  });

  it("detects the featured plugin id filter separately", async () => {
    const pattern = extractFilterPattern(await readFile(rendererPath, "utf8"), "codexPluginFeaturedFilterSourcePattern");
    // featuredPluginIds 那条入参是字符串 id，形态与 build-flavor 不同，必须单独认。
    assert.ok(pattern.test("function THr({buildFlavor:e,featuredPluginIds:t}){let n=Gpt(e);return t.filter(e=>{let t=Gj(e);return t==null||!Mj(t)||t===n})}"));
    assert.ok(!pattern.test("e.filter(e=>!Mj(e.marketplaceName)||e.marketplaceName===n)"), "不应与 build-flavor 过滤器混淆");
    // 真实 bundle 里存在这个形状相近的无关函数；右值 `SFe(t)` 是调用，
    // 早期写法 `(?!\s*\()` 会被贪婪回溯绕过（SFe 退成 SF），实测踩过。
    assert.ok(
      !pattern.test("let r=bSe(e.path)?.pluginMarketplaceName??null;return t==null||r==null||!ds(r)||r===SFe(t)"),
      "不应命中 bundle 里的无关 null-guard",
    );
  });

  it("matches hidden-marketplace filter shapes structurally", async () => {
    const pattern = extractFilterPattern(await readFile(rendererPath, "utf8"), "codexPluginHiddenFilterSourcePattern");
    assert.ok(pattern.test("function hHr(e,t){return t.length===0?e:e.filter(e=>!t.includes(e.name))}"));
    assert.ok(pattern.test("marketplaces:a.filter(e=>!n.includes(e.name)&&(!r.Po(e.name)||e.name===c))"));
    assert.ok(!pattern.test("e.filter(e=>e.plugins.some(p=>p.name))"), "不应命中无关过滤器");
    // 真实 bundle 里的无关守卫：不是 filter 箭头形态，必须排掉。
    assert.ok(!pattern.test("if(n[t.name]=t,!w4.includes(t.name)&&typeof t.setupOnce==`function`"), "不应命中非 filter 守卫");
  });
});

describe("relay pureApi provider resolution", () => {
  function providerRuntime(
    renderer: string,
    backendSettings: Record<string, unknown>,
    catalog: Record<string, unknown>,
    profile: Record<string, unknown>,
  ) {
    const start = renderer.indexOf("function codexRelayConfigModelProvider(");
    const codeStart = renderer.indexOf("function codexRemoteSessionTargetProvider(");
    const end = renderer.indexOf("\n  function codexRemoteSessionProviderRequestMethod", codeStart);
    assert.ok(start >= 0 && codeStart >= 0 && end > codeStart);
    const source = renderer.slice(start, end);
    const create = new Function(
      "codexPlusBackendSettings",
      "codexModelCatalog",
      "codexRemoteSessionActiveProfile",
      `${source}\nreturn { codexRelayConfigModelProvider, codexRemoteSessionTargetProvider };`,
    ) as (
      backend: Record<string, unknown>,
      cat: Record<string, unknown>,
      activeProfile: () => Record<string, unknown>,
    ) => {
      codexRelayConfigModelProvider: (configContents: string) => string;
      codexRemoteSessionTargetProvider: () => string;
    };
    return create(backendSettings, catalog, () => profile);
  }

  it("resolves the real model_provider from a pureApi relay profile instead of hardcoding custom", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      {},
      { codex_model_provider: "deepseek" },
      { relayMode: "pureApi", configContents: 'model = "deepseek-v4-flash-vision-exp"\nmodel_provider = "deepseek"' },
    );

    assert.equal(runtime.codexRelayConfigModelProvider('model_provider = "deepseek"'), "deepseek");
    assert.equal(runtime.codexRemoteSessionTargetProvider(), "deepseek");
  });

  it("still returns custom for pureApi relays that genuinely declare the custom provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      {},
      { codex_model_provider: "custom" },
      { relayMode: "pureApi", configContents: 'model_provider = "custom"\n[model_providers.custom]' },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "custom");
  });

  it("falls back to custom when a pureApi relay declares no provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(renderer, {}, { codex_model_provider: "" }, { relayMode: "pureApi", configContents: "" });

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "custom");
  });

  // activeRelayCodexProvider 是全局缓存，切换供应商后可能还留着上一个的值。
  // pureApi 时优先信 profile 自己的 configContents；profile 没声明就回到
  // "custom"，不采信这个缓存——cdp_bridge.rs 的 refreshedPureApiResumeProvider
  // 正是钉这个：陈旧缓存是 stale_custom_provider 时必须仍解析成 custom。
  it("ignores a possibly stale activeRelayCodexProvider for pureApi relays", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      { activeRelayCodexProvider: "stale_custom_provider" },
      { codex_model_provider: "" },
      { relayMode: "pureApi", configContents: "" },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "custom");
  });

  // 非 pureApi 才拿 activeRelayCodexProvider 兜底。
  it("still falls back to activeRelayCodexProvider outside pureApi", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      { activeRelayCodexProvider: "deepseek" },
      { codex_model_provider: "" },
      { relayMode: "mixedApi", configContents: "" },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "deepseek");
  });

  // profile 自己声明了供应方时，优先级高于全局缓存。
  it("prefers the profile's own configContents over the cached provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = providerRuntime(
      renderer,
      { activeRelayCodexProvider: "stale_custom_provider" },
      { codex_model_provider: "" },
      { relayMode: "pureApi", configContents: 'model_provider = "deepseek"' },
    );

    assert.equal(runtime.codexRemoteSessionTargetProvider(), "deepseek");
  });
});

describe("official experience model and provider switching", () => {
  it("preserves a selected official account for native models and resumed turns", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function codexRemoteSessionActiveProfile(");
    const end = renderer.indexOf("\n  function codexRemoteSessionStartedThreadId(", start);
    assert.ok(start >= 0 && end > start);
    const account = "codex_plus_official_account_2";
    const model = "gpt-6-luna";
    const catalog = { routeDescriptors: [
      { routingSlug: model, providerId: "openai" },
      { routingSlug: `${account}:${model}`, providerId: account },
      { routingSlug: `${account}:gpt-6.1-sol`, providerId: account },
    ] };
    const notices: string[] = [];
    const create = new Function("codexPlusBackendSettings", "codexModelCatalog", "codexCatalogOfficialModels", "sendCodexPlusDiagnostic", "showToast",
      `${renderer.slice(start, end)}\nreturn { apply: applyCodexRemoteSessionProviderOverride, remember: rememberCodexOfficialThreadModel, notify: notifyCodexOfficialThreadProviderChange };`);
    const runtime = create({ officialExperience: { enabled: true } }, catalog, () => [model], () => {}, (message: string) => notices.push(message));
    assert.equal(runtime.notify(account, account), false);
    assert.equal(notices.length, 0);
    assert.equal(runtime.notify("openai", account), true);
    assert.equal(notices.length, 1);
    assert.match(notices[0], /子代理保持各自来源/);
    const following = create({ officialExperience: { enabled: true, followParentAccount: true } }, catalog,
      () => [model], () => {}, (message: string) => notices.push(message));
    assert.equal(following.notify("openai", account), true);
    assert.match(notices[1], /后续官方请求将跟随父对话账号/);
    assert.match(notices[1], /重启 Codex 一次，之后切换无需重启/);
    assert.doesNotMatch(notices[1], /停止旧子代理后重新创建/);
    for (const method of ["thread/settings/update", "thread/resume", "turn/start"]) {
      const threadId = `account-native-${method}`;
      const result = runtime.apply(method, { threadId, model, modelProvider: account, reasoningEffort: "ultra" });
      assert.equal(result.modelProvider, account, `${method} must retain the account provider`);
      assert.equal(result.reasoningEffort, "ultra");
      const nextTurn = runtime.apply("turn/start", { threadId, input: [] });
      assert.equal(nextTurn.model, model);
      assert.equal(nextTurn.modelProvider, account, "a model-less turn must retain the model/provider pair");
      assert.equal(runtime.apply("turn/start", { threadId, modelProvider: "openai", input: [] }).modelProvider, account);
    }
    const snake = runtime.apply("thread/resume", {
      thread_id: "snake-account", model_id: "gpt-6.1-sol", model_provider: account,
    });
    assert.equal(snake.modelProvider, account, "the account's own descriptor may include a model absent from the main catalog");
    assert.equal("model_provider" in snake, false);
    runtime.remember("resume-response", model, account);
    assert.equal(runtime.apply("turn/start", { threadId: "resume-response", input: [] }).modelProvider, account);
    for (const provider of ["openai", undefined]) {
      const params = { threadId: "resume-response", model, ...(provider ? { modelProvider: provider } : {}) };
      assert.equal(runtime.apply("thread/settings/update", params).modelProvider, "openai");
      assert.equal(runtime.apply("turn/start", { threadId: "resume-response", input: [] }).modelProvider, "openai");
    }
    for (const unknown of ["unknown-native", "codex_plus_official_other:gpt-6-luna"]) {
      assert.throws(() => runtime.apply("turn/start", {
        threadId: "reject-cross-account", model: unknown, modelProvider: account,
      }), /没有可用的来源路由/);
    }
  });

  it("keeps the selected route on a model-less turn and returns to the official account", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function codexRemoteSessionActiveProfile(");
    const end = renderer.indexOf("\n  function codexRemoteSessionStartedThreadId(", start);
    assert.ok(start >= 0 && end > start);
    const source = renderer.slice(start, end);
    const catalog = {
      default_model: "gpt-6-sol",
      routeDescriptors: [
        { routingSlug: "deepseek:deepseek-v4-flash", providerId: "codex_plus_relay_deepseek" },
        { routingSlug: "codex_plus_official_account_2:gpt-6-sol", providerId: "codex_plus_official_account_2" },
        { routingSlug: "gpt-6-sol", providerId: "openai" },
      ],
    };
    const create = new Function(
      "codexPlusBackendSettings", "codexModelCatalog", "codexCatalogOfficialModels", "sendCodexPlusDiagnostic",
      `${source}\nreturn { apply: applyCodexRemoteSessionProviderOverride, target: codexRemoteSessionTargetProvider };`,
    ) as (
      settings: object,
      catalog: object,
      officialModels: () => string[],
      diagnostic: () => void,
    ) => {
      apply: (method: string, params: Record<string, unknown>) => Record<string, unknown>;
      target: (model?: string) => string;
    };
    const runtime = create({ officialExperience: { enabled: true } }, catalog, () => ["gpt-6-sol"], () => {});
    const threadId = "thread-route-switch";
    assert.equal(runtime.target(), "", "an absent model must not inherit the catalog default");

    const selected = runtime.apply("thread/settings/update", {
      threadId, model: "deepseek:deepseek-v4-flash", modelProvider: "openai", reasoningEffort: "high",
    });
    assert.deepEqual(selected, {
      threadId, model: "deepseek:deepseek-v4-flash", modelProvider: "codex_plus_relay_deepseek", reasoningEffort: "high",
    });
    const explicitTurn = runtime.apply("turn/start", {
      threadId, model: "deepseek:deepseek-v4-flash", modelProvider: "openai", input: [],
    });
    assert.equal(explicitTurn.modelProvider, "codex_plus_relay_deepseek");
    const inheritedTurn = runtime.apply("turn/start", { threadId, modelProvider: "openai", input: [] });
    assert.equal(inheritedTurn.model, "deepseek:deepseek-v4-flash");
    assert.equal(inheritedTurn.modelProvider, "codex_plus_relay_deepseek");
    assert.deepEqual(runtime.apply("turn/start", { threadId: "unseen-thread", input: [] }), {
      threadId: "unseen-thread", input: [],
    });

    const secondAccount = runtime.apply("thread/settings/update", {
      threadId, model: "codex_plus_official_account_2:gpt-6-sol", modelProvider: "openai", reasoningEffort: "high",
    });
    assert.equal(secondAccount.modelProvider, "codex_plus_official_account_2");
    const secondAccountTurn = runtime.apply("turn/start", { threadId, input: [] });
    assert.equal(secondAccountTurn.model, "codex_plus_official_account_2:gpt-6-sol");
    assert.equal(secondAccountTurn.modelProvider, "codex_plus_official_account_2");

    const officialSelection = runtime.apply("thread/settings/update", {
      threadId, model: "gpt-6-sol", modelProvider: "codex_plus_relay_deepseek", reasoningEffort: "medium",
    });
    assert.equal(officialSelection.modelProvider, "openai");
    const officialTurn = runtime.apply("turn/start", { threadId, input: [] });
    assert.equal(officialTurn.model, "gpt-6-sol");
    assert.equal(officialTurn.modelProvider, "openai");
    assert.throws(() => runtime.apply("turn/start", {
      threadId, model: "unknown:outside-catalog", modelProvider: "openai", input: [],
    }), /没有可用的来源路由/);
  });

  it("records the account returned by a model-less resume before its next turn", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const routeStart = renderer.indexOf("  function codexRemoteSessionActiveProfile(");
    const routeEnd = renderer.indexOf("\n  function codexRemoteSessionStartedThreadId(", routeStart);
    const transportStart = renderer.indexOf("  function patchCodexVscodeRequestTransport(");
    const transportEnd = renderer.indexOf("\n  const appServerModelRequestPatchMaxMisses", transportStart);
    assert.ok(routeStart >= 0 && routeEnd > routeStart && transportStart >= 0 && transportEnd > transportStart);
    const account = "codex_plus_official_account_2";
    const model = "gpt-6-luna";
    const sent: Array<{ method: string; params: Record<string, unknown> }> = [];
    const transport = {
      pendingRequests: new Map(),
      async sendRequest(_method: string, url: string, options: { body: string }) {
        if (!url.startsWith("vscode://codex/") || !this.pendingRequests) throw new Error("invalid transport");
        const method = url.slice("vscode://codex/".length);
        sent.push({ method, params: JSON.parse(options.body) });
        return method === "thread/resume"
          ? { thread: { id: "native-resumed", model }, modelProvider: account }
          : { turn: { status: "inProgress" } };
      },
    };
    const Transport = class { static getInstance() { return transport; } };
    const create = new Function("Transport", "account", "model", `
      const codexPlusBackendSettings = { officialExperience: { enabled: true } };
      const codexModelCatalog = { routeDescriptors: [{ routingSlug: model, providerId: "openai" }] };
      const codexCatalogOfficialModels = () => [model];
      const sendCodexPlusDiagnostic = () => {};
      const codexAppServerModelRequestPatchVersion = "account-native-test";
      const appServerModelRequestMethod = (url) => url.slice("vscode://codex/".length);
      const loadBackendSettingsState = async () => true;
      const loadCodexModelCatalog = async () => codexModelCatalog;
      const normalizeCodexModelReasoningParams = (method, params) => applyCodexRemoteSessionProviderOverride(method, params);
      const codexRateLimitUnlockRequestMethod = () => "";
      const neutralizeCodexRateLimitPayload = (payload) => payload;
      ${renderer.slice(routeStart, routeEnd)}
      ${renderer.slice(transportStart, transportEnd)}
      ${providerSwitchHelperSource(renderer)}
       return patchCodexVscodeRequestTransport({ Transport });`);
    assert.equal(create(Transport, account, model), true);
    await transport.sendRequest("POST", "vscode://codex/thread/resume", { body: JSON.stringify({ threadId: "native-resumed" }) });
    await transport.sendRequest("POST", "vscode://codex/turn/start", { body: JSON.stringify({ threadId: "native-resumed", input: [] }) });
    assert.deepEqual(sent.map((entry) => entry.method), ["thread/resume", "turn/start"]);
    assert.equal(sent[1].params.model, model);
    assert.equal(sent[1].params.modelProvider, account, "the actual resumed provider must override the main catalog's model provider");
  });
});

describe("vscode model transport routing", () => {
  it("preserves workspace permissions and collaboration settings without broadening custom read access", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const switchProvider = new Function("normalizeCodexModelReasoningParams",
      `${providerSwitchHelperSource(renderer)};return switchCodexOfficialThreadProvider;`)(
      (_method: string, params: Record<string, unknown>) => params,
    );
    const calls: Array<{ method: string; params: Record<string, any> }> = [];
    const sandbox = { type: "workspaceWrite", readOnlyAccess: { type: "fullAccess" }, writableRoots: ["C:/allowed"],
      networkAccess: false, excludeTmpdirEnvVar: true, excludeSlashTmp: true };
    const send = async (method: string, params: Record<string, any>) => {
      calls.push({ method, params });
      if (method !== "thread/resume") return {};
      return { thread: { id: "original", status: { type: "idle" } }, modelProvider: params.modelProvider || "openai",
        sandbox, cwd: "C:/allowed", approvalPolicy: "on-request", approvalsReviewer: "user", reasoningEffort: "high",
        disabledPluginIds: ["disabled-plugin"], collaborationMode: { mode: "plan", settings: { model: "old", developer_instructions: "plan rules" } } };
    };
    await switchProvider(send, "thread/settings/update", { threadId: "original", model: "target" }, "schedule");
    const resume = calls[2].params;
    assert.equal(resume.cwd, "C:/allowed");
    assert.equal(resume.approvalPolicy, "on-request");
    assert.equal(resume.approvalsReviewer, "user");
    assert.equal(resume.sandbox, "workspace-write");
    assert.deepEqual(resume.config.sandbox_workspace_write, { writable_roots: ["C:/allowed"], network_access: false,
      exclude_tmpdir_env_var: true, exclude_slash_tmp: true });
    assert.deepEqual(calls[3].params.collaborationMode, { mode: "plan", settings: { model: "target", reasoning_effort: "high", developer_instructions: "plan rules" } });
    assert.deepEqual(calls[3].params.disabledPluginIds, ["disabled-plugin"]);
    calls.length = 0;
    sandbox.readOnlyAccess.type = "restricted";
    await assert.rejects(switchProvider(send, "turn/start", { threadId: "original", model: "target", input: [] }, "schedule"), /权限无法完整保留/);
    assert.deepEqual(calls.map(call => call.method), ["thread/resume"]);
    calls.length = 0;
    sandbox.readOnlyAccess.type = "fullAccess";
    const wrongThread = async (method: string, params: Record<string, any>) => {
      const response = await send(method, params);
      return params.modelProvider ? { ...response, thread: { id: "unexpected-copy" } } : response;
    };
    await assert.rejects(switchProvider(wrongThread, "turn/start", { threadId: "original", model: "target", input: ["private"] }, "schedule"), /本次消息未发送/);
    assert.deepEqual(calls.map(call => call.method), ["thread/resume", "thread/unsubscribe", "thread/resume"]);
  });

  it("switches scheduler settings in the original chat before forwarding aliases", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function patchCodexVscodeRequestTransport(");
    const end = renderer.indexOf("\n  const appServerModelRequestPatchMaxMisses", start);
    const provider = "codex_plus_official_schedule";
    const model = `${provider}:gpt-6-sol`;
    const calls: string[] = [];
    let loadedProvider = "openai";
    let unsubscribed = false;
    let resumeParams: Record<string, any> = {};
    const transport = { pendingRequests: new Map(), async sendRequest(_method: string, url: string, options: { body: string }) {
      if (!url.startsWith("vscode://codex/") || !this.pendingRequests) throw new Error("invalid transport");
      const method = url.slice("vscode://codex/".length);
      const params = JSON.parse(options.body);
      calls.push(method);
      if (method === "thread/read") {
        assert.equal(params.includeTurns, false);
        return { thread: { id: "original", modelProvider: loadedProvider, status: { type: loadedProvider ? "idle" : "notLoaded" } } };
      }
      if (method === "thread/unsubscribe") { unsubscribed = true; return {}; }
      if (method === "thread/resume") {
        assert.equal(params.excludeTurns, true);
        if (params.modelProvider) resumeParams = params;
        if (unsubscribed) loadedProvider = params.modelProvider || loadedProvider;
        unsubscribed = false;
        return { thread: { id: "original", status: { type: "idle" } }, modelProvider: loadedProvider,
          approvalPolicy: "never", sandbox: { type: "readOnly" }, reasoningEffort: "high" };
      }
      if (method === "thread/settings/update" || method === "turn/start") {
        assert.equal(loadedProvider, provider, "aliases must not reach the old login provider");
        return {};
      }
      throw new Error(`unexpected lifecycle call ${method}`);
    } };
    const Transport = class { static getInstance() { return transport; } };
    const patch = new Function("Transport", "model", "provider", `
      const codexAppServerModelRequestPatchVersion = "test-schedule";
      const codexPlusBackendSettings = { officialExperience: { enabled: true } };
      const codexModelCatalog = { routeDescriptors: [] };
      const appServerModelRequestMethod = url => url.slice("vscode://codex/".length);
      const codexRemoteSessionProviderRequestMethod = method => ["turn/start", "thread/settings/update"].includes(method);
      const codexRemoteSessionProviderPatchEnabled = () => true;
      const loadBackendSettingsState = async () => true;
      const loadCodexModelCatalog = async (force) => {
        if (force !== true) throw new Error("stale model routes must be refreshed");
        codexModelCatalog.routeDescriptors = [{ routingSlug: model, providerId: provider }];
        return codexModelCatalog;
      };
      const codexOfficialExperienceRouteDescriptor = slug => codexModelCatalog.routeDescriptors.find(route => route.routingSlug === slug);
      const normalizeCodexModelReasoningParams = (_method, params) => ({ ...params, modelProvider: provider });
      const codexRateLimitUnlockRequestMethod = () => false;
      const sendCodexPlusDiagnostic = () => {};
      const notifyCodexOfficialThreadProviderChange = () => {};
      ${renderer.slice(start, end)}
      ${providerSwitchHelperSource(renderer)}
       return patchCodexVscodeRequestTransport({ Transport });`);
    assert.equal(patch(Transport, model, provider), true);
    await Promise.all([
      transport.sendRequest("POST", "vscode://codex/thread/settings/update", { body: JSON.stringify({ threadId: "original", model, permissions: "my-profile" }) }),
      transport.sendRequest("POST", "vscode://codex/turn/start", { body: JSON.stringify({ threadId: "original", model, input: ["private"] }) }),
    ]);
    assert.deepEqual(calls, ["thread/read", "thread/resume", "thread/unsubscribe", "thread/resume", "thread/settings/update", "turn/start"]);
    assert.equal(resumeParams.threadId, "original");
    assert.equal(resumeParams.permissions, "my-profile");
    assert.equal(resumeParams.sandbox, undefined);
    assert.equal(resumeParams.config.model_provider, provider);
    assert.equal(resumeParams.config.model_reasoning_effort, "high");
    assert.equal(resumeParams.input, undefined);
  });

  it("routes official account 2 and returns to the main official account", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function patchCodexVscodeRequestTransport(");
    const end = renderer.indexOf("\n  const appServerModelRequestPatchMaxMisses", start);
    assert.ok(start >= 0 && end > start);
    const sent: Array<{ method: string; url: string; options: { body: string } }> = [];
    const transport = {
      pendingRequests: new Map(),
      async sendRequest(method: string, url: string, options: { body: string }) {
        // Match the current Codex transport shape used for discovery.
        if (url.startsWith("vscode://codex/") && this.pendingRequests) sent.push({ method, url, options });
        return { status: 200 };
      },
    };
    const Transport = class { static getInstance() { return transport; } };
    const catalog = { routeDescriptors: [
      { routingSlug: "codex_plus_official_account_2:gpt-6-sol", providerId: "codex_plus_official_account_2" },
      { routingSlug: "CLIProxyAPI:gpt-6-sol", providerId: "codex_plus_cli_official" },
    ] };
    const settings = { officialExperience: { enabled: true } };
    const create = new Function(
      "catalog", "settings", "diagnostics", "Transport",
      `const codexAppServerModelRequestPatchVersion = "9";
       const codexModelCatalog = catalog;
       const codexPlusBackendSettings = settings;
       const appServerModelRequestMethod = (url) => url.slice("vscode://codex/".length);
       const codexRemoteSessionProviderRequestMethod = (method) => ["thread/start", "turn/start", "thread/settings/update", "thread/resume"].includes(method);
       const codexRemoteSessionProviderPatchEnabled = () => true;
       const loadBackendSettingsState = async () => true;
       const loadCodexModelCatalog = async () => catalog;
       const codexOfficialExperienceRouteDescriptor = (model) => catalog.routeDescriptors.find((item) => item.routingSlug === model);
       const normalizeCodexModelReasoningParams = (_method, params) => ({ ...params, modelProvider: params.model.startsWith("codex_plus_official_") ? "codex_plus_official_account_2" : params.model.startsWith("CLIProxyAPI:") ? "codex_plus_cli_official" : "openai" });
       const sendCodexPlusDiagnostic = (event) => diagnostics.push(event);
       const codexRateLimitUnlockRequestMethod = () => "";
       const neutralizeCodexRateLimitPayload = (payload) => payload;
       ${renderer.slice(start, end)}
       ${providerSwitchHelperSource(renderer)}
       return patchCodexVscodeRequestTransport({ Transport });`,
    ) as (catalog: object, settings: object, diagnostics: string[], Transport: object) => boolean;
    const diagnostics: string[] = [];
    assert.equal(create(catalog, settings, diagnostics, Transport), true);
    for (const model of ["gpt-6-sol", "codex_plus_official_account_2:gpt-6-sol", "CLIProxyAPI:gpt-6-sol", "gpt-6-sol"]) {
      await transport.sendRequest("POST", "vscode://codex/thread/start", { body: JSON.stringify({ model }) });
    }
    assert.deepEqual(sent.map((entry) => JSON.parse(entry.options.body).modelProvider), [
      "openai", "codex_plus_official_account_2", "codex_plus_cli_official", "openai",
    ]);
    await assert.rejects(
      transport.sendRequest("POST", "vscode://codex/thread/start", { body: JSON.stringify({ model: "Unknown:model" }) }),
      /没有可用的来源路由/,
    );
    assert.equal(sent.length, 4);
    assert.deepEqual(diagnostics, ["model_vscode_request_patch_installed"]);
  });

  it("unloads a saved thread before switching between direct official accounts", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function patchCodexVscodeRequestTransport(");
    const end = renderer.indexOf("\n  const appServerModelRequestPatchMaxMisses", start);
    const calls: string[] = [];
    const providerChanges: string[][] = [];
    let loadedProvider = "";
    let unsubscribed = false;
    const transport = {
      pendingRequests: new Map(),
      async sendRequest(method: string, url: string, options: { body: string }) {
        if (!url.startsWith("vscode://codex/") || !this.pendingRequests) throw new Error("invalid transport");
        const requestMethod = url.slice("vscode://codex/".length);
        const params = JSON.parse(options.body);
        calls.push(requestMethod);
        if (requestMethod === "thread/start") {
          loadedProvider = params.modelProvider;
          return { thread: { id: "thread-hot" }, modelProvider: loadedProvider };
        }
        if (requestMethod === "thread/unsubscribe") {
          unsubscribed = true;
          return { status: "unsubscribed" };
        }
        if (requestMethod === "thread/read") {
          if (params.threadId === "empty-thread") throw new Error("no rollout found for thread id empty-thread");
          assert.equal(params.includeTurns, false);
          return { thread: { id: params.threadId, modelProvider: loadedProvider, status: { type: loadedProvider ? "idle" : "notLoaded" } } };
        }
        if (requestMethod === "thread/resume") {
          if (params.threadId === "empty-thread") throw new Error("no rollout found for thread id empty-thread");
          if (unsubscribed) loadedProvider = params.modelProvider || loadedProvider;
          loadedProvider ||= "openai";
          unsubscribed = false;
          return { modelProvider: loadedProvider, thread: { id: params.threadId, status: { type: "idle" } } };
        }
        if (requestMethod === "turn/start") {
          assert.equal(params.modelProvider, loadedProvider, "the turn must use the resumed provider");
          return { turn: { status: "inProgress" } };
        }
        throw new Error(`unexpected ${requestMethod}`);
      },
    };
    const Transport = class { static getInstance() { return transport; } };
    const account2 = "codex_plus_official_account_2";
    const route = `${account2}:gpt-6-sol`;
    const create = new Function(
      "Transport", "route", "account2", "providerChanges",
      `const codexAppServerModelRequestPatchVersion = "9";
       const codexModelCatalog = { routeDescriptors: [{ routingSlug: route, providerId: account2 }] };
       const codexPlusBackendSettings = { officialExperience: { enabled: true } };
       const appServerModelRequestMethod = (url) => url.slice("vscode://codex/".length);
       const codexRemoteSessionProviderRequestMethod = (method) => ["thread/start", "turn/start", "thread/settings/update", "thread/resume"].includes(method);
       const codexRemoteSessionProviderPatchEnabled = () => true;
       const loadBackendSettingsState = async () => true;
       const loadCodexModelCatalog = async () => codexModelCatalog;
       const codexOfficialExperienceRouteDescriptor = (model) => codexModelCatalog.routeDescriptors.find((item) => item.routingSlug === model);
       const normalizeCodexModelReasoningParams = (_method, params) => ({ ...params, modelProvider: params.model === route ? account2 : "openai" });
       const notifyCodexOfficialThreadProviderChange = (from, to) => providerChanges.push([from, to]);
       const sendCodexPlusDiagnostic = () => {};
       const codexRateLimitUnlockRequestMethod = () => "";
       const neutralizeCodexRateLimitPayload = (payload) => payload;
       ${renderer.slice(start, end)}
       ${providerSwitchHelperSource(renderer)}
       return patchCodexVscodeRequestTransport({ Transport });`,
    ) as (Transport: object, route: string, account2: string, providerChanges: string[][]) => boolean;
    assert.equal(create(Transport, route, account2, providerChanges), true);
    const request = (method: string, params: object) => transport.sendRequest(
      "POST", `vscode://codex/${method}`, { body: JSON.stringify(params) },
    );
    await request("thread/start", { model: "gpt-6-sol" });
    await request("turn/start", { threadId: "thread-hot", model: route, input: [] });
    await request("turn/start", { threadId: "thread-hot", model: "gpt-6-sol", input: [] });
    assert.deepEqual(calls, [
      "thread/start", "thread/resume", "thread/unsubscribe", "thread/resume", "turn/start",
      "thread/resume", "thread/unsubscribe", "thread/resume", "turn/start",
    ]);
    assert.deepEqual(providerChanges, [["openai", account2], [account2, "openai"]]);
    await assert.rejects(
      request("turn/start", { threadId: "empty-thread", model: route, input: [] }),
      /无法确认会话当前账号/,
    );
  });
});

describe("mcp broker model transport routing", () => {
  const account2 = "codex_plus_official_account_2";
  const account2Model = `${account2}:gpt-6-sol`;

  async function createBrokerRuntime(options: { readProvider?: string; hangResume?: boolean; active?: boolean; errorState?: boolean } = {}) {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const start = renderer.indexOf("  function patchCodexMcpRequestTransport(");
    const end = renderer.indexOf("\n  function patchCodexVscodeRequestTransport(", start);
    assert.ok(start >= 0 && end > start, "mcp broker transport patch not found");

    type Request = { id: number; method: string; params: Record<string, unknown> };
    type Message = { type: string; hostId: string; request?: Request; message?: { id: number; result?: unknown; error?: unknown } };
    const sent: Request[] = [];
    const sentTypes: string[] = [];
    const received: Message[] = [];
    const handlers = new Map<string, Set<(message: Message) => void>>();
    let loadedProvider = options.readProvider || "";
    let unsubscribed = false;
    let forcedResumeProvider = "";
    const broker = {
      handlers,
      bufferedMessages: new Map(),
      subscribe(type: string, handler: (message: Message) => void) {
        const listeners = handlers.get(type) || new Set<(message: Message) => void>();
        listeners.add(handler);
        handlers.set(type, listeners);
        return () => listeners.delete(handler);
      },
      deliverOrBufferMessage(message: Message) {
        received.push(message);
        for (const handler of handlers.get(message.type) || []) handler(message);
      },
      dispatchMessage(type: string, detail: Message) {
        assert.ok(type === "mcp-request" || type === "thread-prewarm-start");
        assert.equal(detail.hostId, "local");
        const request = detail.request;
        assert.ok(request);
        sent.push(request);
        sentTypes.push(type);
        const params = request.params || {};
        let result: unknown = {};
        if (request.method === "thread/resume" && options.hangResume) return;
        if (request.method === "thread/start") {
          loadedProvider = String(params.modelProvider || "openai");
          result = { thread: { id: "thread-hot" }, modelProvider: loadedProvider };
        } else if (request.method === "thread/read") {
          assert.equal(params.includeTurns, false);
          result = { thread: { id: "thread-hot", modelProvider: loadedProvider || "openai", status: { type: unsubscribed ? "notLoaded" : "idle" } } };
        } else if (request.method === "thread/unsubscribe") {
          unsubscribed = true;
        } else if (request.method === "thread/resume") {
          if (unsubscribed && !options.active && !options.errorState) loadedProvider = forcedResumeProvider || String(params.modelProvider || loadedProvider);
          unsubscribed = false;
          loadedProvider ||= forcedResumeProvider || String(params.modelProvider || "openai");
          result = { thread: { id: "thread-hot", status: { type: options.active ? "active" : options.errorState ? "systemError" : "idle" } }, modelProvider: loadedProvider,
            cwd: "C:/previous", approvalPolicy: "never", sandbox: { type: "readOnly" }, reasoningEffort: "high" };
        } else if (request.method === "thread/fork") {
          loadedProvider = String(params.modelProvider || "openai");
          result = { thread: { id: "thread-fork" }, modelProvider: loadedProvider };
        } else if (request.method === "thread/settings/update" && params.model) {
          assert.equal(params.modelProvider, loadedProvider, "settings aliases must use the selected provider");
        } else if (request.method === "turn/start") {
          assert.equal(params.modelProvider, loadedProvider, "turn must use the resumed account");
          result = { turn: { status: "inProgress" } };
        }
        queueMicrotask(() => broker.deliverOrBufferMessage({
          type: "mcp-response", hostId: "local", message: { id: request.id, result },
        }));
      },
    };
    const scheduleProvider = "codex_plus_official_schedule";
    const scheduleModel = `${scheduleProvider}:gpt-6-sol`;
    const catalog = { routeDescriptors: [{ routingSlug: account2Model, providerId: account2 }, { routingSlug: scheduleModel, providerId: scheduleProvider }] };
    const diagnostics: string[] = [];
    const providerChanges: string[][] = [];
    const create = new Function(
      "broker", "catalog", "diagnostics", "account2Model", "account2", "providerChanges",
      `const codexAppServerModelRequestPatchVersion = "test-mcp";
       const codexModelCatalog = catalog;
       const codexPlusBackendSettings = { officialExperience: { enabled: true } };
       const appServerModelRequestMethod = (method) => method;
        const codexRemoteSessionProviderRequestMethod = (method) => ["thread/fork", "thread/start", "turn/start", "thread/settings/update", "turn/settings/update", "thread/resume"].includes(method);
       const codexRemoteSessionProviderPatchEnabled = () => true;
       const loadBackendSettingsState = async () => true;
       const loadCodexModelCatalog = async () => catalog;
       const codexOfficialExperienceRouteDescriptor = (model) => catalog.routeDescriptors.find((item) => item.routingSlug === model);
       const normalizeCodexModelReasoningParams = (_method, params) => ({
         ...params, modelProvider: catalog.routeDescriptors.find(item => item.routingSlug === params.model)?.providerId || "openai",
       });
       const rememberCodexOfficialThreadModel = () => {};
       const notifyCodexOfficialThreadProviderChange = (from, to) => providerChanges.push([from, to]);
       const sendCodexPlusDiagnostic = (event) => diagnostics.push(event);
       ${renderer.slice(start, end)}
       ${providerSwitchHelperSource(renderer)}
       return patchCodexMcpRequestTransport({ K1t: broker });`,
    ) as (broker: object, catalog: object, diagnostics: string[], route: string, provider: string, providerChanges: string[][]) => boolean;
    assert.equal(create(broker, catalog, diagnostics, account2Model, account2, providerChanges), true);
    const dispatch = (id: number, method: string, params: Record<string, unknown>, type = "mcp-request") => broker.dispatchMessage(
      type, { type, hostId: "local", request: { id, method, params } },
    );
    const waitFor = async (predicate: () => boolean) => {
      for (let attempt = 0; attempt < 100 && !predicate(); attempt += 1) {
        await new Promise<void>((resolve) => setImmediate(resolve));
      }
      assert.ok(predicate(), "timed out waiting for broker message");
    };
    return { dispatch, sent, sentTypes, received, diagnostics, providerChanges, waitFor, forceResumeProvider: (value: string) => { forcedResumeProvider = value; } };
  }

  it("routes a new account 2 thread through its own provider", async () => {
    const runtime = await createBrokerRuntime();
    runtime.dispatch(1001, "thread/start", { model: account2Model });
    await runtime.waitFor(() => runtime.sent.length === 1);
    assert.equal(runtime.sent[0].method, "thread/start");
    assert.equal(runtime.sent[0].params.model, account2Model);
    assert.equal(runtime.sent[0].params.modelProvider, account2);
  });

  it("probes a scheduled chat without resuming history even when resume would hang", async () => {
    const provider = "codex_plus_official_schedule";
    const runtime = await createBrokerRuntime({ readProvider: provider, hangResume: true });
    runtime.dispatch(2101, "turn/start", { threadId: "thread-hot", model: `${provider}:gpt-6-sol`, input: [] });
    await runtime.waitFor(() => runtime.sent.some(request => request.method === "turn/start"));
    assert.deepEqual(runtime.sent.map(request => request.method), ["thread/read", "turn/start"]);
    assert.equal(runtime.sent[1].params.modelProvider, provider);
  });

  it("enters scheduling in the original chat and forwards private input exactly once", async () => {
    const provider = "codex_plus_official_schedule";
    const runtime = await createBrokerRuntime({ readProvider: "openai" });
    const input = [{ type: "text", text: "private input" }];
    runtime.dispatch(2102, "turn/start", { threadId: "thread-hot", model: `${provider}:gpt-6-sol`, input, cwd: "C:/work", approvalPolicy: "never", permissions: "my-profile", effort: "high" });
    await runtime.waitFor(() => runtime.sent.some(request => request.method === "turn/start"));
    assert.deepEqual(runtime.sent.map(request => request.method), ["thread/read", "thread/resume", "thread/unsubscribe", "thread/resume", "turn/start"]);
    const resume = runtime.sent[3].params;
    assert.equal(resume.modelProvider, provider);
    assert.equal(resume.cwd, "C:/work");
    assert.equal(resume.approvalPolicy, "never");
    assert.equal(resume.permissions, "my-profile");
    assert.equal(resume.sandbox, undefined);
    assert.equal(resume.input, undefined);
    assert.equal((resume.config as Record<string, unknown>).model_reasoning_effort, "high");
    assert.strictEqual(runtime.sent[4].params.input, input);
    assert.ok(runtime.sent.every(request => request.params.threadId === "thread-hot"));
  });

  it("switches scheduler model settings before a turn can use the old provider", async () => {
    const runtime = await createBrokerRuntime({ readProvider: "openai" });
    const provider = "codex_plus_official_schedule";
    runtime.dispatch(2201, "thread/settings/update", { threadId: "thread-hot", model: `${provider}:gpt-6-sol` });
    await runtime.waitFor(() => runtime.received.some(message => message.message?.id === 2201));
    assert.deepEqual(runtime.sent.map(request => request.method), ["thread/read", "thread/resume", "thread/unsubscribe", "thread/resume", "thread/settings/update"]);
    assert.equal(runtime.received.find(message => message.message?.id === 2201)?.message?.error, undefined);
    assert.equal(runtime.sent[4].params.modelProvider, provider);
  });

  it("never unloads an active turn or creates a chat when switching providers", async () => {
    for (const method of ["turn/start", "thread/settings/update", "turn/settings/update"]) {
      const runtime = await createBrokerRuntime({ readProvider: "openai", active: true });
      runtime.dispatch(2202, method, { threadId: "thread-hot", model: `${account2}:gpt-6-sol`, input: [] });
      await runtime.waitFor(() => runtime.received.some(message => message.message?.id === 2202 && message.message?.error));
      assert.ok(runtime.sent.every(request => ["thread/read", "thread/resume"].includes(request.method)));
    }
  });

  it("blocks an errored native instance before sending aliases or altering chat history", async () => {
    const runtime = await createBrokerRuntime({ readProvider: "openai", errorState: true });
    runtime.dispatch(2203, "thread/settings/update", { threadId: "thread-hot", model: "codex_plus_official_schedule:gpt-6-sol" });
    await runtime.waitFor(() => runtime.received.some(message => message.message?.id === 2203));
    const failure = runtime.received.find(message => message.message?.id === 2203)?.message?.error as { message?: string };
    assert.match(String(failure?.message), /错误状态.*重启 Codex.*无需创建副本/);
    assert.deepEqual(runtime.sent.map(request => request.method), ["thread/read", "thread/resume"]);
  });

  it("routes an account 2 prewarm thread through its own provider", async () => {
    const runtime = await createBrokerRuntime();
    runtime.dispatch(1001, "thread/start", { model: account2Model }, "thread-prewarm-start");
    await runtime.waitFor(() => runtime.sent.length === 1);
    assert.deepEqual(runtime.sentTypes, ["thread-prewarm-start"]);
    assert.equal(runtime.sent[0].params.modelProvider, account2);
  });

  it("routes a history preserving fork through the selected account provider", async () => {
    const runtime = await createBrokerRuntime();
    runtime.dispatch(1001, "thread/fork", {
      threadId: "legacy-thread", model: account2Model, excludeTurns: false,
    });
    await runtime.waitFor(() => runtime.sent.length === 1);
    assert.equal(runtime.sent[0].method, "thread/fork");
    assert.equal(runtime.sent[0].params.modelProvider, account2);
    assert.equal(runtime.sent[0].params.excludeTurns, false);
  });

  it("rejects an unknown extended model without forwarding a thread", async () => {
    const runtime = await createBrokerRuntime();
    runtime.dispatch(1001, "thread/start", { model: "unknown:outside-catalog" });
    await runtime.waitFor(() => runtime.received.some((message) => message.message?.id === 1001));
    assert.deepEqual(runtime.sent, []);
    const failure = runtime.received.find((message) => message.message?.id === 1001);
    assert.equal(failure?.hostId, "local");
    assert.match(String((failure?.message?.error as { message?: string })?.message), /没有可用的来源路由/);
  });

  it("unsubscribes and resumes before switching account 1 to account 2 and back", async () => {
    const runtime = await createBrokerRuntime();
    runtime.dispatch(1001, "thread/start", { model: "gpt-6-sol" });
    await runtime.waitFor(() => runtime.received.some((message) => message.message?.id === 1001));
    runtime.dispatch(1002, "turn/start", { threadId: "thread-hot", model: account2Model, input: [] });
    await runtime.waitFor(() => runtime.sent.some((request) => request.method === "turn/start"));
    assert.deepEqual(runtime.sent.map((request) => request.method), [
      "thread/start", "thread/resume", "thread/unsubscribe", "thread/resume", "turn/start",
    ]);
    assert.equal(runtime.sent[3].params.modelProvider, account2);
    assert.equal(runtime.sent[4].params.modelProvider, account2);

    runtime.dispatch(1003, "turn/start", { threadId: "thread-hot", model: "gpt-6-sol", input: [] });
    await runtime.waitFor(() => runtime.sent.filter((request) => request.method === "turn/start").length === 2);
    assert.deepEqual(runtime.sent.slice(5).map((request) => request.method), [
      "thread/resume", "thread/unsubscribe", "thread/resume", "turn/start",
    ]);
    assert.equal(runtime.sent[7].params.modelProvider, "openai");
    assert.equal(runtime.sent[8].params.modelProvider, "openai");
    assert.deepEqual(runtime.providerChanges, [["openai", account2], [account2, "openai"]]);
  });

  it("fails closed without creating copies when a provider switch is ignored", async () => {
    const runtime = await createBrokerRuntime();
    runtime.dispatch(1001, "thread/start", { model: "gpt-6-sol" });
    await runtime.waitFor(() => runtime.received.some((message) => message.message?.id === 1001));
    runtime.forceResumeProvider("openai");
    runtime.dispatch(1002, "turn/start", { threadId: "thread-hot", model: account2Model, input: [] });
    await runtime.waitFor(() => runtime.received.some((message) => message.message?.id === 1002 && message.message?.error));
    assert.deepEqual(runtime.sent.map((request) => request.method), [
      "thread/start", "thread/resume", "thread/unsubscribe", "thread/resume",
    ]);
    const failure = runtime.received.find((message) => message.message?.id === 1002);
    assert.ok(failure?.message?.error);
    assert.match(String((failure?.message?.error as { message?: string })?.message), /不会创建副本/);
    assert.ok(runtime.diagnostics.includes("official_mcp_route_failed"));
    runtime.dispatch(1003, "turn/start", { threadId: "thread-hot", model: account2Model, input: [] });
    await runtime.waitFor(() => runtime.received.some((message) => message.message?.id === 1003 && message.message?.error));
    assert.equal(runtime.sent.filter((request) => request.method === "thread/fork").length, 0);
    assert.equal(runtime.sent.filter((request) => request.method === "turn/start").length, 0);
    const retryFailure = runtime.received.find((message) => message.message?.id === 1003);
    assert.equal((retryFailure?.message?.error as { message?: string })?.message,
      (failure?.message?.error as { message?: string })?.message);
  });
});

describe("quota-stop native resume", () => {
  function quotaResumePureRuntime(renderer: string) {
    const start = renderer.indexOf("  function codexQuotaResumeTextHasMarker(");
    const end = renderer.indexOf("\n  function codexQuotaResumeTurnParamsFromMessage", start);
    assert.ok(start >= 0 && end > start);
    const source = renderer.slice(start, end);
    const create = new Function(
      "validThreadScrollSessionKey",
      "currentSessionRef",
      "codexPlusSettings",
      "window",
      `const codexQuotaResumeVersion = "test";\n${source}\nreturn {
        codexQuotaResumeValueHasMarker,
        codexQuotaResumeTurnTemplate,
        rememberCodexQuotaResumeTurnRequest,
        mergeCodexQuotaResumeThreadSettings,
      };`,
    ) as (
      validThreadId: (value: unknown) => string,
      currentSession: () => { session_id: string },
      settings: () => { quotaResume: boolean },
      runtimeWindow: Record<string, unknown>,
    ) => {
      codexQuotaResumeValueHasMarker: (value: unknown) => boolean;
      codexQuotaResumeTurnTemplate: (params: Record<string, unknown>, threadIdHint?: string) => Record<string, unknown> | null;
      rememberCodexQuotaResumeTurnRequest: (
        client: unknown,
        params: Record<string, unknown>,
        threadIdHint?: string,
      ) => Record<string, unknown> | null;
      mergeCodexQuotaResumeThreadSettings: (
        client: unknown,
        params: Record<string, unknown>,
        threadIdHint?: string,
      ) => Record<string, unknown> | null;
    };
    const runtimeWindow: Record<string, unknown> = {};
    return create(
      (value) => typeof value === "string" ? value.replace(/^local:/, "") : "",
      () => ({ session_id: "thread-current" }),
      () => ({ quotaResume: true }),
      runtimeWindow,
    );
  }

  it("recognizes explicit quota failures without treating ordinary model errors as quota stops", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = quotaResumePureRuntime(renderer);

    assert.equal(runtime.codexQuotaResumeValueHasMarker({ errorInfo: "usageLimitExceeded" }), true);
    assert.equal(runtime.codexQuotaResumeValueHasMarker({ error: { code: "insufficient_quota" } }), true);
    assert.equal(runtime.codexQuotaResumeValueHasMarker({ statusCode: 429 }), true);
    assert.equal(runtime.codexQuotaResumeValueHasMarker(new Error("HTTP 429 Too Many Requests")), true);
    assert.equal(runtime.codexQuotaResumeValueHasMarker({ message: "请求过于频繁，请稍后重试" }), true);
    assert.equal(runtime.codexQuotaResumeValueHasMarker(new Error("credits exhausted")), true);
    assert.equal(runtime.codexQuotaResumeValueHasMarker({ code: "internal_server_error" }), false);
    assert.equal(runtime.codexQuotaResumeValueHasMarker({ message: "selected model is at capacity" }), false);
  });

  it("builds an empty native resume turn while preserving execution context and external provider", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = quotaResumePureRuntime(renderer);
    const template = runtime.codexQuotaResumeTurnTemplate({
      threadId: "local:thread-ecnu",
      input: [{ type: "text", text: "secret prompt" }],
      cwd: "C:/work",
      approvalPolicy: "on-request",
      sandboxPolicy: { type: "workspace-write" },
      model: "ecnu-reasoner",
      modelProvider: "deepseek",
      effort: "high",
      collaborationMode: "default",
      unrelated: "drop-me",
    });

    assert.deepEqual(template, {
      threadId: "thread-ecnu",
      cwd: "C:/work",
      approvalPolicy: "on-request",
      sandboxPolicy: { type: "workspace-write" },
      model: "ecnu-reasoner",
      modelProvider: "deepseek",
      effort: "high",
      collaborationMode: "default",
    });
    assert.match(renderer, /input: \[\],[\s\S]*turnTrigger: "resume_interrupted_task"/);
    assert.match(renderer, /client\.sendRequest\("turn\/start", params\)/);
    assert.doesNotMatch(renderer, /setText\(["']继续["']\)/);
    assert.match(renderer, /send message\|send\|add to queue\|发送消息\|发送/);
  });

  it("merges model and reasoning changes made after a quota stop into the resume template", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const runtime = quotaResumePureRuntime(renderer);

    runtime.rememberCodexQuotaResumeTurnRequest(null, {
      threadId: "thread-ecnu",
      input: [{ type: "text", text: "original prompt" }],
      cwd: "C:/work",
      model: "deepseek-v4",
      modelProvider: "deepseek",
      reasoningEffort: "high",
    });
    const template = runtime.mergeCodexQuotaResumeThreadSettings(null, {
      threadId: "thread-ecnu",
      model: "deepseek-v4-0528",
      reasoningEffort: "xhigh",
    });

    assert.deepEqual(template, {
      threadId: "thread-ecnu",
      cwd: "C:/work",
      model: "deepseek-v4-0528",
      modelProvider: "deepseek",
      reasoningEffort: "xhigh",
    });
    assert.match(renderer, /quotaResumeTarget\?\.requestMethod === "turn\/start"/);
    assert.match(renderer, /mergeCodexQuotaResumeThreadSettings\([\s\S]*quotaResumeTarget\.params/);
  });

  it("is exposed as a default-on Codex enhancement toggle", async () => {
    const app = await readFile(new URL("./App.tsx", import.meta.url), "utf8");
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(app, /codexAppQuotaResume: true,/);
    assert.match(app, /setEnhanceFlag\("codexAppQuotaResume", value\)/);
    assert.match(renderer, /quotaResume: true,/);
    assert.match(renderer, /quotaResume: "codexAppQuotaResume"/);
    assert.match(renderer, /codex-plus-quota-resume-button/);
  });
});

describe("Stepwise generation mode contracts", () => {
  it("exposes automatic and manual generation in manager settings", async () => {
    const app = await readFile(new URL("./App.tsx", import.meta.url), "utf8");
    const renderer = await readFile(
      new URL("../../../assets/inject/renderer-inject.js", import.meta.url),
      "utf8",
    );

    assert.match(app, /type StepwiseGenerationMode = "auto" \| "manual";/);
    assert.match(app, /type StepwiseProtocol = "auto" \| "chat_completions" \| "responses" \| "anthropic_messages";/);
    assert.match(app, /codexAppStepwiseProtocol: "chat_completions",/);
    assert.match(app, /codexAppStepwiseGenerationMode: "auto",/);
    assert.match(app, /codexAppAnswerOutlineEnabled: false,/);
    assert.match(renderer, /answerOutline: false,/);
    assert.match(app, /<Field label=\{t\("模式"\)\}>/);
    assert.match(app, /\{ value: "auto", label: t\("自动生成"\) \}/);
    assert.match(app, /\{ value: "manual", label: t\("手动刷新"\) \}/);
    assert.match(app, /\{ value: "auto", label: t\("自动兼容"\) \}/);
    assert.match(app, /\{ value: "anthropic_messages", label: "Anthropic Messages" \}/);
    assert.match(app, /function normalizeStepwiseProtocol\(/);
    assert.match(app, /return value === "manual" \? "manual" : "auto";/);
  });

  it("defers manual generation until refresh and rejects stale mode results", async () => {
    const stepwise = await readStepwiseSource();

    assert.match(stepwise, /const manualRequestPending = generationMode === "manual"/);
    assert.match(stepwise, /if \(generationMode === "manual" && !manualResultVisible && !manualRequestPending\)/);
    assert.match(stepwise, /\} else if \(manualRequestPending\) \{/);
    assert.match(stepwise, /state\.bridgeStatus = "manual-ready";/);
    assert.match(
      stepwise,
      /requestBridgeStepwise\(bridgeKey, userText, assistantText, generationMode, \{ userInitiated: true \}\)/,
    );
    assert.match(stepwise, /requestBridgeStepwise\(bridgeKey, userText, assistantText, "auto"\)/);
    assert.match(stepwise, /normalizedMode === "manual" && options\.userInitiated !== true/);
    assert.match(stepwise, /stepwiseGenerationMode\(\) === normalizedMode/);
    assert.match(stepwise, /state\.bridgePendingMode === normalizedMode/);
    assert.match(stepwise, /Object\.prototype\.hasOwnProperty\.call\(normalizedPatch, "generationMode"\)/);
    assert.match(stepwise, /if \(!Object\.prototype\.hasOwnProperty\.call\(nextSettings, "generationMode"\)\)/);
    assert.match(stepwise, /nextSettings\.generationMode = stepwiseGenerationMode\(\);/);
    const appearanceStart = stepwise.indexOf("function appearanceSettingsHtml()");
    const settingsStart = stepwise.indexOf("function settingsHtml()", appearanceStart);
    const appearanceMarkup = stepwise.slice(appearanceStart, settingsStart);
    assert.doesNotMatch(appearanceMarkup, /data-action="generation-mode"/);
    const footerStart = stepwise.indexOf('<div class="csw-runtime-grid"', settingsStart);
    const generationModeControl = stepwise.indexOf('data-action="generation-mode"', footerStart);
    const promptClickControl = stepwise.indexOf('data-action="prompt-click-mode"', footerStart);
    assert.ok(footerStart >= 0 && generationModeControl > footerStart && promptClickControl > generationModeControl);
    assert.match(stepwise, /<span class="csw-metric-label">模式<\/span>/);
    assert.match(stepwise, /return normalizeGenerationMode\(value\) === "manual" \? "手动刷新" : "自动生成";/);
    assert.match(stepwise, /return setGenerationMode\(nextGenerationMode\(\)\);/);
    assert.match(stepwise, /return writePromptClickMode\(nextPromptClickMode\(\)\);/);
    assert.match(stepwise, /button\.csw-metric-action\s*\{[^}]*padding:\s*0;/s);
    assert.match(
      stepwise,
      /\.csw-click-mode,[\s\S]*?\.csw-generation-mode\s*\{[^}]*min-width:\s*0;/,
    );
    assert.match(stepwise, /\.csw-generation-mode\s*\{[^}]*white-space:\s*nowrap;/s);
    assert.match(
      stepwise,
      /\.csw-metric-value,[\s\S]*?\.csw-metric-action\s*\{[^}]*overflow:\s*visible;[^}]*text-overflow:\s*clip;/,
    );
    assert.match(
      stepwise,
      /\.csw-metric-value,[\s\S]*?\.csw-generation-mode \.csw-metric-action\s*\{[^}]*white-space:\s*nowrap;/,
    );
    assert.match(
      stepwise,
      /\.csw-click-mode \.csw-metric-action\s*\{[^}]*overflow-wrap:\s*anywhere;[^}]*white-space:\s*normal;/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 440px\)[\s\S]*?\.csw-settings-footer\s*\{[^}]*display:\s*grid;[^}]*grid-template-columns:\s*minmax\(max-content, 1fr\) auto;/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 440px\)[\s\S]*?\.csw-runtime-grid\s*\{[^}]*grid-template-columns:\s*minmax\(0, max-content\) minmax\(0, 1fr\);[^}]*width:\s*100%;/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 440px\)[\s\S]*?\.csw-command-button\s*\{[^}]*flex:\s*0 0 30px;[^}]*height:\s*30px;[^}]*padding:\s*0;[^}]*width:\s*30px;/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 440px\)[\s\S]*?\.csw-command-label\s*\{[^}]*display:\s*none;/,
    );
    assert.match(
      stepwise,
      /class="csw-command-button"[^>]*title="\$\{escapeAttr\(title\)\}"[^>]*aria-label="\$\{escapeAttr\(title\)\}"/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 360px\)[\s\S]*?\.csw-runtime-grid\s*\{[^}]*grid-template-columns:\s*minmax\(0, 1fr\);/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 360px\)[\s\S]*?\.csw-generation-mode,[\s\S]*?\.csw-click-mode\s*\{[^}]*width:\s*100%;/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 320px\)[\s\S]*?\.csw-metric\s*\{[^}]*white-space:\s*nowrap;/,
    );
    assert.match(
      stepwise,
      /@container csw-panel \(max-width: 320px\)[\s\S]*?\.csw-command-button\s*\{[^}]*flex:\s*0 0 28px;[^}]*height:\s*28px;[^}]*width:\s*28px;/,
    );
    const toggleStart = stepwise.indexOf("async function setGenerationMode(value)");
    const immediateCancel = stepwise.indexOf(
      "applyRuntimeSettings({ ...(state.settings || {}), generationMode: nextMode });",
      toggleStart,
    );
    const settingsSave = stepwise.indexOf('bridgeCall("/settings/set", {', toggleStart);
    assert.ok(toggleStart >= 0 && immediateCancel > toggleStart && settingsSave > immediateCancel);

    const progressStart = stepwise.indexOf("function nextProgressState()");
    const manualProgressGuard = stepwise.indexOf('if (stepwiseGenerationMode() === "manual") return null;', progressStart);
    const localScanProgress = stepwise.indexOf('state.scanStatus === "assistant-changed"', progressStart);
    assert.ok(progressStart >= 0 && manualProgressGuard > progressStart && localScanProgress > manualProgressGuard);
    assert.match(stepwise, /title: "当前为手动模式"/);
    assert.doesNotMatch(stepwise, /title: "待生成"/);

    const outlineExpressionStart = stepwise.indexOf("function usesOutlineExpression(");
    const outlineExpressionEnd = stepwise.indexOf("function resolveFabExpression(", outlineExpressionStart);
    const outlineExpression = stepwise.slice(outlineExpressionStart, outlineExpressionEnd);
    assert.match(outlineExpression, /stepwiseWaitingForManualRefresh\(\)/);

    const runtimePresentationStart = stepwise.indexOf("function settingsRuntimePresentation(");
    const runtimePresentationEnd = stepwise.indexOf("function settingsCommandHtml(", runtimePresentationStart);
    const runtimePresentation = stepwise.slice(runtimePresentationStart, runtimePresentationEnd);
    assert.match(runtimePresentation, /!outlineExpression && stepwiseWaitingForManualRefresh\(settings\)/);

    const scanStart = stepwise.indexOf("function scan(");
    const outlineRefresh = stepwise.indexOf("void refreshOutline({ message, assistantHash: hash });", scanStart);
    const manualScanBranch = stepwise.indexOf('if (generationMode === "manual" && !manualResultVisible && !manualRequestPending)', scanStart);
    const cachedScanBranch = stepwise.indexOf('else if (hasSuccessfulCache)', scanStart);
    const automaticGenerate = stepwise.indexOf('requestBridgeStepwise(bridgeKey, userText, assistantText, "auto")', scanStart);
    assert.ok(scanStart >= 0 && outlineRefresh > scanStart && manualScanBranch > outlineRefresh);
    assert.ok(cachedScanBranch > manualScanBranch);
    assert.ok(automaticGenerate > cachedScanBranch);
  });

  it("keeps feature tab order draggable and persistent", async () => {
    const stepwise = await readStepwiseSource();

    assert.match(stepwise, /const VIEW_ORDER_KEY = "codex-stepwise-view-order-v1";/);
    assert.match(stepwise, /function normalizeViewOrder\(value\)/);
    assert.match(stepwise, /function persistViewOrder\(order\)/);
    assert.match(stepwise, /function installViewTabReorder\(\)/);
    assert.match(stepwise, /data-reorderable="true"/);
    assert.match(stepwise, /state\.viewReorderCleanup\?\.\(\)/);
    assert.match(stepwise, /syncViewTabSelection\(state\.activeTab, true\);/);
    assert.match(stepwise, /dataset\.codexStepwiseStyleVersion === SCRIPT_VERSION/);
    assert.match(stepwise, /style\.dataset\.codexStepwiseStyleVersion = SCRIPT_VERSION/);
    assert.match(stepwise, /VIEW_SLIDE_MS = 240/);
    assert.match(stepwise, /VIEW_INDICATOR_MS = 220/);
  });

  it("keeps Stepwise Manager controls at one height", async () => {
    const styles = await readFile(new URL("./styles.css", import.meta.url), "utf8");

    assert.match(styles, /\.stepwise-settings-block\s*\{[\s\S]*?--stepwise-control-height:\s*40px;/);
    assert.match(
      styles,
      /\.stepwise-settings-block input[^,]*,[\s\S]*?\.stepwise-settings-block \.app-select-trigger,[\s\S]*?\.stepwise-settings-block \.field-select,[\s\S]*?\.stepwise-settings-block \.select-input\s*\{[\s\S]*?height:\s*var\(--stepwise-control-height\);[\s\S]*?min-height:\s*var\(--stepwise-control-height\);/,
    );
  });
});

// issue #2256/#2255：app-server model request patch 的 miss 熔断以前被 provider
// 重试路径提前 return 绕过，失败变成 250ms 无限重试（每轮全量 fetch 全部 app asset）。
describe("renderer injection app-server model request patch", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  interface AppServerPatchHarness {
    install: () => void;
    sweeps: () => number;
    diagnostics: () => string[];
    settle: () => Promise<void>;
  }

  function appServerPatchRuntime(renderer: string, patchSucceeds: boolean): AppServerPatchHarness {
    const start = renderer.indexOf("  const appServerModelRequestPatchMaxMisses = ");
    const end = renderer.indexOf("\n  function ensureCodexModelWhitelistInstalls(", start);
    assert.ok(start >= 0 && end > start, "app-server model request patch block not found");
    const source = renderer.slice(start, end);

    let sweeps = 0;
    let pending: Array<() => void> = [];
    const diagnostics: string[] = [];
    const timers: Array<number> = [];
    const fakeWindow: Record<string, unknown> = {
      setTimeout: ((fn: () => void) => {
        timers.push(0);
        pending.push(fn);
        return 0;
      }) as unknown,
      clearTimeout: () => {},
    };

    const factory = new Function(
      "window",
      "codexAppServerModelRequestPatchVersion",
      "codexRemoteSessionProviderPatchEnabled",
      "loadAppServerRequestCandidates",
      "patchAppServerModelRequestClient",
      "sendCodexPlusDiagnostic",
      "Date",
      "patchCodexUsageHttpTransport",
      "patchCodexVscodeRequestTransport",
      "patchCodexMcpRequestTransport",
      "codexPlusBackendSettings",
      "codexPlusSettings",
      "clearTimeout",
      `${source}\nreturn installAppServerModelRequestPatch;`,
    );

    const install = factory(
      fakeWindow,
      1,
      // provider patch 开关两态都要测：以前 enabled 时走提前 return 绕过熔断。
      () => true,
      () =>
        new Promise((resolve) => {
          sweeps += 1;
          pending.push(() => resolve({ modules: [{}], candidates: [{}], sources: [], discovery: "fallback" }));
        }),
      () => patchSucceeds,
      (event: string) => diagnostics.push(event),
      Date,
      () => false,
      () => false,
      () => false,
      { officialExperience: { enabled: false } },
      () => ({ quotaResume: true, serviceTierControls: false }),
      () => {},
    ) as () => void;

    const settle = async () => {
      // 重试定时器是挂起的回调：排空 sweep 再触发到期的 retry，直到没有新定时器。
      for (let round = 0; round < 32; round += 1) {
        if (!pending.length) break;
        const flushSweeps = pending;
        pending = [];
        flushSweeps.forEach((resolve) => resolve());
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      }
    };

    return { install, sweeps: () => sweeps, diagnostics: () => diagnostics, settle };
  }

  it("does not start a new sweep while the previous one is still running", async () => {
    const harness = appServerPatchRuntime(await readFile(rendererPath, "utf8"), false);

    for (let i = 0; i < 20; i += 1) harness.install();

    assert.equal(harness.sweeps(), 1);
    await harness.settle();
  });

  it("stops retrying via the provider path once maxMisses is reached", async () => {
    const harness = appServerPatchRuntime(await readFile(rendererPath, "utf8"), false);

    // 反复 install + settle，让每轮 miss 走完 provider 重试调度。
    for (let i = 0; i < 40; i += 1) {
      harness.install();
      await harness.settle();
    }

    // 关键回归断言：以前 provider 路径无限重试（40 轮 = 40 次 sweep），
    // 现在到 maxMisses(8) 就熔断停手。
    assert.equal(harness.sweeps(), 8);
    assert.equal(harness.diagnostics().filter((e) => e === "model_app_server_request_patch_not_found").length, 1);
    assert.deepEqual(harness.diagnostics().at(-1), "model_app_server_request_patch_skipped");
    const settled = harness.sweeps();
    harness.install();
    await harness.settle();
    assert.equal(harness.sweeps(), settled);
  });

  it("keeps working normally when the patch actually lands", async () => {
    const harness = appServerPatchRuntime(await readFile(rendererPath, "utf8"), true);

    harness.install();
    await harness.settle();
    for (let i = 0; i < 10; i += 1) harness.install();

    assert.equal(harness.sweeps(), 1);
    assert.deepEqual(harness.diagnostics(), ["model_app_server_request_patch_installed"]);
  });
});

// renderer-inject.js 现在是「分片源码 + 生成产物」：真正的源码在
// assets/inject/renderer-inject/*.js，产物是拼回去的单体文件，供 assets.rs 的
// include_str! 和本文件的大量按路径断言消费。
// 这组用例防止有人只改产物、不回改分片，让两边悄悄分叉。
describe("renderer inject 分片与产物一致", () => {
  const fragmentDir = new URL("../../../assets/inject/renderer-inject/", import.meta.url);
  const artifactPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  async function assembleFromFragments(): Promise<{ source: string; names: string[] }> {
    const manifest = JSON.parse(
      await readFile(new URL("manifest.json", fragmentDir), "utf8"),
    ) as { fragments: Array<{ name: string; description?: string }> };
    assert.ok(
      Array.isArray(manifest.fragments) && manifest.fragments.length > 0,
      "manifest.json 必须有 fragments",
    );
    const bodies = await Promise.all(
      manifest.fragments.map(async (fragment) => {
        assert.ok(fragment.name, "分片必须带 name");
        const body = await readFile(new URL(fragment.name, fragmentDir), "utf8");
        // 每片以换行收尾，直接相接才能还原原始行结构；漏了会让两处声明粘连。
        assert.ok(body.endsWith("\n"), `分片 ${fragment.name} 未以换行结尾`);
        return body;
      }),
    );
    return { source: bodies.join(""), names: manifest.fragments.map((f) => f.name) };
  }

  it("分片按 manifest 顺序拼接后与产物逐字节一致", async () => {
    const { source, names } = await assembleFromFragments();
    const artifact = await readFile(artifactPath, "utf8");
    assert.equal(
      source,
      artifact,
      `产物与分片不一致（分片 ${names.length} 个）。` +
        "改分片后请跑 node scripts/assemble-renderer-inject.mjs 重新组装。",
    );
  });

  it("拼接结果是完整可解析的单个 IIFE", async () => {
    const { source } = await assembleFromFragments();
    assert.ok(source.startsWith("(() => {\n"), "必须以 IIFE 开头");
    assert.ok(source.includes("\n})();\n"), "必须包含主 IIFE 的收尾");
    // 语法解析（不执行）能抓出分片切坏造成的括号不匹配。
    assert.doesNotThrow(() => new Function(source));
  });

  it("测试辅助依赖的锚点仍落在同一个分片里且顺序不变", async () => {
    // installRendererStyle() 用 indexOf 划区间，靠这两个标记定位。
    // 它们一旦被拆到不同分片、或前后顺序反转，helper 会静默取到空区间。
    const { source } = await assembleFromFragments();
    const start = source.indexOf("  function installStyle()");
    const end = source.indexOf("\n  function defaultCodexPlusSettings", start);
    assert.ok(start >= 0, "找不到 installStyle 锚点");
    assert.ok(end > start, "defaultCodexPlusSettings 必须排在 installStyle 之后");
  });

  it("拓展列表渲染不裸取 entry.item", async () => {
    // 已安装条目的形状是 { kind, key, name, meta, enabled, script }，没有 item；
    // 只有市场条目才有。裸取 entry.item.description 会在列表里只要有任意一个
    // 已安装脚本时就抛 TypeError，整块渲染中断，左面板停在「正在读取…」占位。
    // 已安装条目要走 codexPlusExtensionMarketItem(entry) 按 market_id 回查。
    const { source } = await assembleFromFragments();
    const openingTag = source.indexOf("  function renderCodexPlusExtensionsNav()");
    const closingTag = source.indexOf("\n  /** 左面板内容变了就整块重绘", openingTag);
    assert.ok(openingTag >= 0, "找不到 renderCodexPlusExtensionsNav");
    assert.ok(closingTag > openingTag, "找不到函数结束位置");
    const body = source.slice(openingTag, closingTag);
    assert.ok(
      !/entry\.item\s*\./.test(body),
      "itemHtml 里出现了不带可选链的 entry.item.*，会在已安装条目上抛 TypeError",
    );
    assert.ok(
      body.includes("codexPlusExtensionMarketItem(entry)"),
      "itemHtml 应该用 codexPlusExtensionMarketItem(entry) 解析图标与简介来源",
    );
  });
});

/**
 * 拓展接口层的契约测试。
 *
 * 这些测试把 01-registry.js 与 91-extension-api.js 的定义抽出来单独执行，
 * 不依赖真实 DOM——注册中心本身刻意不读 DOM，正是为了让它可以这样被验证。
 */
describe("拓展注册中心", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  interface RegistryHarness {
    registry: { rowActions: Map<string, unknown>; navEntries: Map<string, unknown>; pages: Map<string, unknown>; menuItems: Map<string, unknown> };
    register: (kind: string, reg: Map<string, unknown>, id: string, def: unknown, key: string) => () => void;
    registerSelector: (selector: string) => boolean;
    extensionSelector: () => string;
    isExtensionNode: (node: unknown) => boolean;
    items: (reg: Map<string, unknown>) => Array<Record<string, unknown>>;
    runCallback: (key: string, label: string, cb: () => unknown) => { ok: boolean; value?: unknown; error?: string };
    failures: () => Array<{ script_key: string; message: string }>;
    registryLog: () => unknown[];
  }

  async function registryRuntime(): Promise<RegistryHarness> {
    // 直接读注册中心分片本身：它刻意不读 DOM、不依赖 prelude 常量，所以能独立执行。
    // 不要从产物里 indexOf 切片——注册中心与接口层之间隔着十来个分片，区间会失控。
    const source = await readFile(
      new URL("../../../assets/inject/renderer-inject/01-registry.js", import.meta.url),
      "utf8",
    );

    const windowValue: Record<string, unknown> = {};
    const documentValue = {
      // registerCodexPlusExtensionSelector 用空 fragment 验证选择器语法，
      // 非法选择器会抛错，模拟这个方法就足以覆盖合法性判定。
      createDocumentFragment: () => ({
        querySelector: (selector: string) => {
          if (/[<>]/.test(selector)) throw new Error("invalid selector");
          return null;
        },
      }),
    };
    const factory = new Function(
      "window",
      "document",
      `${source}
return { codexPlusRegistry, registerCodexPlusExtension, registerCodexPlusExtensionSelector,
  codexPlusExtensionSelector, isCodexPlusExtensionNode, codexPlusExtensionItems,
  runCodexPlusExtensionCallback };`,
    );
    const api = factory(windowValue, documentValue) as Record<string, unknown>;
    return {
      registry: api.codexPlusRegistry as RegistryHarness["registry"],
      register: api.registerCodexPlusExtension as RegistryHarness["register"],
      registerSelector: api.registerCodexPlusExtensionSelector as RegistryHarness["registerSelector"],
      extensionSelector: api.codexPlusExtensionSelector as RegistryHarness["extensionSelector"],
      isExtensionNode: api.isCodexPlusExtensionNode as RegistryHarness["isExtensionNode"],
      items: api.codexPlusExtensionItems as RegistryHarness["items"],
      runCallback: api.runCodexPlusExtensionCallback as RegistryHarness["runCallback"],
      failures: () => (windowValue.__codexPlusExtensionFailures || []) as Array<{ script_key: string; message: string }>,
      registryLog: () => (windowValue.__codexPlusRegistryLog || []) as unknown[],
    };
  }

  it("第三方项从 1000 起排，不会插到内置项前面", async () => {
    const runtime = await registryRuntime();
    runtime.register("rowAction", runtime.registry.rowActions, "builtin:a", { order: 10, onActivate() {} }, "builtin");
    runtime.register("rowAction", runtime.registry.rowActions, "ext:b", { order: 5, onActivate() {} }, "user:x.js");
    const items = runtime.items(runtime.registry.rowActions);
    // 传入的 5 被抬到 1000：内置项永远排在拓展项之前。
    assert.deepEqual(items.map((item) => item.id), ["builtin:a", "ext:b"]);
    assert.equal(items[1].order, 1000);
  });

  it("按脚本配额限制注册数量，超出时抛错", async () => {
    const runtime = await registryRuntime();
    for (let index = 0; index < 16; index += 1) {
      runtime.register("navEntry", runtime.registry.navEntries, `ext:${index}`, { onActivate() {} }, "user:noisy.js");
    }
    assert.throws(
      () => runtime.register("navEntry", runtime.registry.navEntries, "ext:overflow", { onActivate() {} }, "user:noisy.js"),
      /最多注册 16 项/,
    );
    // 另一个脚本不受影响。
    assert.doesNotThrow(
      () => runtime.register("navEntry", runtime.registry.navEntries, "ext:other", { onActivate() {} }, "user:other.js"),
    );
  });

  it("拒绝重复 id 与缺少回调的定义", async () => {
    const runtime = await registryRuntime();
    runtime.register("page", runtime.registry.pages, "ext:p", { render() {} }, "user:x.js");
    assert.throws(
      () => runtime.register("page", runtime.registry.pages, "ext:p", { render() {} }, "user:x.js"),
      /已被占用/,
    );
    assert.throws(
      () => runtime.register("page", runtime.registry.pages, "ext:empty", {}, "user:x.js"),
      /必须提供 render \/ onActivate/,
    );
  });

  it("菜单项的开关形态（只给 onChange）不被校验误拒", async () => {
    const runtime = await registryRuntime();
    // 开关形态没有 render / onActivate，早期实现会把它当成非法定义拒掉。
    assert.doesNotThrow(
      () => runtime.register("menuItem", runtime.registry.menuItems, "ext:toggle", { onChange() {} }, "user:x.js"),
    );
    assert.doesNotThrow(
      () => runtime.register("page", runtime.registry.pages, "ext:cleanup-only", { onCleanup() {} }, "user:x.js"),
    );
  });

  it("dispose 只移除自己注册的那一项", async () => {
    const runtime = await registryRuntime();
    const dispose = runtime.register("page", runtime.registry.pages, "ext:first", { render() {} }, "user:x.js");
    runtime.register("page", runtime.registry.pages, "ext:second", { render() {} }, "user:y.js");
    dispose();
    assert.deepEqual([...runtime.registry.pages.keys()], ["ext:second"]);
    // 重复 dispose 不应误删后来者。
    dispose();
    assert.deepEqual([...runtime.registry.pages.keys()], ["ext:second"]);
  });

  it("选择器登记拒绝非法语法，避免 closest() 在每次 mutation 上抛错", async () => {
    const runtime = await registryRuntime();
    assert.equal(runtime.registerSelector('[data-codex-plus-ext="a"]'), true);
    assert.match(runtime.extensionSelector(), /data-codex-plus-ext/);
    assert.equal(runtime.registerSelector("div << p"), false);
    assert.equal(runtime.registerSelector(""), false);
  });

  it("回调抛错时记进该脚本的失败通道，不向调用方抛出", async () => {
    const runtime = await registryRuntime();
    const outcome = runtime.runCallback("user:broken.js", "rowAction.onActivate", () => {
      throw new Error("boom");
    });
    assert.equal(outcome.ok, false);
    assert.match(String(outcome.error), /boom/);
    assert.equal(runtime.failures().length, 1);
    assert.equal(runtime.failures()[0].script_key, "user:broken.js");
  });
});

/**
 * 对外接口层的护栏测试。
 *
 * 接口层挂在 window.codexPlus 上、面向第三方脚本，所以「哪些能力被开放」必须
 * 有测试守着——新增路由时默认不开放，漏网才是 bug。
 */
describe("拓展接口层", () => {
  const apiFragmentPath = new URL(
    "../../../assets/inject/renderer-inject/91-extension-api.js",
    import.meta.url,
  );
  const artifactPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  async function readRouteWhitelist(): Promise<string[]> {
    const source = await readFile(apiFragmentPath, "utf8");
    const start = source.indexOf("const codexPlusExtensionRoutes = new Set([");
    const end = source.indexOf("]);", start);
    assert.ok(start >= 0 && end > start, "找不到路由白名单");
    const body = source.slice(start, end);
    return Array.from(body.matchAll(/"([^"]+)"/g), ([, route]) => route);
  }

  it("路由白名单只放只读能力，不含设置写入与远端控制", async () => {
    const routes = await readRouteWhitelist();
    assert.ok(routes.length > 0, "白名单不能为空");
    // 这些路由一旦开放，第三方脚本就能改用户配置或操作远端，必须显式决策后才能加。
    for (const forbidden of ["/settings/set", "/settings/get", "/delete", "/undo", "/share/create"]) {
      assert.ok(!routes.includes(forbidden), `${forbidden} 不应默认开放给拓展`);
    }
    assert.ok(routes.includes("/diagnostics/log"), "诊断上报应保持开放");
  });

  it("接口对象在 IIFE 收尾前挂载", async () => {
    const renderer = await readFile(artifactPath, "utf8");
    const mount = renderer.indexOf("window.codexPlus = buildCodexPlusExtensionApi();");
    const tail = renderer.indexOf("\n})();\n");
    assert.ok(mount >= 0, "找不到 window.codexPlus 的挂载点");
    // 挂载必须在主 IIFE 结束之前，否则闭包里的函数已经不可达。
    assert.ok(mount < tail, "挂载点必须位于 IIFE 收尾之前");
  });
});

/**
 * 菜单项的接入方式护栏。
 *
 * 菜单的接入点是「在 home 面板模板末尾追加一块」，不是把内置的一百多行模板
 * 拆成数据结构——后者会动到内置 UI 主干。这些断言守住这个决定，以及点击
 * 委托的分支顺序。
 */
describe("拓展菜单项", () => {
  const settingsPath = new URL(
    "../../../assets/inject/renderer-inject/40-backend-settings.js",
    import.meta.url,
  );
  const hostPath = new URL(
    "../../../assets/inject/renderer-inject/92-extension-host.js",
    import.meta.url,
  );

  it("内置菜单模板保持原样，只追加一个拓展挂载点", async () => {
    const source = await readFile(settingsPath, "utf8");
    // 挂载点在 home 面板里，且位于「提出问题」之后（即内置项末尾）。
    const openIdx = source.indexOf("overlay.innerHTML = `");
    const mountIdx = source.indexOf("${renderCodexPlusExtensionMenuRows()}");
    const issueIdx = source.indexOf("提出问题");
    assert.ok(mountIdx > 0, "找不到拓展菜单挂载点");
    assert.ok(mountIdx > issueIdx, "挂载点应位于内置项之后");
    // 关键：内置的行仍然是内联模板，没有被拆成数组。
    assert.ok(
      source.slice(openIdx, mountIdx).includes('class="codex-plus-row"'),
      "内置菜单行应当仍是内联模板",
    );
    assert.ok(
      !/const codexPlusBuiltinMenuRows\s*=/.test(source),
      "不应把内置菜单行抽成数组",
    );
  });

  it("点击委托里拓展分支排在内置分支之前", async () => {
    const source = await readFile(settingsPath, "utf8");
    const handler = source.indexOf('overlay.addEventListener("click"');
    assert.ok(handler > 0, "找不到点击委托");
    const body = source.slice(handler, handler + 600);
    const extIdx = body.indexOf("handleCodexPlusExtensionMenuClick(target)");
    const devtoolsIdx = body.indexOf("data-codex-open-devtools");
    assert.ok(extIdx > 0, "点击委托应当调用拓展菜单处理器");
    assert.ok(extIdx < devtoolsIdx, "拓展分支应排在内置分支之前");
  });

  it("开关与按钮分别渲染成不同的控件", async () => {
    const source = await readFile(hostPath, "utf8");
    const start = source.indexOf("function renderCodexPlusExtensionMenuRows()");
    const end = source.indexOf("function handleCodexPlusExtensionMenuClick(", start);
    assert.ok(start > 0 && end > start, "找不到菜单渲染函数");
    const body = source.slice(start, end);
    assert.match(body, /data-codex-plus-ext-setting/, "开关应有自己的 data 属性");
    assert.match(body, /data-codex-plus-ext-action/, "按钮应有自己的 data 属性");
    // 是追加到 home 面板，不是替换它。
    assert.ok(!/panel\.innerHTML\s*=/.test(body), "不应整体替换面板内容");
  });

  it("菜单项失败经由脚本状态上报，不向调用方抛出", async () => {
    const source = await readFile(hostPath, "utf8");
    const start = source.indexOf("function handleCodexPlusExtensionMenuClick(");
    const body = source.slice(start, start + 1400);
    assert.match(body, /runCodexPlusExtensionCallback/, "回调必须经失败隔离包装");
    assert.ok(!/try\s*{[\s\S]*item\.onActivate\(/.test(body), "不应裸调 onActivate");
  });
});

/**
 * 会话视图对齐（issue #2258 / #2085）的夹具运行时。
 *
 * 这里跑的是产物里真实的目标查找与宽度计算函数，配一个极简 DOM 桩——
 * 只实现查找链路真正用到的方法（querySelector/matches/closest/getBoundingClientRect）。
 * 类名夹具直接取自 Codex 打包产物里的字符串，不手写臆造。
 */
function conversationViewRuntime(renderer: string) {
  const start = renderer.indexOf("  // 旧版（26.9xx 之前）内容容器类名清单");
  const end = renderer.indexOf("  function codexServiceTierBadgeVisibleElement(", start);
  assert.ok(start >= 0 && end > start, "conversation view target finder block not found");
  const finderSource = renderer.slice(start, end);
  // 宽度计算与目标查找在文件里不相邻，单独切一段。
  const widthStart = renderer.indexOf("  function conversationViewEffectiveWidth(");
  const widthEnd = renderer.indexOf("  function conversationViewHtmlCenter(", widthStart);
  assert.ok(widthStart >= 0 && widthEnd > widthStart, "conversation view width block not found");
  const widthSource = renderer.slice(widthStart, widthEnd);
  const source = `${finderSource}\n${widthSource}`;

  const selectorStart = renderer.indexOf("  const selectors = {");
  const selectorEnd = renderer.indexOf("  };", selectorStart) + 4;
  assert.ok(selectorStart >= 0, "selectors table not found");
  const selectorsTable = renderer.slice(selectorStart, selectorEnd);

  type FakeEl = {
    tagName: string;
    className: string;
    attrs: Record<string, string>;
    children: FakeEl[];
    parentElement: FakeEl | null;
    style: Record<string, string>;
    dataset: Record<string, string>;
    rect: { left: number; width: number };
    vars: Record<string, string>;
    padding: { left: string; right: string };
    querySelectorAll(selector: string): FakeEl[];
    querySelector(selector: string): FakeEl | null;
    matches(selector: string): boolean;
    closest(selector: string): FakeEl | null;
    getBoundingClientRect(): { left: number; width: number; right: number; top: number; bottom: number; height: number };
  };

  function matches(el: FakeEl, selector: string): boolean {
    if (selector === "div") return el.tagName === "DIV";
    if (selector.startsWith("[") && selector.endsWith("]")) {
      const body = selector.slice(1, -1);
      const eq = body.indexOf("=");
      const name = eq === -1 ? body : body.slice(0, eq);
      const value = eq === -1 ? null : body.slice(eq + 1).replace(/^["']|["']$/g, "");
      const actual = el.attrs[name];
      if (actual === undefined) return false;
      return value === null ? true : actual === value;
    }
    if (selector.startsWith(".")) return el.className.split(/\s+/).includes(selector.slice(1));
    throw new Error(`夹具未实现的选择器: ${selector}`);
  }

  function descendants(root: FakeEl, out: FakeEl[] = []): FakeEl[] {
    for (const child of root.children) {
      out.push(child);
      descendants(child, out);
    }
    return out;
  }

  function query(root: FakeEl, selector: string): FakeEl[] {
    const parts = selector.split(",").map((part) => part.trim());
    return descendants(root).filter((el) => parts.some((part) => matches(el, part)));
  }

  function el(tag: string, className = "", attrs: Record<string, string> = {}): FakeEl {
    const node: FakeEl = {
      tagName: tag.toUpperCase(),
      className,
      attrs,
      children: [],
      parentElement: null,
      style: {},
      dataset: {},
      rect: { left: 0, width: 0 },
      vars: {},
      padding: { left: "", right: "" },
      querySelectorAll: (selector: string) => query(node, selector),
      querySelector: (selector: string) => query(node, selector)[0] ?? null,
      matches: (selector) => matches(node, selector),
      closest: (selector) => {
        let current: FakeEl | null = node;
        while (current) {
          if (matches(current, selector)) return current;
          current = current.parentElement;
        }
        return null;
      },
      getBoundingClientRect: () => ({
        left: node.rect.left,
        width: node.rect.width,
        right: node.rect.left + node.rect.width,
        top: 0,
        bottom: 0,
        height: 0,
      }),
    };
    return node;
  }

  function append(parent: FakeEl, child: FakeEl): FakeEl {
    parent.children.push(child);
    child.parentElement = parent;
    return child;
  }

  const root = el("body");
  const documentValue = {
    querySelector: (selector: string) => query(root, selector)[0] ?? null,
    querySelectorAll: (selector: string) => query(root, selector),
    documentElement: { getBoundingClientRect: () => ({ left: 0, width: 1440, right: 1440, top: 0, bottom: 0, height: 0 }) },
  };
  const factory = new Function(
    "document",
    "getComputedStyle",
    "sendCodexPlusDiagnostic",
    "codexPlusSettings",
    "localStorage",
    `${selectorsTable}
const conversationViewMinWidth = 320;
const conversationViewMaxAllowedWidth = 4000;
const conversationViewDefaultWidth = 900;
const conversationViewLegacyWidthKey = "codexPlus.threadCenter.maxWidth";
// 夹具只需要观察 style 写入结果，原始值记录这一步用空实现顶掉。
function conversationViewRememberOriginals() {}
function normalizeConversationViewWidth(value) {
  if (value === null || value === undefined || String(value).trim() === "") return null;
  const number = Number(value);
  if (!Number.isFinite(number)) return null;
  return Math.max(320, Math.min(4000, Math.round(number)));
}
function conversationViewWidth() {
  const settingsWidth = normalizeConversationViewWidth(codexPlusSettings().conversationViewMaxWidth);
  if (settingsWidth) return settingsWidth;
  const legacyWidth = normalizeConversationViewWidth(localStorage.getItem(conversationViewLegacyWidthKey));
  return legacyWidth || 900;
}
${source}
return {
  findContent: conversationViewFindContentEl,
  findComposer: conversationViewFindComposerEl,
  effective: conversationViewEffectiveWidth,
  available: conversationViewAvailableWidth,
  apply: conversationViewApplyNativeWidth,
};`,
  );

  const api = factory(
    documentValue,
    (node: FakeEl) => {
      // 同时充当 padding 来源与 CSSStyleDeclaration：兜底的变量反查会读索引属性。
      const style: Record<string, unknown> = {
        paddingLeft: node.padding.left,
        paddingRight: node.padding.right,
        length: Object.keys(node.vars).length,
        getPropertyValue: (name: string) => node.vars[name] ?? "",
      };
      Object.keys(node.vars).forEach((name, index) => { style[index] = name; });
      return style;
    },
    () => undefined,
    () => ({ conversationView: true, conversationViewMaxWidth: 900 }),
    { getItem: () => null },
  ) as {
    findContent(): FakeEl | null;
    findComposer(): FakeEl | null;
    effective(containerWidth: number): number;
    available(el: FakeEl): number;
    apply(el: FakeEl, effectiveWidth: number): void;
  };

  return { root, el, append, api };
}

describe("renderer injection conversation view alignment", () => {
  const rendererPath = new URL("../../../assets/inject/renderer-inject.js", import.meta.url);

  // 夹具类名取自真实产物：
  //   旧版内容容器 "… flex shrink-0 flex-col pb-8"
  //   新版内容容器 "relative flex flex-1 shrink-0 flex-col"
  //   页脚包裹层   "relative z-10 flex flex-col mx-auto w-full max-w-(--thread-content-max-width) px-toolbar"
  const legacyContentClass = "mx-auto w-full max-w-(--thread-content-max-width) px-toolbar relative flex shrink-0 flex-col pb-8";
  const newContentClass = "relative flex flex-1 shrink-0 flex-col";

  async function buildFixture(mode: "legacy" | "modern") {
    const runtime = conversationViewRuntime(await readFile(rendererPath, "utf8"));
    const scroller = runtime.append(runtime.root, runtime.el("div", "thread-scroll-container"));
    const host = runtime.append(scroller, runtime.el("div", "h-full flex"));
    if (mode === "legacy") {
      runtime.append(host, runtime.el("div", legacyContentClass));
    } else {
      // 新版：结构上只剩居中 + 满宽 + thread 宽度工具类，pb-8 已并入条件组合。
      runtime.append(host, runtime.el(
        "div",
        "relative flex flex-1 shrink-0 flex-col",
        { "data-thread-user-message-navigation-content": "" },
      ));
    }
    // 页脚包裹层带同样的宽度工具类，是最容易被误认成内容容器的节点。
    const footer = runtime.append(host, runtime.el(
      "div",
      "relative z-10 flex flex-col mx-auto w-full max-w-(--thread-content-max-width) px-toolbar",
      { "data-thread-scroll-footer": "true" },
    ));
    runtime.append(footer, runtime.el(
      "div",
      "mx-auto flex w-full max-w-(--thread-body-max-width) flex-col gap-8",
    ));
    return { runtime, scroller, host, footer };
  }

  it("旧版类名容器仍然命中", async () => {
    const { runtime } = await buildFixture("legacy");
    const found = runtime.api.findContent();
    assert.ok(found, "旧版类名应命中");
    assert.match(found.className, /pb-8/);
  });

  // 回归点：新版 Codex 把 max-w-(--thread-content-max-width) 换成
  // max-w-(--thread-body-max-width)、pb-8 消失后，全等匹配归零、居中宽度规则整体失效（#2258）。
  it("新版容器结构变化后仍能命中内容容器", async () => {
    const { runtime, footer } = await buildFixture("modern");
    const found = runtime.api.findContent();
    assert.ok(found, "新版类名变化后必须仍有降级路径，不能返回 null");
    assert.notEqual(found, footer, "不能把页脚包裹层当成内容容器");
    assert.equal(found.attrs["data-thread-user-message-navigation-content"], "");
  });

  it("作曲器优先落在页脚结构内，而不是内容容器", async () => {
    const { runtime, footer } = await buildFixture("modern");
    const found = runtime.api.findComposer();
    assert.ok(found, "作曲器应命中");
    assert.ok(found.closest("[data-thread-scroll-footer]"), "作曲器应在页脚包裹层内");
    assert.notEqual(found, footer);
  });

  // 用途词换过好几轮（content → body），所以按形状而不是字面量识别；
  // 与 50-navigation.js 的 ...FilterSourcePattern 同一套做法。
  it("宽度工具类按形状识别，换用途词也认", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const match = renderer.match(/const conversationViewThreadWidthTokenPattern\s*=\s*\/([\s\S]*?)\/;/);
    assert.ok(match, "未找到 conversationViewThreadWidthTokenPattern");
    const pattern = new RegExp(match![1]);
    for (const token of [
      "max-w-(--thread-content-max-width)", // 旧版
      "max-w-(--thread-body-max-width)", // 26.9xx 起
      "max-w-(--thread-content-responsive-max-width)",
      "md:max-w-(--thread-content-max-width)", // 带断点前缀
    ]) {
      assert.ok(pattern.test(token), `应命中: ${token}`);
    }
    // 哈希类名与无关工具类必须排掉；尤其不能把 max-w-2xl 这类固定宽度也算进来。
    for (const token of ["max-w-2xl", "_shell_151xi_3", "max-w-full", "w-full"]) {
      assert.ok(!pattern.test(token), `不应命中: ${token}`);
    }
  });

  it("两处类名都不对时按 CSS 变量反查宿主节点", async () => {
    const runtime = conversationViewRuntime(await readFile(rendererPath, "utf8"));
    const scroller = runtime.append(runtime.root, runtime.el("div", "thread-scroll-container"));
    const host = runtime.append(scroller, runtime.el("div", "h-full flex"));
    // 既没有新版类名工具类，也没有 data-* 锚点，只剩 Codex 注入的 CSS 变量。
    const bare = runtime.append(host, runtime.el("div", "some-renamed-class"));
    bare.vars["--thread-body-max-width"] = "calc(900px + 0px)";
    const found = runtime.api.findContent();
    assert.ok(found, "兜底候选应能反查到宿主节点");
    assert.equal(found, bare);
  });

  it("加载早期继承线程宽度变量的应用根与侧栏不能作为聊天容器", async () => {
    const runtime = conversationViewRuntime(await readFile(rendererPath, "utf8"));
    const appRoot = runtime.append(runtime.root, runtime.el("div", "", { id: "root" }));
    const sidebar = runtime.append(appRoot, runtime.el("div", "sidebar"));
    appRoot.vars["--thread-body-max-width"] = "900px";
    sidebar.vars["--thread-body-max-width"] = "900px";
    assert.equal(runtime.api.findContent(), null);
    assert.equal(runtime.api.findComposer(), null);
    assert.deepEqual(appRoot.style, {});
  });

  it("目标全部缺失时上报诊断，不再静默 return", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const start = renderer.indexOf("  function conversationViewAlignNow()");
    const end = renderer.indexOf("  function conversationViewHasRoomForHtmlCenterAt(", start);
    assert.ok(start >= 0 && end > start);
    const body = renderer.slice(start, end);
    assert.match(body, /conversationViewReportMissingTargets\(\)/, "归零路径必须上报诊断");
    assert.match(renderer, /"conversation_view_target_not_found"/);
  });

  // #2085：设置值是上限，实际宽度按容器可用宽度收敛。
  it("容器窄于设置上限时按容器宽度收敛，宽于上限时用上限", async () => {
    const { runtime } = await buildFixture("modern");
    // 设置 900（夹具里 conversationViewMaxWidth = 900）。
    assert.equal(runtime.api.effective(1400), 900, "宽容器应按设置上限");
    assert.equal(runtime.api.effective(600), 600, "窄容器应按容器可用宽度");
    assert.equal(runtime.api.effective(0), 900, "拿不到几何时回落设置上限");
    assert.equal(runtime.api.effective(Number.NaN), 900);
    // 硬下限仍是既有边界，不因自适应而改变。
    assert.equal(runtime.api.effective(100), 320);
  });

  it("可用宽度取宿主内容盒（扣掉左右内边距）", async () => {
    const { runtime, host } = await buildFixture("modern");
    host.rect.width = 1000;
    host.padding.left = "16px";
    host.padding.right = "16px";
    const target = runtime.api.findContent()!;
    assert.equal(runtime.api.available(target), 968);
    // 写进 style 的是收敛后的值，不是写死设置值。
    runtime.api.apply(target, runtime.api.effective(runtime.api.available(target)));
    assert.equal(target.style.maxWidth, "900px");
    host.rect.width = 500;
    runtime.api.apply(target, runtime.api.effective(runtime.api.available(target)));
    assert.equal(target.style.maxWidth, "468px");
  });

  // 自适应计算必须留在既有的「先统一写 style、再统一读几何」两阶段里，
  // 否则退回 commit 82fb0924 修掉的读-写交替强制重排。
  it("自适应计算不引入逐元素读-写交替", async () => {
    const renderer = await readFile(rendererPath, "utf8");
    const start = renderer.indexOf("  function conversationViewAlignNow()");
    const end = renderer.indexOf("  function conversationViewReportMissingTargets(", start);
    const body = renderer.slice(start, end);
    // 宽度读取必须集中在写之前，且是一次 map 批量读取。
    const readIndex = body.indexOf("conversationViewAvailableWidth(el)");
    const writeIndex = body.indexOf("conversationViewApplyNativeWidth(el");
    assert.ok(readIndex >= 0 && writeIndex > readIndex, "读取必须在写入之前");
    assert.match(body, /targets\.map\(\(el\) => conversationViewAvailableWidth\(el\)\)/);
  });
});
