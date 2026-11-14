(() => {
  // The launcher targets the Codex app page, but keep a renderer-side guard
  // so this bundle cannot create UI in embedded browser documents.
  const codexPlusIsNodeTestHarness = typeof process === "object" && !!process.versions?.node;
  if (!codexPlusIsNodeTestHarness && (window.top !== window || window.self !== window || !window.electronBridge || !/^app:\/\/\-\//i.test(window.location.href))) return;
  const codexPlusIsWindowsPlatform = /\bWindows\b/i.test(navigator.userAgent || "");
  const codexPlusIsMacPlatform = /\bMacintosh\b/i.test(navigator.userAgent || "");
  const codexPlusSupportsPetRealMouseLook = codexPlusIsWindowsPlatform || codexPlusIsMacPlatform;
  // 每个注入实例的代际。重新注入会带一整套新的闭包，但旧实例留在页面上的定时器、
  // 订阅循环还活着，且它们只认自己闭包里的标志位——每注入一次就多叠一套轮询
  // （实测三份叠起来把 /extensions/history 顶到 544 次/分钟）。所有常驻循环都拿它自检。
  const codexPlusRuntimeGeneration = (window.__codexPlusRuntimeGeneration || 0) + 1;
  window.__codexPlusRuntimeGeneration = codexPlusRuntimeGeneration;

  function codexPlusStatsigClients() {
    const root = window.__STATSIG__ || globalThis.__STATSIG__;
    if (!root || typeof root !== "object") return [];
    const clients = [root.firstInstance, typeof root.instance === "function" ? root.instance() : null];
    if (root.instances && typeof root.instances === "object") clients.push(...Object.values(root.instances));
    return clients.filter((client, index, array) => client && typeof client === "object" && array.indexOf(client) === index);
  }

  /**
   * User-triggered Statsig refresh. SDK versions expose different refresh
   * methods, so only invoke methods proven to exist on the live client and
   * report unsupported when none is available.
   */
  window.__codexPlusRefreshStatsigUiConfig = async () => {
    const clients = codexPlusStatsigClients();
    const methods = [];
    const errors = [];
    for (const client of clients) {
      try {
        if (typeof client.updateUserAsync === "function") {
          const user = typeof client.getUser === "function" ? client.getUser() : client.user;
          await client.updateUserAsync(user);
          methods.push("updateUserAsync");
        } else if (typeof client.initializeAsync === "function") {
          await client.initializeAsync();
          methods.push("initializeAsync");
        } else if (typeof client.fetchValues === "function") {
          await client.fetchValues();
          methods.push("fetchValues");
        }
      } catch (error) {
        errors.push(String(error?.message || error));
      }
    }
    const current = clients[0];
    let enableI18n = null;
    try {
      const layer = typeof current?.getLayer === "function" ? current.getLayer("72216192") : null;
      if (layer && typeof layer.get === "function") enableI18n = layer.get("enable_i18n", null);
      if (enableI18n === null && typeof current?.getDynamicConfig === "function") {
        enableI18n = current.getDynamicConfig("72216192", { disableExposureLog: true })?.get?.("enable_i18n", null) ?? null;
      }
    } catch (error) {
      errors.push(String(error?.message || error));
    }
    const result = {
      status: methods.length ? (errors.length ? "partial" : "ok") : "unsupported",
      methods: Array.from(new Set(methods)),
      clientCount: clients.length,
      enable_i18n: typeof enableI18n === "boolean" ? enableI18n : null,
      errors,
    };
    window.dispatchEvent(new CustomEvent("codex-plus-statsig-refreshed", { detail: result }));
    return result;
  };

  function installCodexPlusForceChineseLocale() {
    const config = window.__CODEX_PLUS_FORCE_CHINESE_LOCALE__;
    if (!config) return;
    const enabled = config.enabled === true;
    const locale = typeof config.locale === "string" && config.locale ? config.locale : "zh-CN";
    const installationKey = `2:${enabled ? "on" : "off"}:${locale}`;
    if (window.__codexPlusForceChineseLocaleInstalled === installationKey) return;
    window.__codexPlusForceChineseLocaleInstalled = installationKey;
    const languages = [locale, "zh", "en-US", "en"];
    const managedLocaleStorageKey = "codexPlus.forceChineseLocale.managed.v1";
    const localeReloadStorageKey = "codexPlus.forceChineseLocale.reload.v1";

    const readManagedLocale = () => {
      try {
        const value = JSON.parse(window.localStorage.getItem(managedLocaleStorageKey) || "null");
        return value && typeof value === "object" ? value : null;
      } catch {
        return null;
      }
    };

    const writeManagedLocale = (value) => {
      try {
        if (value) {
          window.localStorage.setItem(managedLocaleStorageKey, JSON.stringify(value));
        } else {
          window.localStorage.removeItem(managedLocaleStorageKey);
        }
      } catch {
      }
    };

    // 读写走 readCodexAppSetting / writeCodexAppSetting（20-menu.js）：先找宿主服务的
    // settings.readAll / settings.write，找不到再退回 codexStateCall 的 get-setting /
    // set-setting。issue #2755：Codex 26.10 起设置改由宿主服务读写，产物里已没有
    // get-setting / set-setting / set-global-state，只走 state API 时 localeOverride
    // 永远写不进去、也不会触发刷新，界面就一直是英文。
    //
    // 两条通道都依赖 App 的 asset：模块 promise 会被缓存，asset 加载失败还会留下
    // 30 秒冷却（见 20-menu.js 的 codexAppModuleRetryCooldownMs）。注入发生在 asset
    // 就绪之前时第一次调用必然失败，所以这里必须有界重试，不能一次不成就算了。
    const localeSyncRetryDelaysMs = [500, 1500, 3000, 6000, 12000, 20000];
    const withLocaleSyncRetry = async (operation) => {
      let lastError;
      for (let attempt = 0; attempt <= localeSyncRetryDelaysMs.length; attempt += 1) {
        try {
          return await operation();
        } catch (error) {
          lastError = error;
          const delay = localeSyncRetryDelaysMs[attempt];
          if (delay === undefined) break;
          await new Promise((resolve) => window.setTimeout(resolve, delay));
        }
      }
      throw lastError;
    };
    // 未写过时宿主服务里没有这个键，按 Codex 的默认值 null 处理。
    const readLocaleOverride = () => withLocaleSyncRetry(async () => (await readCodexAppSetting("localeOverride")) ?? null);
    const writeLocaleOverride = (value) => withLocaleSyncRetry(() => writeCodexAppSetting("localeOverride", value));
    const reloadAfterLocaleChange = (value) => {
      const marker = JSON.stringify(value);
      try {
        if (window.sessionStorage.getItem(localeReloadStorageKey) === marker) return;
        window.sessionStorage.setItem(localeReloadStorageKey, marker);
        // 标记写不进去就不要刷新，否则下次加载读不到标记，会再次刷新。
        if (window.sessionStorage.getItem(localeReloadStorageKey) !== marker) return;
      } catch {
        return;
      }
      window.location.reload();
    };

    const clearLocaleReloadMarker = () => {
      try {
        window.sessionStorage.removeItem(localeReloadStorageKey);
      } catch {
      }
    };

    const syncOfficialLocaleSetting = async () => {
      const managed = readManagedLocale();
      if (!enabled && !managed) return;
      const currentValue = await readLocaleOverride();

      if (enabled) {
        if (currentValue === locale) {
          clearLocaleReloadMarker();
          return;
        }
        if (!managed) {
          writeManagedLocale({ appliedLocale: locale, previousValue: currentValue });
        }
        await writeLocaleOverride(locale);
        reloadAfterLocaleChange(locale);
        return;
      }

      if (currentValue !== managed.appliedLocale) {
        writeManagedLocale(null);
        clearLocaleReloadMarker();
        return;
      }
      const previousValue = managed.previousValue ?? null;
      await writeLocaleOverride(previousValue);
      writeManagedLocale(null);
      reloadAfterLocaleChange(previousValue);
    };

    syncOfficialLocaleSetting().catch(() => {});
    if (!enabled) return;

    const defineNavigatorGetter = (name, value) => {
      try {
        Object.defineProperty(Navigator.prototype, name, {
          configurable: true,
          get: () => value,
        });
      } catch {
        try {
          Object.defineProperty(navigator, name, {
            configurable: true,
            get: () => value,
          });
        } catch {
        }
      }
    };

    defineNavigatorGetter("language", locale);
    defineNavigatorGetter("languages", languages);

    const patchI18nConfig = (dynamicConfig) => {
      if (!dynamicConfig || typeof dynamicConfig !== "object") return dynamicConfig;
      const value = dynamicConfig.value && typeof dynamicConfig.value === "object" ? dynamicConfig.value : {};
      const nextValue = {
        ...value,
        enable_i18n: true,
        locale_source: "SYSTEM",
      };
      try {
        dynamicConfig.value = nextValue;
      } catch {
      }
      if (typeof dynamicConfig.get === "function" && !dynamicConfig.__codexPlusForceChineseLocaleGetPatched) {
        const originalGet = dynamicConfig.get.bind(dynamicConfig);
        dynamicConfig.get = (key, fallback) => {
          if (key === "enable_i18n") return true;
          if (key === "locale_source") return "SYSTEM";
          return originalGet(key, fallback);
        };
        dynamicConfig.__codexPlusForceChineseLocaleGetPatched = true;
      }
      return dynamicConfig;
    };

    const statsigClients = () => {
      const root = window.__STATSIG__ || globalThis.__STATSIG__;
      if (!root || typeof root !== "object") return [];
      const clients = [root.firstInstance, typeof root.instance === "function" ? root.instance() : null];
      if (root.instances && typeof root.instances === "object") clients.push(...Object.values(root.instances));
      return clients.filter((client, index, array) => client && typeof client === "object" && array.indexOf(client) === index);
    };

    // 语言包的加载 gate 读的是 Layer 而不是 DynamicConfig（issue #2329 根因 a）：
    // 应用用 useLayer('72216192').get('enable_i18n', false) 取开关，Layer 的 memo
    // 缓存键恒为 NoValues，于是即使 DynamicConfig 被补成 enable_i18n:true 也不生效。
    // 这里给 layer 对象补上同样的取值覆盖；__value 是 Statsig 存原始值的字段，
    // 一并 assign，避免应用直接从 __value 读时绕过 get。
    const patchI18nLayer = (layer) => {
      if (!layer || typeof layer !== "object") return layer;
      const value = layer.__value && typeof layer.__value === "object" ? layer.__value : {};
      const nextValue = {
        ...value,
        enable_i18n: true,
        locale_source: "SYSTEM",
      };
      try {
        layer.__value = nextValue;
      } catch {
      }
      try {
        layer.value = nextValue;
      } catch {
      }
      if (typeof layer.get === "function" && !layer.__codexPlusForceChineseLocaleLayerPatched) {
        const originalGet = layer.get.bind(layer);
        layer.get = (key, fallback) => {
          if (key === "enable_i18n") return true;
          if (key === "locale_source") return "SYSTEM";
          return originalGet(key, fallback);
        };
        layer.__codexPlusForceChineseLocaleLayerPatched = true;
      }
      return layer;
    };

    const patchStatsigClient = (client) => {
      if (!client || typeof client !== "object") return;
      if (typeof client.getLayer === "function" && !client.__codexPlusForceChineseLocaleLayerChannelPatched) {
        const originalGetLayer = client.getLayer.bind(client);
        client.getLayer = (name, options) => {
          const result = originalGetLayer(name, options);
          return name === "72216192" ? patchI18nLayer(result) : result;
        };
        client.__codexPlusForceChineseLocaleLayerChannelPatched = true;
      }
      if (typeof client._getLayerImpl === "function" && !client.__codexPlusForceChineseLocaleLayerImplPatched) {
        const originalGetLayerImpl = client._getLayerImpl.bind(client);
        client._getLayerImpl = function (name, ...rest) {
          const result = originalGetLayerImpl(name, ...rest);
          return name === "72216192" ? patchI18nLayer(result) : result;
        };
        client.__codexPlusForceChineseLocaleLayerImplPatched = true;
      }
      if (typeof client.getDynamicConfig !== "function") return;
      if (!client.__codexPlusForceChineseLocalePatched) {
        const originalGetDynamicConfig = client.getDynamicConfig.bind(client);
        client.getDynamicConfig = (name, options) => {
          const result = originalGetDynamicConfig(name, options);
          return name === "72216192" ? patchI18nConfig(result) : result;
        };
        client.__codexPlusForceChineseLocalePatched = true;
      }
      try {
        patchI18nConfig(client.getDynamicConfig("72216192", { disableExposureLog: true }));
      } catch {
      }
      try {
        if (typeof client.getLayer === "function") {
          patchI18nLayer(client.getLayer("72216192", { disableExposureLog: true }));
        }
      } catch {
      }
    };

    const patchStatsigRoot = (root) => {
      if (!root || typeof root !== "object" || root.__codexPlusForceChineseLocaleRootPatched) return;
      root.__codexPlusForceChineseLocaleRootPatched = true;
      ["firstInstance", "instance"].forEach((key) => {
        let current;
        try {
          current = root[key];
        } catch {
          return;
        }
        patchStatsigClient(typeof current === "function" && key === "instance" ? current.call(root) : current);
        try {
          Object.defineProperty(root, key, {
            configurable: true,
            get: () => current,
            set: (next) => {
              current = next;
              patchStatsigClient(typeof next === "function" && key === "instance" ? next.call(root) : next);
            },
          });
        } catch {
        }
      });
    };

    const installStatsigRootSetter = () => {
      const descriptor = Object.getOwnPropertyDescriptor(window, "__STATSIG__");
      if (descriptor && descriptor.configurable === false) return;
      let currentRoot = window.__STATSIG__;
      patchStatsigRoot(currentRoot);
      try {
        Object.defineProperty(window, "__STATSIG__", {
          configurable: true,
          get: () => currentRoot,
          set: (next) => {
            currentRoot = next;
            patchStatsigRoot(next);
            statsigClients().forEach(patchStatsigClient);
          },
        });
      } catch {
      }
    };

    const patchStatsigI18nConfig = () => {
      installStatsigRootSetter();
      const root = window.__STATSIG__ || globalThis.__STATSIG__;
      patchStatsigRoot(root);
      statsigClients().forEach((client) => {
        if (typeof client.getDynamicConfig !== "function") return;
        patchStatsigClient(client);
      });
    };

    patchStatsigI18nConfig();
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      patchStatsigI18nConfig();
      if (Date.now() - startedAt > 5000) window.clearInterval(timer);
    }, 50);
  }

  const helperBase = window.__CODEX_SESSION_DELETE_HELPER__ || "http://127.0.0.1:57321";
  const buttonClass = "codex-delete-button";
  const exportButtonClass = "codex-export-button";
  const actionButtonClass = "codex-session-action-button";
  const actionGroupClass = "codex-session-actions";
  const moreButtonClass = "codex-session-more-button";
  const moreMenuClass = "codex-session-more-menu";
  const actionTooltipClass = "codex-session-action-tooltip";
  const threadIdBadgeClass = "codex-thread-id-badge";
  const conversationViewMinWidth = 320;
  const conversationViewMaxAllowedWidth = 4000;
  const conversationViewDefaultWidth = 900;
  const conversationViewLegacyWidthKey = "codexPlus.threadCenter.maxWidth";
  // 模型容量错误自动重试的次数上下限。放在 prelude 是因为默认设置
  // （10-style.js 的 defaultCodexPlusSettings）和运行逻辑（81-capacity-auto-retry.js）
  // 都要用，而后者的 const 在 IIFE 里初始化得更晚。
  const capacityAutoRetryDefaultMaxAttempts = 15;
  const capacityAutoRetryMaxAttemptsCeiling = 200;
  // 默认 0 秒 = 上一轮失败后立刻重试（与手动点击的手感一致），
  // 运行逻辑里另有 1 秒地板值防热循环。
  const capacityAutoRetryDefaultIntervalSeconds = 0;
  const capacityAutoRetryIntervalCeilingSeconds = 600;
  // 已发布的旧类名保留兼容；对应功能不再安装。
  const zedRemoteButtonClass = "codex-zed-remote-button";
  const zedRemoteOpenInMenuItemClass = "codex-zed-open-in-menu-item";
  const sessionCopyMenuItemClass = "codex-session-copy-menu-item";
  const sessionCopyMenuItemVersion = "1";
  const sessionCopyMenuActivationTimeoutMs = 12000;
  const sessionShareButtonClass = "codex-session-share-button";
  const zedRemoteToastClass = "codex-zed-remote-toast";
  const upstreamWorktreeDialogClass = "codex-upstream-worktree-dialog";
  const upstreamBranchOptionAttribute = "data-codex-upstream-branch-option";
  const styleId = "codex-delete-style";
  // 改 10-style.js 里的任何 CSS 都要把它 +1：installStyle 靠这个版本号判断
  // 页面里已有的 <style> 是否过期，不升的话新样式在旧标签存在时会被直接跳过。
  const codexDeleteStyleVersion = "38";
  const codexPlusMenuId = "codex-plus-menu";
  const codexPlusSidebarNavId = "codex-plus-sidebar-nav";
  const codexPlusPageClass = "codex-plus-page-overlay";
  // 新版 Codex 在最左侧多出一条导航图标栏（navigation rail）。
  // 三个入口分别挂进去：Codex++ 主页、「拓展」（原用户脚本）和「推荐内容」。
  // 三者各自是一个独立页面，不再作为弹窗里的二级 tab。
  const codexPlusRailNavId = "codex-plus-rail-nav";
  const codexPlusRailExtensionsId = "codex-plus-rail-extensions";
  const codexPlusRailSponsorId = "codex-plus-rail-sponsor";
  const codexPlusRailPluginMarketId = "codex-plus-rail-plugin-market";
  const codexPlusSidebarPluginMarketId = "codex-plus-sidebar-plugin-market";
  const codexPlusPluginMarketTab = "plugin-market";
  const codexPlusRailSelector = "nav[data-app-navigation-rail]";
  const codexPlusRailDestinationSelector = "[data-sidebar-destination]";
  const codexPlusExtensionsTab = "extensions";
  const codexPlusSponsorTab = "sponsor";
  // Codex 的界面缩放是给内层布局节点设 CSS zoom，不是改 documentElement。
  // 我们的 overlay 挂在 body 下、落在那棵缩放子树之外，只能自己读这个变量跟随。
  const codexPlusWindowZoomVar = "--codex-window-zoom";
  const codexDeleteVersion = "7";
  const codexExportVersion = "1";
  const codexActionGroupVersion = "8";
  const codexArchiveRowActionsVersion = "1";
  const codexArchiveDeleteAllVersion = "2";
  const codexConversationViewVersion = "1";
  const codexThreadScrollVersion = "1";
  const codexThreadIdBadgeVersion = "1";
  const codexServiceTierBadgeClass = "codex-service-tier-badge";
  const codexQuotaResumeButtonClass = "codex-plus-quota-resume-button";
  const codexServiceTierBadgeVersion = "3";
  let codexPlusVersion = window.__CODEX_PLUS_VERSION__ || "unknown";
  const codexPlusBuild = window.__CODEX_PLUS_BUILD__ || "unknown";
  let lastSessionActionTrigger = null;
  const codexPlusSettingsKey = "codexPlusSettings";
  const codexThreadScrollKey = "codexThreadScroll";
  const codexThreadServiceTierKey = "codexThreadServiceTierOverrides";
  const codexThreadServiceTierMaxEntries = 120;
  const codexThreadServiceTierDraftBindWindowMs = 60 * 1000;
  const codexServiceTierRequestOverrideVersion = "10";
  const codexAppServerModelRequestPatchVersion = "38";
  const codexAppServerClientCaptureMarker = "AppServerRequestClient is missing a message dispatcher";
  const codexAppServerClientCaptureAnchor = "async sendRequest(";
  const codexRemoteSessionRecoveryVersion = "8";
  const codexQuotaResumeVersion = "1";
  const codexPluginMarketplaceUnlockVersion = "17";
  const codexModelRequestBridgePatchVersion = "2";
  const codexRateLimitUnlockVersion = "3";
  const codexThreadScrollMaxEntries = 120;
  const codexThreadScrollSaveThrottleMs = 120;
  const codexThreadScrollRestoreWindowMs = 3200;
  const codexThreadScrollRestoreDelaysMs = [0, 80, 220, 500, 1000, 1800, 2800];
  const codexThreadScrollUserIntentWindowMs = 1200;
  const codexThreadScrollProgrammaticGuardVersion = "dispatcher:2";
  const codexThreadScrollRouteHooksVersion = "dispatcher:2";
  const codexThreadScrollListenerVersion = "4";
  const codexThreadScrollUserIntentVersion = "dispatcher:2";
  const codexPlusDreamSkinStyleId = "codex-dream-skin-style";
  const codexPlusDreamSkinPlatform = String(window.__CODEX_PLUS_DREAM_SKIN_PLATFORM__ || "macos");
  const codexPlusDreamSkinRevision = String(window.__CODEX_PLUS_DREAM_SKIN_REVISION__ || "1");
  clearTimeout(window.__codexThreadScrollSaveTimer);
  window.__codexThreadScrollSaveTimer = null;
  (window.__codexThreadScrollRestoreTimers || []).forEach((timer) => clearTimeout(timer));
  window.__codexThreadScrollRestoreTimers = [];
  (window.__codexThreadScrollSyncTimers || []).forEach((timer) => clearTimeout(timer));
  window.__codexThreadScrollSyncTimers = [];
  window.__codexThreadScrollRestoreRevision = (window.__codexThreadScrollRestoreRevision || 0) + 1;

  window.__codexThreadScrollSyncRevision = (window.__codexThreadScrollSyncRevision || 0) + 1;
  ["__codexPlusHtmlCenteredThreadWidth", "__codexPlusViewportCenteredThreadWidth", "__codexPlusBoundedThreadCenter"].forEach((key) => {
    try {
      window[key]?.cleanup?.();
    } catch (_) {}
  });
  try {
    window.__codexPlusConversationViewCleanup?.();
  } catch (_) {}
  window.__codexPlusConversationViewCleanup = null;
  const selectors = {
    sidebarThread: "[data-app-action-sidebar-thread-id]",
    threadTitle: "[data-thread-title]",
    appHeader: '[class*="ApplicationMenuTopBar"], .app-header-tint',
    archiveNav: 'button[aria-label="已归档对话"], button[aria-label="Archived conversations"]',
    disabledInstallButton: 'button:disabled, button[aria-disabled="true"], [role="button"][aria-disabled="true"], button[data-disabled], [role="button"][data-disabled], button.cursor-not-allowed, [role="button"].cursor-not-allowed, button.pointer-events-none, [role="button"].pointer-events-none',
    pluginNavButton: 'nav[role="navigation"] button.h-token-nav-row.w-full',
    pluginSvgPath: 'svg path[d^="M7.94562 14.0277"]',
    // 会话视图对齐的目标锚点。全部走 data-* / 结构性写法，不绑 Codex 的哈希类名，
    // 见 90-action-groups.js 的候选链说明（issue #2258）。
    conversationViewScrollContainer: ".thread-scroll-container",
    conversationViewContentAnchor: "[data-thread-user-message-navigation-content]",
    conversationViewFooter: "[data-thread-scroll-footer]",
  };
  const headerContextButtonClass = "border-token-border user-select-none no-drag cursor-interaction flex items-center gap-1 border whitespace-nowrap focus:outline-none disabled:cursor-not-allowed disabled:opacity-40 rounded-lg border-token-border text-token-button-tertiary-foreground bg-token-bg-fog enabled:hover:bg-token-list-hover-background data-[state=open]:bg-token-list-hover-background border h-token-button-composer px-2 py-0 text-base leading-[18px]";

  function installCodexPlusFastStartup() {
    const config = window.__CODEX_PLUS_FAST_STARTUP__;
    if (!config || config.enabled !== true) return;
    if (window.__codexPlusFastStartupInstalled === "1") return;
    window.__codexPlusFastStartupInstalled = "1";
    const timeoutMs = Math.max(100, Math.min(Number(config.statsigTimeoutMs) || 800, 3000));
    const statsigHosts = new Set([
      "ab.chatgpt.com",
      "featureassets.org",
      "prodregistryv2.org",
      "api.statsigcdn.com",
      "statsigapi.net",
      "cloudflare-dns.com",
    ]);

    const isStatsigUrl = (input) => {
      try {
        const url = new URL(typeof input === "string" ? input : input?.url ?? "", window.location.href);
        return statsigHosts.has(url.hostname);
      } catch {
        return false;
      }
    };

    const timeoutSignal = (signal) => {
      const controller = new AbortController();
      const timer = window.setTimeout(() => controller.abort(), timeoutMs);
      const clear = () => window.clearTimeout(timer);
      if (signal) {
        if (signal.aborted) controller.abort();
        else signal.addEventListener("abort", () => controller.abort(), { once: true });
      }
      return { signal: controller.signal, clear };
    };

    const patchFetch = () => {
      if (typeof window.fetch !== "function" || window.fetch.__codexPlusFastStartupPatched) return;
      const originalFetch = window.fetch.bind(window);
      const patchedFetch = (input, init = undefined) => {
        if (!isStatsigUrl(input)) return originalFetch(input, init);
        const { signal, clear } = timeoutSignal(init?.signal);
        const nextInit = { ...(init || {}), signal };
        return originalFetch(input, nextInit).finally(clear);
      };
      patchedFetch.__codexPlusFastStartupPatched = true;
      window.fetch = patchedFetch;
    };

    const markStatsigReady = (client) => {
      if (!client || typeof client !== "object" || client.__codexPlusFastStartupReadyPatched) return;
      client.__codexPlusFastStartupReadyPatched = true;
      const markReady = () => {
        try {
          if (client.loadingStatus && client.loadingStatus !== "Ready") client.loadingStatus = "Ready";
        } catch {
        }
        try {
          if (typeof client.$emt === "function") client.$emt({ name: "values_updated" });
        } catch {
        }
      };
      if (typeof client.initializeAsync === "function") {
        const originalInitializeAsync = client.initializeAsync.bind(client);
        client.initializeAsync = (...args) => Promise.race([
          originalInitializeAsync(...args).catch(() => null),
          new Promise((resolve) => window.setTimeout(() => resolve(null), timeoutMs)),
        ]).finally(markReady);
      }
      markReady();
    };

    const statsigClients = () => {
      const root = window.__STATSIG__ || globalThis.__STATSIG__;
      if (!root || typeof root !== "object") return [];
      const clients = [root.firstInstance, typeof root.instance === "function" ? root.instance() : null];
      if (root.instances && typeof root.instances === "object") clients.push(...Object.values(root.instances));
      return clients.filter((client, index, array) => client && typeof client === "object" && array.indexOf(client) === index);
    };

    const patchStatsigRoot = () => statsigClients().forEach(markStatsigReady);

    patchFetch();
    patchStatsigRoot();
    const startedAt = Date.now();
    const timer = window.setInterval(() => {
      patchFetch();
      patchStatsigRoot();
      if (Date.now() - startedAt > 5000) window.clearInterval(timer);
    }, 50);
  }

  const upstreamBranchSelectionKey = "codexUpstreamBranchSelection";

  const upstreamProjectContextKey = "codexUpstreamProjectContext";

  const zedRemoteOpenInMenuVersion = "1";

  const zedRemoteOpenInMenuActivationWindowMs = 600;

  const codexThreadServiceTierVersion = "1";

  const codexMenuLocalizationVersion = "1";

  const codexMenuLocalizationMap = new Map([
    ["Toggle Sidebar", "切换侧边栏"],
    ["Toggle Bottom Panel", "切换底部面板"],
    ["Toggle Pinned Summary", "切换置顶摘要"],
    ["Open Terminal", "打开终端"],
    ["Toggle File Tree", "切换文件树"],
    ["Open Browser Tab", "打开浏览器标签页"],
    ["Focus Browser Address Bar", "聚焦浏览器地址栏"],
    ["Reload Browser Page", "重新加载浏览器页面"],
    ["Force Reload Browser Page", "强制重新加载浏览器页面"],
    ["Toggle Browser Panel", "切换浏览器面板"],
    ["Toggle Side Panel", "切换侧边面板"],
    ["Find", "查找"],
    ["Previous Chat", "上一个对话"],
    ["Next Chat", "下一个对话"],
    ["Back", "后退"],
    ["Forward", "前进"],
    ["Zoom In", "放大"],
    ["Zoom Out", "缩小"],
    ["Actual Size", "实际大小"],
    ["Toggle Full Screen", "切换全屏"],
    ["Keyboard Shortcuts", "键盘快捷键"],
    ["Open command menu", "打开命令菜单"],
    ["Search Chats…", "搜索对话…"],
    ["Search Files…", "搜索文件…"],
    ["New Chat", "新建对话"],
    ["Quick Chat", "快速对话"],
    ["Open in New Window", "在新窗口打开"],
    ["Archive chat", "归档对话"],
    ["Pin/unpin chat", "置顶/取消置顶对话"],
    ["Settings…", "设置…"],
    ["Open Folder…", "打开文件夹…"],
    ["Close Tab", "关闭标签页"],
    ["Close", "关闭"],
    ["New Window", "新建窗口"],
    ["Copy conversation path", "复制对话路径"],
    ["Copy deeplink", "复制深层链接"],
    ["Copy session id", "复制会话 ID"],
    ["Copy working directory", "复制工作目录"],
  ]);

  let upstreamBranchDefaultsCache = new Map();

  const upstreamBranchDefaultsCacheTtlMs = 5000;

  const upstreamRemoteBranchDefaultsCacheTtlMs = 30000;

  let upstreamBranchDefaultsInflight = new Map();

  const upstreamProjectContextTtlMs = 10 * 60 * 1000;

  const branchWorktreePathAttribute = "data-codex-branch-worktree-path";
