import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { readFile } from "node:fs/promises";

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

type FakeElementOptions = {
  className?: string;
  dismissLabel?: string;
  hasProgress?: boolean;
  styleDisplay?: string;
};

class FakeElement {
  children: FakeElement[] = [];
  dataset: Record<string, string> = {};
  parentElement: FakeElement | null = null;
  style: { display: string };
  private readonly className: string;
  private readonly dismissLabel: string;
  private readonly hasProgress: boolean;

  constructor(options: FakeElementOptions = {}) {
    this.className = options.className ?? "";
    this.dismissLabel = options.dismissLabel ?? "";
    this.hasProgress = options.hasProgress ?? false;
    this.style = { display: options.styleDisplay ?? "" };
  }

  appendChild(child: FakeElement) {
    child.parentElement = this;
    this.children.push(child);
  }

  getAttribute(name: string) {
    return name === "aria-label" ? this.dismissLabel : null;
  }

  matches(selector: string) {
    return selector === "div.w-full" && this.className.split(/\s+/).includes("w-full");
  }

  querySelector(selector: string) {
    return selector === 'progress[max="100"]' && this.hasProgress ? new FakeElement() : null;
  }

  querySelectorAll(selector: string) {
    return selector === "button" && this.dismissLabel ? [this] : [];
  }
}

function usageAlertRuntime(renderer: string, cards: FakeElement[], managed: FakeElement[]) {
  const start = renderer.indexOf("  function officialUsageAlertHidden(");
  const end = renderer.indexOf("\n  let zedRemoteStatusPromise", start);
  assert.ok(start >= 0 && end > start);
  const source = renderer.slice(start, end);
  const selectors: string[] = [];
  const document = {
    querySelectorAll(selector: string) {
      selectors.push(selector);
      return selector === '[data-codex-plus-usage-alert-hidden="true"]'
        ? managed.filter((node) => node.dataset.codexPlusUsageAlertHidden === "true")
        : cards;
    },
  };
  const windowValue: Record<string, unknown> = {};
  const create = new Function(
    "window",
    "document",
    "HTMLElement",
    `${source}\nreturn { officialUsageAlertHidden, refreshOfficialUsageAlertVisibility };`,
  ) as (
    windowValue: Record<string, unknown>,
    documentValue: typeof document,
    elementType: typeof FakeElement,
  ) => {
    officialUsageAlertHidden: () => boolean;
    refreshOfficialUsageAlertVisibility: () => void;
  };
  return { runtime: create(windowValue, document, FakeElement), selectors, windowValue };
}

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
    assert.match(renderer, /installCodexPlusSidebarNavigation\(\);/);
    assert.match(renderer, /document\.querySelectorAll\(`#\$\{codexPlusMenuId\}/);
  });

  it("renders the official multi-account quota monitor through safe bridge routes", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /data-codex-plus-tab="officialAccounts"/);
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

  it("hides only the official usage alert and restores it without changing upstream styles", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");
    const wrapper = new FakeElement({ className: "w-full", styleDisplay: "grid" });
    const usageAlert = new FakeElement({ dismissLabel: "Dismiss usage alert", hasProgress: true });
    const otherStatus = new FakeElement({ dismissLabel: "Dismiss sync status", hasProgress: true });
    wrapper.appendChild(usageAlert);
    const { runtime, selectors, windowValue } = usageAlertRuntime(renderer, [usageAlert, otherStatus], [wrapper]);

    windowValue.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = true;
    runtime.refreshOfficialUsageAlertVisibility();

    assert.equal(wrapper.dataset.codexPlusUsageAlertHidden, "true");
    assert.equal(wrapper.style.display, "grid");
    assert.equal(otherStatus.dataset.codexPlusUsageAlertHidden, undefined);
    assert.deepEqual(selectors, [
      '[data-codex-plus-usage-alert-hidden="true"]',
      'aside.app-shell-left-panel [role="status"][aria-live="polite"]',
    ]);

    windowValue.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = false;
    runtime.refreshOfficialUsageAlertVisibility();

    assert.equal(wrapper.dataset.codexPlusUsageAlertHidden, undefined);
    assert.equal(wrapper.style.display, "grid");
    assert.equal(wrapper.children[0], usageAlert);
    assert.equal(selectors.at(-1), '[data-codex-plus-usage-alert-hidden="true"]');
  });

  it("refreshes active-profile usage alert settings through the existing backend heartbeat", async () => {
    const renderer = await readFile(new URL("../../../assets/inject/renderer-inject.js", import.meta.url), "utf8");

    assert.match(renderer, /typeof nextStatus\.hideOfficialUsageAlert === "boolean"/);
    assert.match(renderer, /window\.__CODEX_PLUS_HIDE_OFFICIAL_USAGE_ALERT__ = nextStatus\.hideOfficialUsageAlert/);
    assert.match(renderer, /\[data-codex-plus-usage-alert-hidden="true"\] \{ display: none !important; \}/);
    assert.doesNotMatch(renderer, /container\.style\.(?:setProperty|removeProperty)\("display"/);
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
});

describe("vscode model transport routing", () => {
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
    const catalog = { routeDescriptors: [{ routingSlug: "CLIProxyAPI:gpt-6-sol", providerId: "codex_plus_cli_official" }] };
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
       const normalizeCodexModelReasoningParams = (_method, params) => ({ ...params, modelProvider: params.model.startsWith("CLIProxyAPI:") ? "codex_plus_cli_official" : "openai" });
       const sendCodexPlusDiagnostic = (event) => diagnostics.push(event);
       const codexRateLimitUnlockRequestMethod = () => "";
       const neutralizeCodexRateLimitPayload = (payload) => payload;
       ${renderer.slice(start, end)}
       return patchCodexVscodeRequestTransport({ Transport });`,
    ) as (catalog: object, settings: object, diagnostics: string[], Transport: object) => boolean;
    const diagnostics: string[] = [];
    assert.equal(create(catalog, settings, diagnostics, Transport), true);
    for (const model of ["gpt-6-sol", "CLIProxyAPI:gpt-6-sol", "gpt-6-sol"]) {
      await transport.sendRequest("POST", "vscode://codex/thread/start", { body: JSON.stringify({ model }) });
    }
    assert.deepEqual(sent.map((entry) => JSON.parse(entry.options.body).modelProvider), [
      "openai", "codex_plus_cli_official", "openai",
    ]);
    await assert.rejects(
      transport.sendRequest("POST", "vscode://codex/thread/start", { body: JSON.stringify({ model: "Unknown:model" }) }),
      /没有可用的来源路由/,
    );
    assert.equal(sent.length, 3);
    assert.deepEqual(diagnostics, ["model_vscode_request_patch_installed"]);
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
