  async function setCodexGlobalState(key, value) {
    return await codexStateCall("set-global-state", { params: { key, value } });
  }

  function dispatchCodexPlusMessage(dispatcher, type, payload) {
    const message = codexServiceTierRequestOverride({ ...(payload || {}), type });
    const nextType = message?.type || type;
    const { type: _type, ...nextPayload } = message || {};
    if (nextType === "browser-use-session-route-capture") {
      observeCodexRemoteSessionNotification({ type: nextType, params: nextPayload });
    }
    return dispatcher.__codexServiceTierOriginalDispatchMessage(nextType, nextPayload);
  }

  function objectGlobalState(value) {
    return value && typeof value === "object" && !Array.isArray(value) ? { ...value } : {};
  }

  function uniqueValues(values) {
    return Array.from(new Set(values.filter((value) => typeof value === "string" && value.trim().length > 0)));
  }

  let codexModelCatalog = { status: "loading", model: "", default_model: "", model_provider: "", codex_model_provider: "", provider_name: "", models: [], officialModels: [], trustedOfficialModels: [], modelMetadata: {}, sources: [], responses_api: { status: "unknown", message: "" } };
  let codexModelCatalogLoadedAt = 0;
  let codexModelCatalogPromise = null;
  let codexModelWhitelistRefreshTimer = 0;
  let codexModelWhitelistRefreshUntil = 0;
  const codexPlusModelListRequestIds = new Set();
  const codexPlusModelListRequestHosts = new Map();
  // 跨重注入保留原生数据快照；WeakMap 仅持有模型集合，不缓存 DOM。
  const codexPlusModelCollectionSnapshots = window.__codexPlusModelCollectionSnapshots
    || (window.__codexPlusModelCollectionSnapshots = new WeakMap());
  const codexPlusModelDefaultSnapshots = window.__codexPlusModelDefaultSnapshots
    || (window.__codexPlusModelDefaultSnapshots = new WeakMap());

  if (window.__CODEX_PLUS_TEST_SERVICE_TIER__) {
    window.__codexPlusServiceTierTest = {
      applyServiceTierOverride: (method, params, threadIdHint = "") => applyCodexServiceTierRequestOverride(method, params, threadIdHint),
      applyProviderOverride: (method, params) => applyCodexRemoteSessionProviderOverride(method, params),
      remoteSessionStartedThreadId: (value) => codexRemoteSessionStartedThreadId(value),
      observeRemoteSessionNotification: (value) => observeCodexRemoteSessionNotification(value),
      installRemoteSessionRecoveryListener: () => installCodexRemoteSessionRecoveryListener(),
      installRemoteSessionDispatcherSubscription: (dispatcher, assetPrefix = "test") => installCodexRemoteSessionDispatcherSubscription(dispatcher, assetPrefix),
      dispatchMessage: (dispatcher, type, payload) => dispatchCodexPlusMessage(dispatcher, type, payload),
      requestOverride: (message) => codexServiceTierRequestOverride(message),
      diagnostics: () => [...(window.__codexPlusServiceTierTestDiagnostics || [])],
      currentModelName: () => codexServiceTierCurrentModelName(),
      fastAvailability: (modelName = codexServiceTierCurrentModelName()) => codexServiceTierFastAvailability(modelName),
      modelDescriptor: (modelName) => codexPlusModelDescriptor(modelName),
      normalizeReasoningParams: (method, params) => normalizeCodexModelReasoningParams(method, params),
      patchReasoningMessage: (message) => patchCodexModelReasoningRequestMessage(message),
      patchModelArray: (models, allowEmpty = false) => patchModelArray(models, allowEmpty),
      setModelCatalog: (catalog = {}) => {
        codexModelCatalog = {
          status: "ok",
          model: "",
          default_model: "",
          model_provider: "",
          codex_model_provider: "",
          provider_name: "",
          models: [],
          officialModels: [],
          trustedOfficialModels: [],
          modelMetadata: {},
          sources: [],
          responses_api: { status: "unknown", message: "" },
          ...catalog,
        };
        codexModelCatalogLoadedAt = Date.now();
        codexModelCatalogPromise = null;
      },
      loadModelCatalog: (force = false) => loadCodexModelCatalog(force),
      patchModelContainer: (value, hostId = "local") => patchModelContainer(value, hostId),
      patchModelArray: (models, allowEmpty = false, hostId = "local") => patchModelArray(models, allowEmpty, hostId),
      patchStatsigModelDynamicConfig: (config, hostId = "local") => patchStatsigModelDynamicConfig(config, hostId),
      patchModelJsonResponse,
      patchMcpModelResponseData,
      recordModelListRequest,
      setBackendSettings: (settings = {}) => {
        codexPlusBackendSettings = { ...codexPlusBackendSettings, ...settings };
        codexPlusBackendSettingsLoaded = true;
      },
      providerPatchEnabled: () => codexRemoteSessionProviderPatchEnabled(),
      providerNormalizationEnabled: () => codexRemoteSessionProviderNormalizationEnabled(),
      settingStorageFromModule: codexSettingStorageFromModule,
      stateApiFromModule: codexStateApiFromModule,
      dispatcherFromModule: codexServiceTierDispatcherFromModule,
      patchAppServerClient: patchAppServerModelRequestClient,
      locateAppServerClientBreakpoint: locateCodexAppServerClientBreakpoint,
      installAppServerClientPrototypePatch: installCodexAppServerClientPrototypePatch,
      appServerClientPrototypeState: () => ({
        hasClass: typeof window.__codexPlusAppServerClientClass === "function",
        installed: window.__codexPlusAppServerClientPrototypePatchInstalled || null,
      }),
    };
    return;
  }

  function codexPlusModelUnlockEnabled() {
    // 被拓展顶替时让位（停用集合由 96-builtin-extensions.js 按下发的顶替状态维护）。
    return !!codexPlusSettings().modelWhitelistUnlock && !globalThis.__codexPlusBuiltinInactive?.has("model-whitelist-unlock");
  }

  function codexPlusModelNames() {
    return uniqueValues([
      codexModelCatalog.default_model,
      codexModelCatalog.model,
      ...(Array.isArray(codexModelCatalog.models) ? codexModelCatalog.models : []),
    ]);
  }

  async function loadCodexModelCatalog(force = false) {
    if (!force && codexModelCatalogPromise) return codexModelCatalogPromise;
    if (!force && codexModelCatalogLoadedAt && Date.now() - codexModelCatalogLoadedAt < 10000) return codexModelCatalog;
    codexModelCatalogPromise = readCodexAppServerPreparation("/codex-model-catalog", {})
      .then(async (result) => {
        codexModelCatalog = result && typeof result === "object" ? result : { status: "failed", model: "", default_model: "", model_provider: "", codex_model_provider: "", provider_name: "", models: [], officialModels: [], trustedOfficialModels: [], modelMetadata: {}, sources: [], responses_api: { status: "unknown", message: "" } };
        if ((!codexModelCatalog.models || codexModelCatalog.models.length === 0) && codexModelCatalog.status === "not_configured") {
          try {
            const settingsResp = await readCodexAppServerPreparation("/settings/get", {});
            if (settingsResp && settingsResp.relayProfiles && Array.isArray(settingsResp.relayProfiles)) {
              const activeId = settingsResp.activeRelayId || "";
              const profile = settingsResp.relayProfiles.find(p => p.id === activeId);
              if (profile && profile.modelList) {
                const extraModels = profile.modelList.split(/[\r\n,]+/).map(s => s.trim()).filter(Boolean);
                if (extraModels.length > 0) {
                  codexModelCatalog.models = extraModels;
                  codexModelCatalog.default_model = codexModelCatalog.default_model || extraModels[0];
                  sendCodexPlusDiagnostic("model_catalog_fallback_applied", { count: extraModels.length });
                }
              }
            }
          } catch (fallbackError) {
            sendCodexPlusDiagnostic("model_catalog_fallback_error", { error: String(fallbackError?.message || fallbackError) });
          }
        }
        codexModelCatalogLoadedAt = Date.now();
        renderCodexPlusMenu();
        scheduleCodexModelWhitelistRefresh();
        return codexModelCatalog;
      })
      .catch((error) => {
        codexModelCatalog = { status: "failed", message: String(error?.message || error), model: "", default_model: "", model_provider: "", codex_model_provider: "", provider_name: "", models: [], officialModels: [], trustedOfficialModels: [], modelMetadata: {}, sources: [], responses_api: { status: "unknown", message: "" } };
        codexModelCatalogLoadedAt = Date.now();
        return codexModelCatalog;
      })
      .finally(() => {
        codexModelCatalogPromise = null;
      });
    return codexModelCatalogPromise;
  }

  function codexPlusModelMetadata(modelName) {
    const normalizedName = codexServiceTierModelFromValue(modelName);
    return codexCatalogMetadata(normalizedName)
      || (normalizedName !== String(modelName || "").trim()
        ? codexCatalogMetadata(String(modelName || "").trim())
        : null);
  }

  function modelReasoningEfforts(modelName) {
    const supported = codexPlusModelMetadata(modelName)?.supportedReasoningEfforts;
    if (Array.isArray(supported) && supported.length > 0) {
      return supported.map((entry) => typeof entry === "string" ? entry : { ...entry });
    }
    return [];
  }

  function codexBuiltInReasoningMetadata(modelName) {
    const metadata = codexPlusModelMetadata(modelName);
    if (!metadata || typeof metadata !== "object") return null;
    const supportedValues = Array.isArray(metadata.supportedReasoningEfforts)
      ? metadata.supportedReasoningEfforts
      : (Array.isArray(metadata.supported_reasoning_levels) ? metadata.supported_reasoning_levels : []);
    const supported = supportedValues
      .map((entry) => typeof entry === "string" ? entry : entry?.reasoningEffort || entry?.effort)
      .map((entry) => String(entry || "").trim())
      .filter(Boolean);
    if (!supported.length) return null;
    return {
      defaultEffort: String(metadata.defaultReasoningEffort || metadata.default_reasoning_level || "").trim(),
      supported,
    };
  }

  function codexModelReasoningEffortValues(modelName) {
    return modelReasoningEfforts(modelName)
      .map((entry) => typeof entry === "string" ? entry : entry?.reasoningEffort)
      .map((entry) => String(entry || "").trim())
      .filter(Boolean);
  }

  function codexModelRequestMethodSupportsReasoning(method) {
    return new Set(["thread/settings/update", "turn/settings/update", "thread/start", "thread/resume", "turn/start"]).has(String(method || ""));
  }

  function codexModelReasoningTarget(method, params) {
    const requestMethod = appServerModelRequestMethod(String(method || ""), params);
    if (requestMethod === "send-cli-request-for-host"
        && params?.params
        && typeof params.params === "object") {
      return codexModelReasoningTarget(params.method, params.params);
    }
    if (!codexModelRequestMethodSupportsReasoning(requestMethod)
        || !params
        || typeof params !== "object") {
      return null;
    }
    if (params.params && typeof params.params === "object") {
      return {
        method: requestMethod,
        params: params.params,
        apply: (nextParams) => ({ ...params, params: nextParams }),
      };
    }
    return { method: requestMethod, params, apply: (nextParams) => nextParams };
  }

  function normalizeCodexModelReasoningParams(method, params) {
    const providerParams = applyCodexRemoteSessionProviderOverride(method, params);
    const target = codexModelReasoningTarget(method, providerParams);
    if (!target) return providerParams;
    const modelKey = ["model", "modelId", "model_id"].find((key) => typeof target.params[key] === "string" && target.params[key].trim());
    if (!modelKey) return providerParams;
    const modelName = target.params[modelKey].trim();
    const collaboration = target.params.collaborationMode;
    const collaborationSettings = collaboration?.settings && typeof collaboration.settings === "object"
      ? collaboration.settings : null;
    let nextParams = target.params;
    // app-server 的协作模式会覆盖外层模型；来源切换后必须同步旧缓存中的账号别名。
    if (collaborationSettings && collaborationSettings.model !== modelName) {
      nextParams = { ...nextParams, collaborationMode: {
        ...collaboration, settings: { ...collaborationSettings, model: modelName },
      } };
    }
    const supported = codexModelReasoningEffortValues(modelName);
    if (!supported.length) return nextParams === target.params ? providerParams : target.apply(nextParams);
    const effortKey = ["reasoningEffort", "reasoning_effort", "effort"].find((key) => Object.prototype.hasOwnProperty.call(target.params, key));
    const current = String((effortKey ? target.params[effortKey] : collaborationSettings?.reasoning_effort) || "").trim();
    const configuredDefault = String(
      codexPlusModelMetadata(modelName)?.defaultReasoningEffort
      || codexBuiltInReasoningMetadata(modelName)?.defaultEffort
      || ""
    ).trim();
    const nextEffort = supported.includes(current) ? current
      : supported.includes(configuredDefault) ? configuredDefault : supported[0];
    if ((effortKey || target.method === "thread/settings/update") && current !== nextEffort) {
      nextParams = { ...nextParams, [effortKey || "reasoningEffort"]: nextEffort };
    }
    if (collaborationSettings && collaborationSettings.reasoning_effort !== nextEffort) {
      nextParams = { ...nextParams, collaborationMode: { ...collaboration, settings: {
        ...collaborationSettings, model: modelName, reasoning_effort: nextEffort,
      } } };
    }
    if (nextParams === target.params) return providerParams;
    sendCodexPlusDiagnostic("model_reasoning_effort_normalized", {
      method: target.method,
      model: modelName,
      previousEffort: current || null,
      nextEffort,
    });
    return target.apply(nextParams);
  }

  function patchCodexModelReasoningRequestMessage(message) {
    if (!message || typeof message !== "object") return message;
    if (message.type === "fetch" && typeof message.url === "string") {
      let body = message.body;
      let parsed = body;
      if (typeof body === "string" && body.trim()) {
        try {
          parsed = JSON.parse(body);
        } catch {
          return message;
        }
      }
      const nextBody = normalizeCodexModelReasoningParams(message.url, parsed);
      if (nextBody === parsed) return message;
      return { ...message, body: typeof body === "string" ? JSON.stringify(nextBody) : nextBody };
    }
    if ((message.type === "mcp-request" || message.type === "worker-request")
        && message.request
        && typeof message.request === "object") {
      const params = normalizeCodexModelReasoningParams(message.request.method, message.request.params);
      if (params === message.request.params) return message;
      return { ...message, request: { ...message.request, params } };
    }
    if (message.type === "send-cli-request-for-host") {
      const params = normalizeCodexModelReasoningParams(message.method, message.params);
      return params === message.params ? message : { ...message, params };
    }
    if (message.type === "start-turn-for-host" && message.params && typeof message.params === "object") {
      const params = normalizeCodexModelReasoningParams("turn/start", message.params);
      return params === message.params ? message : { ...message, params };
    }
    // 新版桌面端把 `turn/start`、`thread/start`、`thread/resume`、
    // `thread/settings/update` 这类 app-server 请求以**裸 JSON-RPC 形态**
    // （`{ method, params }`，没有 `type`，或 type 不在上面那几种里）派发出去。
    // 缺少这一支时 provider 覆盖永远不会执行：会话一直留在内置的 `openai`
    // provider 上直连 chatgpt.com，第三方模型被 ChatGPT 账号侧拒绝，报
    // “The 'X' model is not supported when using Codex with a ChatGPT account.”。
    const directMethod = String(message.method || "").trim();
    if (directMethod
        && codexRemoteSessionProviderRequestMethod(directMethod)
        && message.params
        && typeof message.params === "object") {
      const params = normalizeCodexModelReasoningParams(directMethod, message.params);
      return params === message.params ? message : { ...message, params };
    }
    const nested = message.request && typeof message.request === "object" ? message.request : null;
    const nestedMethod = nested ? String(nested.method || "").trim() : "";
    if (nestedMethod
        && codexRemoteSessionProviderRequestMethod(nestedMethod)
        && nested.params
        && typeof nested.params === "object") {
      const params = normalizeCodexModelReasoningParams(nestedMethod, nested.params);
      if (params !== nested.params) return { ...message, request: { ...nested, params } };
    }
    return message;
  }

  function installCodexModelRequestBridgePatch() {
    installCodexNativeThreadMetadataReader();
    // provider 覆盖是第三方接入的命脉，不能只挂在“模型白名单解锁”这一个开关上：
    // 关掉解锁就会静默退回内置 openai provider，第三方模型全被账号侧拒绝。
    if (!codexPlusModelUnlockEnabled()
        && !codexPlusSettings().quotaResume
        && !codexRemoteSessionProviderOverrideEnabled()) return;
    const bridge = window.electronBridge;
    if (!bridge || typeof bridge.sendMessageFromView !== "function") return;
    if (Object.isFrozen(bridge)) return;
    if (bridge.__codexPlusModelRequestBridgePatch === codexModelRequestBridgePatchVersion) return;
    const originalSendMessageFromView = bridge.sendMessageFromView.bind(bridge);
    bridge.sendMessageFromView = function codexPlusModelPatchedSendMessageFromView(message) {
      let nextMessage = message;
      try {
        if (codexPlusModelUnlockEnabled() || codexRemoteSessionProviderOverrideEnabled()) {
          nextMessage = patchCodexModelReasoningRequestMessage(message);
        }
        rememberCodexQuotaResumeBridgeMessage(nextMessage);
      } catch (error) {
        sendCodexPlusDiagnostic("model_reasoning_bridge_patch_failed", {
          errorName: error?.name || "",
          errorMessage: error?.message || String(error),
        });
        if ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) === true) throw error;
      }
      return originalSendMessageFromView(nextMessage);
    };
    bridge.__codexPlusModelRequestBridgePatch = codexModelRequestBridgePatchVersion;
    if (bridge.__codexPlusModelRequestBridgePatch === codexModelRequestBridgePatchVersion
        && bridge.sendMessageFromView.name === "codexPlusModelPatchedSendMessageFromView") {
      sendCodexPlusDiagnostic("model_reasoning_bridge_patch_installed", {});
    }
  }

  function codexPlusModelDisplayName(metadata, modelName, fallbackDisplayName = "") {
    const displayName = typeof metadata?.displayName === "string" && metadata.displayName.trim()
      ? metadata.displayName.trim()
      : (fallbackDisplayName || modelName);
    // Source routes already carry a complete label. Legacy catalogs may still use a suffix.
    if (metadata?.sourceKind) return displayName;
    const displaySuffix = typeof metadata?.displaySuffix === "string" ? metadata.displaySuffix.trim() : "";
    return displaySuffix && !displayName.endsWith(displaySuffix)
      ? `${displayName}${displaySuffix}`
      : displayName;
  }

  function applyCodexPlusModelMetadata(descriptor, modelName) {
    const metadata = codexPlusModelMetadata(modelName);
    if (!descriptor || !metadata) return false;
    let changed = false;
    for (const key of ["description", "defaultReasoningEffort", "contextWindow", "maxContextWindow", "context_window", "max_context_window"]) {
      const valid = typeof metadata[key] === "string" ? !!metadata[key] : Number.isFinite(metadata[key]) && metadata[key] > 0;
      if (valid && descriptor[key] !== metadata[key]) {
        descriptor[key] = metadata[key];
        changed = true;
      }
    }
    const displayName = typeof codexPlusModelDisplayName === "function"
      ? codexPlusModelDisplayName(metadata, modelName, descriptor.displayName)
      : metadata.displayName || descriptor.displayName || modelName;
    if (descriptor.displayName !== displayName) {
      descriptor.displayName = displayName;
      changed = true;
    }
    if (metadata.providerId && descriptor.modelProvider !== metadata.providerId) {
      descriptor.modelProvider = metadata.providerId;
      changed = true;
    }
    if (Number.isInteger(metadata.priority) && descriptor.priority !== metadata.priority) {
      descriptor.priority = metadata.priority;
      changed = true;
    }
    if (Array.isArray(metadata.supportedReasoningEfforts) && metadata.supportedReasoningEfforts.length > 0) {
      const nextEfforts = modelReasoningEfforts(modelName);
      if (JSON.stringify(descriptor.supportedReasoningEfforts || []) !== JSON.stringify(nextEfforts)) {
        descriptor.supportedReasoningEfforts = nextEfforts;
        changed = true;
      }
    }
    for (const key of ["additionalSpeedTiers", "serviceTiers"]) {
      if (!Array.isArray(metadata[key])) continue;
      const nextValue = metadata[key].map((entry) => entry && typeof entry === "object" ? { ...entry } : entry);
      if (JSON.stringify(descriptor[key] || []) !== JSON.stringify(nextValue)) {
        descriptor[key] = nextValue;
        changed = true;
      }
    }
    return changed;
  }

  function codexPlusModelDescriptor(modelName, availableModels = []) {
    const metadata = codexPlusModelMetadata(modelName);
    const displayName = codexPlusModelDisplayName(metadata, modelName);
    const capabilitySlug = String(metadata?.capabilitySlug || "").trim();
    const officialTemplate = capabilitySlug && Array.isArray(availableModels)
      ? availableModels.find((item) => String(item?.model || item?.slug || "").trim().toLowerCase() === capabilitySlug.toLowerCase())
      : null;
    return {
      ...(officialTemplate && typeof officialTemplate === "object" ? { ...officialTemplate } : {}),
      model: modelName,
      id: modelName,
      slug: modelName,
      name: modelName,
      displayName,
      description: metadata?.description || codexModelCatalog.provider_name || codexModelCatalog.model_provider || "Custom model",
      hidden: false,
      isDefault: false,
      ...(metadata?.providerId ? { modelProvider: metadata.providerId } : {}),
      ...(Number.isInteger(metadata?.priority) ? { priority: metadata.priority } : {}),
      defaultReasoningEffort: metadata?.defaultReasoningEffort || "",
      supportedReasoningEfforts: modelReasoningEfforts(modelName),
      ...(Array.isArray(metadata?.additionalSpeedTiers)
        ? { additionalSpeedTiers: metadata.additionalSpeedTiers.map((entry) => entry && typeof entry === "object" ? { ...entry } : entry) }
        : {}),
      ...(Array.isArray(metadata?.serviceTiers)
        ? { serviceTiers: metadata.serviceTiers.map((entry) => entry && typeof entry === "object" ? { ...entry } : entry) }
        : {}),
    };
  }

  function modelArrayLooksPatchable(value, allowEmpty = false) {
    return Array.isArray(value)
      && (allowEmpty || value.length > 0)
      && value.every((item) => item && typeof item === "object" && typeof item.model === "string");
  }

  function stringArrayLooksPatchable(value) {
    return Array.isArray(value) && value.every((item) => typeof item === "string");
  }

  function patchModelNameArray(models, hostId) {
    if (!stringArrayLooksPatchable(models)) return false;
    const allowed = codexPureApiAllowedModelSet(hostId);
    let changed = prepareModelCollection(models, allowed);
    const customModels = codexPlusModelNames();
    if (!customModels.length) return changed;
    if (allowed) {
      const retained = models.filter((name) => allowed.has(name));
      if (retained.length !== models.length) {
        models.splice(0, models.length, ...retained);
        changed = true;
      }
    }
    customModels.forEach((modelName) => {
      if (!models.includes(modelName)) {
        models.push(modelName);
        changed = true;
      }
    });
    finishModelCollection(models);
    return changed;
  }

  function modelFilterPolicy(allowed) {
    return allowed ? `${codexRemoteSessionActiveProfile()?.id || ""}:${codexModelCatalog.model || ""}:${JSON.stringify([...allowed].sort())}` : "";
  }

  function sameModelCollection(left, right) {
    return !!right && left.length === right.length && left.every((item, index) => item === right[index]);
  }

  function prepareModelCollection(collection, allowed) {
    const policy = modelFilterPolicy(allowed);
    const previous = codexPlusModelCollectionSnapshots.get(collection);
    let current = [...collection];
    let changed = false;
    if (previous && previous.policy !== policy && sameModelCollection(current, previous.patched)) {
      if (!sameModelCollection(current, previous.original)) {
        if (Array.isArray(collection)) collection.splice(0, collection.length, ...previous.original);
        else { collection.clear(); previous.original.forEach((item) => collection.add(item)); }
        current = [...collection];
        changed = true;
      }
    }
    if (!allowed) codexPlusModelCollectionSnapshots.delete(collection);
    else if (!previous || previous.policy !== policy || !sameModelCollection(current, previous.patched)) {
      codexPlusModelCollectionSnapshots.set(collection, { policy, original: current, patched: null });
    }
    return changed;
  }

  function finishModelCollection(collection) {
    const snapshot = codexPlusModelCollectionSnapshots.get(collection);
    if (snapshot) snapshot.patched = [...collection];
  }

  function patchModelDefaults(value, allowed) {
    const policy = modelFilterPolicy(allowed);
    const previous = codexPlusModelDefaultSnapshots.get(value);
    let changed = false;
    if (previous && previous.policy !== policy) {
      for (const [key, state] of Object.entries(previous.fields)) {
        if (value[key] === state.patched && value[key] !== state.original) {
          value[key] = state.original;
          changed = true;
        }
      }
      codexPlusModelDefaultSnapshots.delete(value);
    }
    if (!allowed) return changed;
    const snapshot = codexPlusModelDefaultSnapshots.get(value) || { policy, fields: {} };
    const fallback = [codexModelCatalog.model, codexModelCatalog.default_model, ...allowed]
      .find((name) => typeof name === "string" && allowed.has(name));
    for (const key of ["defaultModel", "default_model"]) {
      const original = value[key];
      const name = typeof original === "string" ? original : original?.model || original?.id || original?.slug;
      if (!name || allowed.has(name)) continue;
      const patched = typeof original === "string" ? fallback : codexPlusModelDescriptor(fallback);
      if (!snapshot.fields[key] || original !== snapshot.fields[key].patched) {
        snapshot.fields[key] = { original, patched };
      }
      value[key] = patched;
      changed = true;
    }
    codexPlusModelDefaultSnapshots.set(value, snapshot);
    return changed;
  }

  // 只收窄当前纯 API 供应商的有效目录；名单里的 GPT 也是可用模型，不能按前缀删。
  // 目录失败/未配置/仍属于上一个供应商时继续保留原生列表。
  function modelResponseHostId(...values) {
    let local = false;
    for (const value of values) {
      let candidates;
      try { candidates = typeof value === "string" ? [value] : [value?.hostId, value?.host_id]; }
      catch { return ""; }
      for (const candidate of candidates) {
        if (typeof candidate !== "string" || !candidate.trim()) continue;
        const host = candidate.trim();
        if (host !== "local") return host;
        local = true;
      }
    }
    return local ? "local" : "";
  }

  function codexPureApiAllowedModelSet(hostId) {
    if (hostId !== "local") return null;
    const profile = codexRemoteSessionActiveProfile();
    if (!codexPlusModelUnlockEnabled() || !codexPlusBackendSettingsLoaded
        || profile?.relayMode !== "pureApi" || codexModelCatalog.status !== "ok") return null;
    const profileId = String(profile.id || "").trim();
    const belongsToProfile = profileId && (codexModelCatalog.model_provider === profileId
      || (Array.isArray(codexModelCatalog.sources)
        && codexModelCatalog.sources.some((source) => source?.id === `relay-profile:${profileId}`)));
    const names = codexPlusModelNames();
    return belongsToProfile && names.length ? new Set(names) : null;
  }

  function patchModelArray(models, allowEmpty = false, hostId) {
    if (!modelArrayLooksPatchable(models, allowEmpty)) return false;
    const allowed = codexPureApiAllowedModelSet(hostId);
    let changed = prepareModelCollection(models, allowed);
    const customModels = codexPlusModelNames();
    if (!customModels.length) return changed;
    if (allowed) {
      const retained = models.filter((item) => allowed.has(item.model));
      if (retained.length !== models.length) {
        models.splice(0, models.length, ...retained);
        changed = true;
      }
    }
    const existing = new Map(models.map((item) => [item.model, item]));
    models.forEach((item) => {
      if (customModels.includes(item.model)) {
        if (item.hidden !== false) {
          item.hidden = false;
          changed = true;
        }
        if (applyCodexPlusModelMetadata(item, item.model)) changed = true;
      }
    });
    customModels.forEach((modelName) => {
      if (!existing.has(modelName)) {
        models.push(codexPlusModelDescriptor(modelName, models));
        changed = true;
      }
    });
    finishModelCollection(models);
    return changed;
  }

  function patchModelContainer(value, hostId) {
    if (!value || typeof value !== "object") return false;
    hostId = modelResponseHostId(hostId, value, value.message, value.result);
    let changed = false;
    if (patchModelArray(value.models, "defaultModel" in value || "availableModels" in value, hostId)) changed = true;
    if (patchModelNameArray(value.models, hostId)) changed = true;
    if (patchModelArray(value.data, false, hostId)) changed = true;
    if (patchModelArray(value.result, false, hostId)) changed = true;
    if (patchModelArray(value.pages?.[0]?.data, false, hostId)) changed = true;
    if (patchModelArray(value.result?.data, false, hostId)) changed = true;
    if (patchModelArray(value.result?.models, false, hostId)) changed = true;
    if (patchModelArray(value.message?.result?.data, false, hostId)) changed = true;
    if (patchModelArray(value.message?.result?.models, false, hostId)) changed = true;
    const names = codexPlusModelNames();
    const allowed = codexPureApiAllowedModelSet(hostId);
    if (value.availableModels instanceof Set) {
      if (prepareModelCollection(value.availableModels, allowed)) changed = true;
      if (allowed) for (const name of value.availableModels) {
        if (!allowed.has(name)) { value.availableModels.delete(name); changed = true; }
      }
      names.forEach((name) => {
        if (!value.availableModels.has(name)) {
          value.availableModels.add(name);
          changed = true;
        }
      });
      finishModelCollection(value.availableModels);
    }
    if (value.available_models instanceof Set) {
      if (prepareModelCollection(value.available_models, allowed)) changed = true;
      if (allowed) for (const name of value.available_models) {
        if (!allowed.has(name)) { value.available_models.delete(name); changed = true; }
      }
      names.forEach((name) => {
        if (!value.available_models.has(name)) {
          value.available_models.add(name);
          changed = true;
        }
      });
      finishModelCollection(value.available_models);
    }
    if (Array.isArray(value.availableModels)) {
      if (patchModelNameArray(value.availableModels, hostId)) changed = true;
    }
    if (Array.isArray(value.available_models)) {
      if (patchModelNameArray(value.available_models, hostId)) changed = true;
    }
    if (Array.isArray(value.hiddenModels)) {
      const before = value.hiddenModels.length;
      value.hiddenModels = value.hiddenModels.filter((name) => !names.includes(name));
      if (value.hiddenModels.length !== before) changed = true;
    }
    if (Array.isArray(value.hidden_models)) {
      const before = value.hidden_models.length;
      value.hidden_models = value.hidden_models.filter((name) => !names.includes(name));
      if (value.hidden_models.length !== before) changed = true;
    }
    if (patchModelDefaults(value, allowed)) changed = true;
    return changed;
  }

  function modelJsonResponseLooksPatchable(payload) {
    if (!payload || typeof payload !== "object") return false;
    const descriptorArrays = [
      payload.models,
      payload.data,
      payload.result,
      payload.pages?.[0]?.data,
      payload.result?.data,
      payload.result?.models,
      payload.message?.result?.data,
      payload.message?.result?.models,
    ];
    if (descriptorArrays.some((value) => modelArrayLooksPatchable(value))) return true;
    const hasModelContainerSignal = "defaultModel" in payload
      || "default_model" in payload
      || "availableModels" in payload
      || "available_models" in payload
      || "hiddenModels" in payload
      || "hidden_models" in payload
      || "modelMetadata" in payload
      || "model_metadata" in payload;
    return hasModelContainerSignal && Array.isArray(payload.models)
      && payload.models.every((value) => typeof value === "string");
  }

  async function patchModelJsonResponse(payload) {
    if (!codexPlusModelUnlockEnabled()) return payload;
    if (!codexPlusModelNames().length) await loadCodexModelCatalog();
    if (!modelJsonResponseLooksPatchable(payload)) return payload;
    try {
      patchModelContainer(payload, modelResponseHostId(payload, payload.result, payload.message));
    } catch (error) {
      window.__codexPlusModelPatchFailures = window.__codexPlusModelPatchFailures || [];
      window.__codexPlusModelPatchFailures.push(String(error?.stack || error));
    }
    return payload;
  }

  function installModelJsonResponsePatch() {
    if (window.__codexPlusModelJsonResponsePatchInstalled === "3") return;
    window.__codexPlusModelJsonResponsePatchInstalled = "3";
    window.__codexPlusModelJsonResponseOriginals = window.__codexPlusModelJsonResponseOriginals || {};
    const originals = window.__codexPlusModelJsonResponseOriginals;
    originals.responseJson = originals.responseJson || Response.prototype.json;
    if (typeof originals.responseJson !== "function") return;
    Response.prototype.json = async function codexPlusPatchedResponseJson(...args) {
      const payload = await originals.responseJson.apply(this, args);
      return await patchModelJsonResponse(payload);
    };
  }

  function patchStatsigModelDynamicConfig(config, hostId) {
    const value = config?.value;
    if (!value || typeof value !== "object") return config;
    const availableModels = Array.isArray(value.available_models) ? [...value.available_models] : [];
    const collectionSnapshot = codexPlusModelCollectionSnapshots.get(value.available_models);
    if (collectionSnapshot) codexPlusModelCollectionSnapshots.set(availableModels, { ...collectionSnapshot });
    const nextValue = { ...value, available_models: availableModels };
    const defaultSnapshot = codexPlusModelDefaultSnapshots.get(value);
    if (defaultSnapshot) codexPlusModelDefaultSnapshots.set(nextValue, { ...defaultSnapshot, fields: { ...defaultSnapshot.fields } });
    hostId = modelResponseHostId(hostId, value);
    const listChanged = patchModelNameArray(availableModels, hostId);
    const defaultsChanged = patchModelDefaults(nextValue, codexPureApiAllowedModelSet(hostId));
    if (!listChanged && !defaultsChanged) return config;
    try {
      config.value = nextValue;
    } catch {
      return { ...config, value: nextValue };
    }
    return config;
  }

  function statsigClients() {
    const root = window.__STATSIG__ || globalThis.__STATSIG__;
    if (!root || typeof root !== "object") return [];
    const clients = [root.firstInstance, typeof root.instance === "function" ? root.instance() : null];
    if (root.instances && typeof root.instances === "object") clients.push(...Object.values(root.instances));
    return clients.filter((client, index, array) => client && typeof client === "object" && array.indexOf(client) === index);
  }

  function patchStatsigModelWhitelist() {
    statsigClients().forEach((client) => {
      if (typeof client.getDynamicConfig !== "function") return;
      if (client.__codexPlusModelWhitelistPatched !== "3") {
        const originalGetDynamicConfig = client.getDynamicConfig.bind(client);
        client.getDynamicConfig = (name, options) => {
          const result = originalGetDynamicConfig(name, options);
          return String(name) === "107580212" ? patchStatsigModelDynamicConfig(result) : result;
        };
        client.__codexPlusModelWhitelistPatched = "3";
      }
      try {
        patchStatsigModelDynamicConfig(client.getDynamicConfig("107580212", { disableExposureLog: true }));
      } catch {
      }
    });
  }

  function patchAppServerModelMessages() {
    if (window.__codexPlusModelMessagePatchInstalled === "4") return;
    window.__codexPlusModelMessagePatchInstalled = "4";
    // Codex may have registered its message listener before this script was attached.
    // Patch on data access so its listener sees the catalog even in an existing document.
    const messageData = Object.getOwnPropertyDescriptor(MessageEvent.prototype, "data");
    if (messageData?.configurable && typeof messageData.get === "function") {
      Object.defineProperty(MessageEvent.prototype, "data", {
        ...messageData,
        get() {
          const data = messageData.get.call(this);
          try {
            patchMcpModelResponseData(data);
          } catch (error) {
            window.__codexPlusModelPatchFailures = window.__codexPlusModelPatchFailures || [];
            window.__codexPlusModelPatchFailures.push(String(error?.stack || error));
          }
          return data;
        },
      });
    }
    window.addEventListener("codex-message-from-view", (event) => {
      try {
        const detail = event?.detail;
        const request = detail?.request;
        if (detail?.type === "mcp-request" && request?.method === "model/list") {
          request.params = { ...(request.params || {}), includeHidden: true };
          if (request.id != null) {
            const requestId = String(request.id);
            recordModelListRequest(requestId, modelResponseHostId(detail, request, request.params));
          }
        }
      } catch (error) {
        window.__codexPlusModelPatchFailures = window.__codexPlusModelPatchFailures || [];
        window.__codexPlusModelPatchFailures.push(String(error?.stack || error));
      }
    }, true);

    window.addEventListener("message", (event) => {
      try {
        patchMcpModelResponseData(event?.data);
      } catch (error) {
        window.__codexPlusModelPatchFailures = window.__codexPlusModelPatchFailures || [];
        window.__codexPlusModelPatchFailures.push(String(error?.stack || error));
      }
    }, true);
  }

  function recordModelListRequest(requestId, hostId) {
    if (codexPlusModelListRequestIds.has(requestId)
        && codexPlusModelListRequestHosts.get(requestId) !== hostId) hostId = "";
    codexPlusModelListRequestIds.add(requestId);
    codexPlusModelListRequestHosts.set(requestId, hostId);
    if (codexPlusModelListRequestIds.size > 64) {
      const oldest = codexPlusModelListRequestIds.values().next().value;
      codexPlusModelListRequestIds.delete(oldest);
      codexPlusModelListRequestHosts.delete(oldest);
    }
    window.setTimeout(() => {
      codexPlusModelListRequestIds.delete(requestId);
      if (typeof codexPlusModelListRequestHosts !== "undefined") codexPlusModelListRequestHosts.delete(requestId);
    }, 30_000);
  }

  function patchMcpModelResponseData(data) {
    if (!codexPlusModelUnlockEnabled()) return false;
    if (data?.type !== "mcp-response") return false;
    const message = data.message || data.response;
    const requestId = message?.id != null ? String(message.id) : "";
    const requestMethod = String(data.requestMethod || data.request?.method || "");
    if (requestMethod !== "model/list" && !codexPlusModelListRequestIds.has(requestId)) return false;
    const hostId = typeof modelResponseHostId === "function" ? modelResponseHostId(data, message, typeof codexPlusModelListRequestHosts !== "undefined" ? codexPlusModelListRequestHosts.get(requestId) : null) : "local";
    if (requestId) { codexPlusModelListRequestIds.delete(requestId); if (typeof codexPlusModelListRequestHosts !== "undefined") codexPlusModelListRequestHosts.delete(requestId); }
    let changed = false;
    if (patchModelArray(message?.result?.data, true, hostId)) changed = true;
    if (patchModelArray(message?.result?.models, true, hostId)) changed = true;
    return changed;
  }

  function appServerModelRequestMethod(method, params) {
    if (method === "send-cli-request-for-host" && params?.method) return appServerModelRequestMethod(String(params.method), params.params);
    if (method === "vscode://codex/list-plugins") return "list-plugins";
    if (method === "vscode://codex/plugin/install") return "install-plugin";
    if (method === "vscode://codex/plugin/uninstall") return "uninstall-plugin";
    const requestMethod = String(method || "").startsWith("vscode://codex/")
      ? String(method).slice("vscode://codex/".length)
      : String(method || "");
    if (requestMethod === "list-plugins" || requestMethod === "plugin/list") return "list-plugins";
    if (requestMethod === "plugin/install") return "install-plugin";
    if (requestMethod === "plugin/uninstall") return "uninstall-plugin";
    if (requestMethod === "plugin/read") return "read-plugin";
    if (requestMethod === "plugin/installed") return "installed-plugins";
    if (requestMethod === "plugin/search") return "search-plugins";
    return requestMethod;
  }

  function patchAppServerModelResult(method, result, hostId) {
    hostId = modelResponseHostId(hostId, result);
    if (method !== "list-models-for-host" && method !== "model/list") return result;
    try {
      if (Array.isArray(result)) patchModelArray(result, true, hostId);
      if (Array.isArray(result?.data)) patchModelArray(result.data, true, hostId);
      if (Array.isArray(result?.models)) patchModelArray(result.models, true, hostId);
      sendCodexPlusDiagnostic("model_app_server_result_patched", {
        method,
        modelCount: Array.isArray(result?.data) ? result.data.length : Array.isArray(result?.models) ? result.models.length : Array.isArray(result) ? result.length : null,
      });
    } catch (error) {
      window.__codexPlusModelPatchFailures = window.__codexPlusModelPatchFailures || [];
      window.__codexPlusModelPatchFailures.push(String(error?.stack || error));
    }
    return result;
  }

  function codexPerModelContextEnabled() {
    const profile = codexRemoteSessionActiveProfile();
    if (!profile) return false;
    return [profile.modelWindows, profile.modelAutoCompact, profile.modelMetadata]
      .some((value) => typeof value === "string" && value.trim() && value.trim() !== "{}");
  }

  function codexThreadModelRequestState(method, params, result) {
    if (method === "send-cli-request-for-host" && params?.method) {
      return codexThreadModelRequestState(String(params.method), params.params, result);
    }
    const requestMethod = String(method || "");
    const threadId = String(
      params?.threadId
      || params?.conversationId
      || result?.thread?.id
      || result?.threadId
      || ""
    ).trim();
    const model = String(params?.model || result?.thread?.model || "").trim();
    return { requestMethod, threadId, model };
  }

  async function refreshCodexThreadModelBeforeTurn(client, originalSendRequest, method, params, options) {
    if (method === "send-cli-request-for-host" && params?.method) {
      if ([params.hostId, params.host_id].some((host) => typeof host === "string" && host !== "local")) return null;
      return refreshCodexThreadModelBeforeTurn(client,
        (nestedMethod, nestedParams, nestedOptions) => originalSendRequest(method,
          { ...params, method: nestedMethod, params: nestedParams }, nestedOptions),
        String(params.method), params.params, options);
    }
    if (String(method || "") !== "turn/start" || !codexPerModelContextEnabled()) return null;
    const { threadId, model } = codexThreadModelRequestState(method, params);
    if (!threadId || !model) return null;
    const previousModel = client.__codexPlusThreadModels?.get(threadId) || "";
    if (!previousModel || previousModel === model) return null;
    if (client.hostId === "local" && codexManagedSessionConnection()
        && window.__codexPlusConnectionRecoveryPending?.has(threadId)) {
      throw new Error("此会话正在检查连接，请稍后重新发送");
    }
    let resumeParams = { threadId, model };
    resumeParams = applyCodexRemoteSessionProviderOverride("thread/resume", resumeParams);
    resumeParams = applyCodexManagedSessionConnection("thread/resume", resumeParams, client.hostId);
    try {
      await originalSendRequest("thread/resume", resumeParams, options);
      client.__codexPlusThreadModels.set(threadId, model);
      sendCodexPlusDiagnostic("thread_model_context_refreshed", {
        threadId,
        from: previousModel,
        to: model,
      });
      return true;
    } catch (error) {
      sendCodexPlusDiagnostic("thread_model_context_refresh_failed", {
        threadId,
        from: previousModel,
        to: model,
        errorName: error?.name || "",
        errorMessage: error?.message || String(error),
      });
      return false;
    }
  }

  function patchAppServerModelRequestClient(client) {
    if (!client || typeof client.sendRequest !== "function") return false;
    registerNativeHostClient(client);
    try {
      if (!Object.isExtensible(client)) return false;
      for (const key of [
        "__codexPlusModelRequestPatch",
        "__codexPlusModelOriginalSendRequest",
        "__codexPlusThreadModels",
        "__codexPlusServiceTierOriginalPrewarmThreadStart",
        "sendRequest",
        "prewarmThreadStart",
      ]) {
        const descriptor = Object.getOwnPropertyDescriptor(client, key);
        if (descriptor && descriptor.writable === false && typeof descriptor.set !== "function") return false;
      }
    } catch {
      return false;
    }
    if (client.__codexPlusModelRequestPatch === codexAppServerModelRequestPatchVersion) return true;
    const originalSendRequest = (client.__codexPlusModelOriginalSendRequest || client.sendRequest).bind(client);
    client.__codexPlusModelOriginalSendRequest = originalSendRequest;
    client.__codexPlusThreadModels = client.__codexPlusThreadModels || new Map();
    registerCodexQuotaResumeClient(client);
    client.sendRequest = async function codexPlusModelPatchedSendRequest(method, params, options) {
      const managed = codexPlusPluginNativeInterceptClient(method, params, options,
        (nextOptions) => originalSendRequest(method, params, nextOptions), client.hostId);
      if (managed) return await managed;
      const requestMethod = appServerModelRequestMethod(String(method || ""), params);
      let providerRefreshFailed = false;
      const managedAggregateTurn = requestMethod === "turn/start"
        && client.hostId === "local" && !!codexManagedSessionConnection();
      if (codexRemoteSessionProviderRequestMethod(requestMethod)
          && codexRemoteSessionProviderPatchEnabled()
          && window.__codexSessionDeleteBridge
          && !managedAggregateTurn) {
        const settingsLoaded = await loadBackendSettingsState(codexAppServerPreparationTimeoutMs);
        providerRefreshFailed = !settingsLoaded;
        if (providerRefreshFailed) {
          sendCodexPlusDiagnostic("remote_session_provider_refresh_failed", {});
        }
      }
      if (codexRemoteSessionProviderRequestMethod(requestMethod)
          && codexRemoteSessionProviderOverrideEnabled()
          && !Array.isArray(codexModelCatalog?.routeDescriptors)) {
        await loadCodexModelCatalog();
      }
      const providerParams = providerRefreshFailed
        ? params
        : applyCodexRemoteSessionProviderOverride(requestMethod, params);
      const resumeParams = await prepareCodexPureApiResume(String(method || ""), providerParams, client.hostId, providerRefreshFailed, originalSendRequest);
      const connectedParams = await prepareCodexManagedSessionConnection(String(method || ""), resumeParams, client.hostId, originalSendRequest);
      const nextParams = applyCodexServiceTierRequestOnly(requestMethod, connectedParams);
      const quotaResumeTarget = codexQuotaResumeAppServerRequestTarget(method, nextParams);
      if (quotaResumeTarget?.requestMethod === "turn/start") {
        rememberCodexQuotaResumeTurnRequest(
          client,
          quotaResumeTarget.params,
          quotaResumeTarget.threadIdHint,
        );
      } else if (quotaResumeTarget) {
        mergeCodexQuotaResumeThreadSettings(
          client,
          quotaResumeTarget.params,
          quotaResumeTarget.threadIdHint,
        );
      }
      const modelContextRefresh = await refreshCodexThreadModelBeforeTurn(
        client,
        originalSendRequest,
        method,
        nextParams,
        options
      );
      if ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) !== true) {
        const managedTurn = await sendCodexManagedSessionTurn(client, originalSendRequest, String(method || ""), nextParams, options, modelContextRefresh);
        if (managedTurn) return managedTurn.result;
      }
      let result;
      try {
        result = await sendAppServerRequestThroughExchange(client, method, nextParams, () => originalSendRequest(method, nextParams, options));
        verifyCodexPureApiResumeResult(String(method || ""), nextParams, result, client.hostId);
        noteCodexManagedSessionStart(String(method || ""), nextParams, result, client.hostId);
      } catch (error) {
        if (requestMethod === "turn/start" && codexQuotaResumeValueHasMarker(error)) {
          const failedTurnParams = quotaResumeTarget?.params;
          const threadId = validThreadScrollSessionKey(failedTurnParams?.threadId || failedTurnParams?.conversationId || "");
          if (threadId) {
            const runtime = codexQuotaResumeRuntime();
            runtime.active.delete(threadId);
            runtime.inFlight.delete(threadId);
            runtime.pending.set(threadId, { at: Date.now() });
            trimCodexQuotaResumeMap(runtime.pending);
            refreshCodexQuotaResumeSoon();
          }
        }
        throw error;
      }
      if (codexRateLimitUnlockRequestMethod(requestMethod)) {
        neutralizeCodexRateLimitPayload(result);
      }
      const threadState = codexThreadModelRequestState(String(method || ""), nextParams, result);
      if (modelContextRefresh !== false && threadState.threadId && threadState.model
          && ["thread/start", "thread/resume", "turn/start"].includes(threadState.requestMethod)) {
        client.__codexPlusThreadModels.set(threadState.threadId, threadState.model);
      }
      if (!codexPlusModelUnlockEnabled() || !["list-models-for-host", "model/list"].includes(requestMethod)) return result;
      if (!codexPlusModelNames().length) await loadCodexModelCatalog();
      return patchAppServerModelResult(requestMethod, result, modelResponseHostId(params, client));
    };
    if (typeof client.prewarmThreadStart === "function") {
      const originalPrewarmThreadStart = (client.__codexPlusServiceTierOriginalPrewarmThreadStart
        || client.prewarmThreadStart).bind(client);
      client.__codexPlusServiceTierOriginalPrewarmThreadStart = originalPrewarmThreadStart;
      client.prewarmThreadStart = async function codexPlusServiceTierPrewarmThreadStart(params, options) {
        return await prewarmCodexManagedSession(client, originalPrewarmThreadStart, params, options);
      };
    }
    client.__codexPlusModelRequestPatch = codexAppServerModelRequestPatchVersion;
    return true;
  }

  /**
   * `codex/exchange` 中间件链：包住一次发往 app-server 的请求。值是 `{ method, params, hostId, send }`，
   * 中间件用 `next({ ...exchange, send: 新的 send })` 在原请求前后做事（内置拓展「侧聊保存」就挂在这里），
   * 链尾调用最终的 send。没有中间件时直接发，不多一层 Promise。
   * 内核分片还没初始化（页面早期、只抽前面分片的测试）时读中间件表会落进暂时性死区，按没有中间件处理。
   */
  async function sendAppServerRequestThroughExchange(client, method, params, send) {
    let wanted = false;
    try {
      wanted = codexPlusExtensionInterceptors.has("codex/exchange");
    } catch {}
    if (!wanted) return send();
    const exchange = await codexPlusRunWaterfall("codex/exchange", { method, params, hostId: client.hostId, send });
    return typeof exchange?.send === "function" ? exchange.send() : send();
  }

  // 账号额度用尽后，上游会把输入框和发送按钮一起锁死（coreRateLimitBlocked /
  // selectedModelRateLimitReached）。判定只依赖 rateLimits 响应里的四个布尔字段，
  // 这里在传输层把它们改写为“未触达”，让用户始终可以换模型继续发送；
  // 真实额度判定仍在服务端，被改写只影响客户端阻断状态。
  const codexRateLimitUnlockMaxTrackedRequests = 64;

  function codexRateLimitUnlockEnabled() {
    return codexPlusSettings().quotaResume === true;
  }

  function codexRateLimitUnlockRequestMethod(value) {
    const method = appServerModelRequestMethod(String(value || ""));
    return method === "account/rateLimits/read"
      || method === "account/usage/read"
      || method === "account/read" ? method : "";
  }

  function neutralizeCodexRateLimitValue(value, depth = 0) {
    if (!value || typeof value !== "object" || depth > 6) return false;
    let changed = false;
    if (Array.isArray(value)) {
      for (const item of value) {
        if (neutralizeCodexRateLimitValue(item, depth + 1)) changed = true;
      }
      return changed;
    }
    for (const key of Object.keys(value)) {
      const item = value[key];
      if (key === "rate_limit_reached_type") {
        if (item != null) {
          value[key] = null;
          changed = true;
        }
        continue;
      }
      if (key === "limit_reached" || key === "blocked" || key === "reached" || key === "overage_limit_reached") {
        if (item !== false) {
          value[key] = false;
          changed = true;
        }
        continue;
      }
      if (key === "allowed") {
        if (item !== true) {
          value[key] = true;
          changed = true;
        }
        continue;
      }
      // 桌面端还会看 credits.has_credits/unlimited 兜底判断是否“额度耗尽”
      // （has_credits=false 且 unlimited=false 时直接判定受限）。这里把
      // has_credits 视为可用，只影响客户端阻断状态，真实额度仍在服务端。
      if (key === "has_credits") {
        if (item !== true) {
          value[key] = true;
          changed = true;
        }
        continue;
      }
      if (item && typeof item === "object" && neutralizeCodexRateLimitValue(item, depth + 1)) {
        changed = true;
      }
    }
    return changed;
  }

  function neutralizeCodexRateLimitPayload(payload) {
    if (!codexRateLimitUnlockEnabled() || !payload || typeof payload !== "object") return payload;
    try {
      if (neutralizeCodexRateLimitValue(payload)) {
        sendCodexPlusDiagnostic("usage_limit_payload_neutralized", {});
      }
    } catch (error) {
      sendCodexPlusDiagnostic("usage_limit_payload_neutralize_failed", {
        errorName: error?.name || "",
        errorMessage: error?.message || String(error),
      });
    }
    return payload;
  }


  function rememberCodexRateLimitRequest(message) {
    if (!codexRateLimitUnlockEnabled()) return;
    if (!message || typeof message !== "object" || message.type !== "fetch") return;
    if (!codexRateLimitUnlockRequestMethod(message.url || message.method || "")) return;
    const requestId = message.requestId != null ? String(message.requestId) : "";
    if (!requestId) return;
    if (!(window.__codexPlusRateLimitRequestIds instanceof Set)) {
      window.__codexPlusRateLimitRequestIds = new Set();
    }
    const requestIds = window.__codexPlusRateLimitRequestIds;
    requestIds.add(requestId);
    while (requestIds.size > codexRateLimitUnlockMaxTrackedRequests) {
      const oldest = requestIds.values().next().value;
      if (oldest === undefined) break;
      requestIds.delete(oldest);
    }
  }

  function patchCodexRateLimitResponseData(data) {
    if (!codexRateLimitUnlockEnabled()) return false;
    if (!data || typeof data !== "object" || data.type !== "fetch-response") return false;
    const requestIds = window.__codexPlusRateLimitRequestIds;
    if (!(requestIds instanceof Set) || requestIds.size === 0) return false;
    const requestId = data.requestId != null ? String(data.requestId) : "";
    if (!requestId || !requestIds.has(requestId)) return false;
    requestIds.delete(requestId);
    if (typeof data.bodyJsonString !== "string" || !data.bodyJsonString.trim()) return false;
    try {
      const parsed = JSON.parse(data.bodyJsonString);
      if (!neutralizeCodexRateLimitValue(parsed)) return false;
      data.bodyJsonString = JSON.stringify(parsed);
      sendCodexPlusDiagnostic("usage_limit_bridge_payload_neutralized", {});
      return true;
    } catch (error) {
      sendCodexPlusDiagnostic("usage_limit_bridge_payload_patch_failed", {
        errorName: error?.name || "",
        errorMessage: error?.message || String(error),
      });
      return false;
    }
  }

  function installCodexRateLimitResponsePatch() {
    if (window.__codexPlusRateLimitResponsePatch === codexRateLimitUnlockVersion) return;
    if (!codexRateLimitUnlockEnabled()) return;
    const bridge = window.electronBridge;
    if (!bridge || typeof bridge.sendMessageFromView !== "function") return;
    if (!bridge.__codexPlusRateLimitOriginalSendMessageFromView) {
      bridge.__codexPlusRateLimitOriginalSendMessageFromView = bridge.sendMessageFromView.bind(bridge);
      bridge.sendMessageFromView = function codexPlusRateLimitPatchedSendMessageFromView(message) {
        let nextMessage = message;
        try {
          rememberCodexRateLimitRequest(message);
        } catch (error) {
          sendCodexPlusDiagnostic("usage_limit_bridge_request_patch_failed", {
            errorName: error?.name || "",
            errorMessage: error?.message || String(error),
          });
        }
        return bridge.__codexPlusRateLimitOriginalSendMessageFromView(nextMessage);
      };
    }
    if (!window.__codexPlusRateLimitResponseListenerInstalled) {
      window.__codexPlusRateLimitResponseListenerInstalled = true;
      window.addEventListener("message", (event) => {
        try {
          patchCodexRateLimitResponseData(event?.data);
        } catch (error) {
          sendCodexPlusDiagnostic("usage_limit_bridge_response_patch_failed", {
            errorName: error?.name || "",
            errorMessage: error?.message || String(error),
          });
        }
      }, true);
    }
    window.__codexPlusRateLimitResponsePatch = codexRateLimitUnlockVersion;
    sendCodexPlusDiagnostic("usage_limit_bridge_patch_installed", {});
  }

  function patchCodexUsageHttpTransport(module) {
    if (!codexRateLimitUnlockEnabled()) return false;
    for (const candidate of Object.values(module || {})) {
      if (typeof candidate !== "function" || typeof candidate.getInstance !== "function") continue;
      let transport;
      try { transport = candidate.getInstance(); } catch { continue; }
      if (!transport || typeof transport.fetch !== "function" || typeof transport.stream !== "function"
          || !(transport.streamControllers instanceof Map)) continue;
      if (transport.__codexPlusUsageHttpPatch === codexRateLimitUnlockVersion) return true;
      let streamSource = "";
      try { streamSource = Function.prototype.toString.call(transport.stream); } catch {}
      if (!streamSource.includes("fetch-stream-event") || !streamSource.includes("streamControllers")) continue;
      const originalFetch = transport.fetch.bind(transport);
      const originalStream = transport.stream.bind(transport);
      transport.fetch = async function codexPlusUsageFetch(url, options) {
        const response = await originalFetch(url, options);
        if (!codexRateLimitUnlockEnabled() || typeof url !== "string"
            || !/^\/wham\/usage(?:\?|$)/.test(url) || !response?.ok) return response;
        try {
          const payload = await response.clone().json();
          if (!payload || typeof payload !== "object" || !neutralizeCodexRateLimitValue(payload)) return response;
          const headers = new Headers(response.headers);
          headers.delete("content-length");
          headers.delete("content-encoding");
          sendCodexPlusDiagnostic("usage_limit_http_payload_neutralized", {});
          return new Response(JSON.stringify(payload), {
            status: response.status,
            statusText: response.statusText,
            headers,
          });
        } catch (error) {
          sendCodexPlusDiagnostic("usage_limit_http_payload_patch_failed", {
            errorName: error?.name || "",
          });
          return response;
        }
      };
      transport.stream = function codexPlusUsageStream(method, url, options) {
        if (!codexRateLimitUnlockEnabled() || method !== "GET" || typeof url !== "string"
            || !/^\/wham\/usage\/stream(?:\?|$)/.test(url)
            || typeof options?.onEvent !== "function") {
          return originalStream(method, url, options);
        }
        return originalStream(method, url, {
          ...options,
          onEvent(event) {
            if (event?.event === "usage.snapshot" && event.data?.usage) {
              try {
                if (neutralizeCodexRateLimitValue(event.data.usage)) {
                  sendCodexPlusDiagnostic("usage_limit_stream_payload_neutralized", {});
                }
              } catch (error) {
                sendCodexPlusDiagnostic("usage_limit_stream_payload_patch_failed", {
                  errorName: error?.name || "",
                });
              }
            }
            return options.onEvent(event);
          },
        });
      };
      transport.__codexPlusUsageHttpPatch = codexRateLimitUnlockVersion;
      sendCodexPlusDiagnostic("usage_limit_http_patch_installed", {});
      return true;
    }
    return false;
  }

  let codexUsageQueryClient = null;

  function findCodexUsageQueryClient() {
    if (codexUsageQueryClient?.getQueryCache && codexUsageQueryClient?.setQueryData) {
      return codexUsageQueryClient;
    }
    const roots = [document.getElementById("root"), document.body, document.documentElement].filter(Boolean);
    const queue = [];
    for (const root of roots) {
      for (const key of Object.keys(root)) {
        if (key.startsWith("__reactContainer$") || key.startsWith("__reactFiber$")) queue.push(root[key]?.current || root[key]);
      }
    }
    const seen = new Set();
    for (let index = 0; index < queue.length && index < 30000; index += 1) {
      const fiber = queue[index];
      if (!fiber || seen.has(fiber)) continue;
      seen.add(fiber);
      const client = fiber.memoizedProps?.client;
      if (client && typeof client.getQueryCache === "function" && typeof client.setQueryData === "function") {
        codexUsageQueryClient = client;
        return client;
      }
      if (fiber.child) queue.push(fiber.child);
      if (fiber.sibling) queue.push(fiber.sibling);
    }
    return null;
  }

  // model/list 可在注入前进入 React Query 缓存；只拦后续 RPC 会留下隐藏的来源模型。
  // 只处理明确属于本机的模型查询，不修改远程主机、会话设置或账号身份。
  function refreshCodexModelQueryCache(client = findCodexUsageQueryClient()) {
    if (!codexPlusModelUnlockEnabled() || codexModelCatalog.status !== "ok"
        || !codexPlusModelNames().length || !client?.getQueryCache || !client?.setQueryData) return 0;
    let changed = 0;
    for (const query of client.getQueryCache().findAll({ queryKey: ["models", "list", "local"] })) {
      const key = query.queryKey;
      if (!Array.isArray(key) || key[0] !== "models" || key[1] !== "list" || key[2] !== "local"
          || !query.state?.data || typeof query.state.data !== "object") continue;
      try {
        const data = structuredClone(query.state.data);
        if (!patchModelContainer(data, "local")) continue;
        client.setQueryData(key, data);
        changed += 1;
      } catch (error) {
        sendCodexPlusDiagnostic("model_query_cache_patch_failed", { errorName: error?.name || "" });
      }
    }
    if (changed) sendCodexPlusDiagnostic("model_query_cache_patched", { queries: changed });
    return changed;
  }

  function installCodexModelQueryCachePatch() {
    const client = findCodexUsageQueryClient();
    if (!client?.getQueryCache) return;
    const previous = window.__codexPlusModelQueryCacheRuntime;
    if (previous?.client === client && previous?.patch === refreshCodexModelQueryCache) {
      previous.refresh();
      return;
    }
    previous?.dispose?.();
    let updating = false;
    const refresh = () => {
      if (updating) return;
      updating = true;
      try { refreshCodexModelQueryCache(client); } finally { updating = false; }
    };
    const unsubscribe = client.getQueryCache().subscribe((event) => {
      const key = event?.query?.queryKey;
      if (event?.type === "updated" && key?.[0] === "models" && key?.[1] === "list"
          && key?.[2] === "local") refresh();
    });
    window.__codexPlusModelQueryCacheRuntime = {
      client, patch: refreshCodexModelQueryCache, refresh, dispose: unsubscribe,
    };
    refresh();
  }

  function refreshCodexUsageLimitQueryCache() {
    if (!codexRateLimitUnlockEnabled()) return;
    const client = findCodexUsageQueryClient();
    if (!client) return;
    let changed = 0;
    for (const query of client.getQueryCache().findAll({ queryKey: ["rate-limit-status"] })) {
      if (!Array.isArray(query.queryKey) || query.queryKey[0] !== "rate-limit-status"
          || !query.state?.data || typeof query.state.data !== "object") continue;
      try {
        const data = structuredClone(query.state.data);
        if (!neutralizeCodexRateLimitValue(data)) continue;
        client.setQueryData(query.queryKey, data);
        changed += 1;
      } catch (error) {
        sendCodexPlusDiagnostic("usage_limit_cache_patch_failed", {
          errorName: error?.name || "",
        });
      }
    }
    if (changed) sendCodexPlusDiagnostic("usage_limit_cache_neutralized", { queries: changed });
  }

  // The desktop app sends conversation requests through app-shared's message
  // broker. Its preload bridge is frozen, and the vscode://codex/ fetch transport
  // below is used by other requests, so the broker is the routing boundary.
  async function prepareCodexGatewaySourceRequest(send, method, params, hostId = "local") {
    if (hostId !== "local" || !codexGatewaySessionRoutingEnabled()
        || !["thread/fork", "thread/resume"].includes(method) || !params?.threadId
        || (method === "thread/fork" && params.experimentalPredictionMode === true)) return params;
    const threadId = String(params.threadId);
    const probe = await send("thread/read", { threadId, includeTurns: false });
    const thread = probe?.thread;
    const source = String(thread?.modelProvider || "");
    if (thread?.id !== threadId || !source.startsWith("codex_plus_official_")) return params;
    if (thread.ephemeral === true || JSON.stringify(thread.source || "").toLowerCase().includes("subagent")) {
      throw new Error("临时会话和子代理不能单独迁移来源；请从根对话继续。");
    }
    let requested = String(params.model || params.modelId || params.model_id || thread.model || "").trim();
    // 新版 thread/read 不返回 model；原生打开历史聊天也可不携带模型覆盖。
    // 只读同一 thread 的持久模型，不能以当前默认模型替换原账号来源。
    if (!requested) {
      const saved = await postJson("/thread-resume-model-metadata", { thread_id: threadId, host_id: "local" });
      if (saved?.status !== "ok" || saved.threadId !== threadId || typeof saved.model !== "string" || !saved.model.trim()) {
        throw new Error("无法读取此旧会话保存的模型，未更改来源或发送消息。");
      }
      requested = saved.model.trim();
    }
    const legacyKey = requested.startsWith("codex_plus_official_") && requested.includes(":")
      ? requested : `${source}:${requested}`;
    // 显式的当前网关模型可直接使用；旧裸模型必须精确映射，不能用默认模型代替账号。
    const mapped = codexModelCatalog?.legacyProviderRoutes?.[legacyKey];
    const model = mapped || (params.model && codexModelCatalog?.models?.includes(requested) ? requested : "");
    const target = model && codexRemoteSessionTargetProvider(model);
    if (!target) throw new Error("此旧来源尚无唯一的网关模型；请先在网关接入原账号或选择明确的来源模型。");
    const next = { ...params, model, modelProvider: target, config: { ...params.config, model_provider: target } };
    delete next.model_provider;
    if (method === "thread/fork") return next;
    if (!["idle", "notLoaded"].includes(thread.status?.type)) {
      throw new Error("请等当前回合和审批结束，再恢复旧来源会话；本次消息未发送。");
    }
    if (thread.status.type !== "notLoaded") {
      await send("thread/unsubscribe", { threadId });
      const unloaded = await send("thread/read", { threadId, includeTurns: false });
      if (unloaded?.thread?.id !== threadId || unloaded.thread.modelProvider !== source
          || unloaded.thread.status?.type !== "notLoaded") {
        throw new Error("旧会话仍被原生运行时占用；请保存工作并重启 Codex 后打开原会话，来源迁移会自动重试。");
      }
    }
    // 这是写操作，必须等待完成，不能使用 preparation read 的超时竞速。
    const migrated = await postJson("/session/gateway-provider/migrate", {
      threadId, sourceProvider: source, targetProvider: target, model, nativeUnloaded: true,
    });
    if (migrated?.status !== "ok" || migrated.threadId !== threadId || migrated.modelProvider !== target) {
      throw new Error(migrated?.error || migrated?.message || "旧会话来源迁移未完成，本次消息未发送。");
    }
    return next;
  }

  async function switchCodexOfficialThreadProvider(send, method, params, targetProvider) {
    const threadId = String(params.threadId || params.thread_id || "").trim();
    if (method === "turn/settings/update") {
      throw new Error("当前回合不能更换账号来源；请等回合结束后在此对话中切换，不需要创建新对话。");
    }
    // 已加载的 resume 不会改 provider；先取得当前运行配置，避免重载丢失权限或推理档位。
    let previous;
    try {
      previous = await send("thread/resume", { threadId, excludeTurns: true });
    } catch (error) {
      if (!codexGatewaySessionRoutingEnabled()
          || !/This chat uses .+current provider is/i.test(String(error?.message || error))) throw error;
      const migrated = await prepareCodexGatewaySourceRequest(send, "thread/resume", { ...params, threadId });
      if (migrated === params || migrated.modelProvider !== targetProvider) throw error;
      previous = await send("thread/resume", { ...migrated, excludeTurns: true });
      if (previous?.thread?.id !== threadId) throw new Error("恢复结果的会话 ID 不一致，本次消息未发送。");
    }
    const previousProvider = String(previous?.modelProvider || previous?.thread?.modelProvider || "").trim();
    if (previousProvider === targetProvider) return targetProvider;
    if (previous?.thread?.status?.type === "systemError") {
      throw new Error("当前 Codex 对话实例处于错误状态，原生接口暂不允许重载来源；请重启 Codex 后打开原对话重试，无需创建副本。本次消息未发送。");
    }
    if (previous?.thread?.status?.type !== "idle") {
      throw new Error("请等当前回合和审批结束后再切换来源；原对话与正在执行的任务保持原状。");
    }
    const resume = { threadId, model: String(params.model || previous.model || "").trim(),
      modelProvider: targetProvider, excludeTurns: true, config: { model_provider: targetProvider } };
    for (const key of ["cwd", "runtimeWorkspaceRoots", "approvalPolicy", "approvalsReviewer", "serviceTier"]) {
      if (Object.prototype.hasOwnProperty.call(params, key)) resume[key] = params[key];
      else if (Object.prototype.hasOwnProperty.call(previous, key)) resume[key] = previous[key];
    }
    const permissions = params.permissions || (!(params.sandbox || params.sandboxPolicy) && previous.activePermissionProfile?.id);
    if (permissions) resume.permissions = permissions;
    else {
      const policy = params.sandboxPolicy || previous.sandbox;
      if (typeof params.sandbox === "string") resume.sandbox = params.sandbox;
      else if (policy) {
        const modes = { readOnly: "read-only", workspaceWrite: "workspace-write", dangerFullAccess: "danger-full-access" };
        if (!modes[policy.type] || (policy.readOnlyAccess && policy.readOnlyAccess.type !== "fullAccess")
            || (policy.type === "readOnly" && policy.networkAccess === true)) {
          throw new Error("当前自定义权限无法完整保留，已停止来源切换；请先选择具名权限配置。");
        }
        resume.sandbox = modes[policy.type];
        if (policy.type === "workspaceWrite") {
          resume.config.sandbox_workspace_write = {};
          for (const [from, to] of [["writableRoots", "writable_roots"], ["networkAccess", "network_access"],
            ["excludeTmpdirEnvVar", "exclude_tmpdir_env_var"], ["excludeSlashTmp", "exclude_slash_tmp"]]) {
            if (policy[from] !== undefined) resume.config.sandbox_workspace_write[to] = policy[from];
          }
        }
      }
    }
    const reasoning = normalizeCodexModelReasoningParams("thread/settings/update", {
      threadId, model: resume.model, modelProvider: targetProvider,
      effort: params.effort || params.reasoningEffort || previous.reasoningEffort || "",
    });
    const effort = reasoning.effort || reasoning.reasoningEffort;
    if (effort) resume.config.model_reasoning_effort = effort;
    await send("thread/unsubscribe", { threadId });
    // unsubscribe 只退订；下一次 resume 才能在无人订阅的 idle 实例上应用来源覆写。
    const opened = await send("thread/resume", resume);
    const actualProvider = String(opened?.modelProvider || opened?.thread?.modelProvider || "").trim();
    const actualThreadId = String(opened?.thread?.id || opened?.threadId || "").trim();
    if (actualProvider !== targetProvider || actualThreadId !== threadId) {
      throw new Error("来源尚未在原对话生效，本次消息未发送；请关闭其他正在查看此对话的窗口后重试，不会创建副本。");
    }
    const settings = { threadId };
    const collaboration = params.collaborationMode || previous.collaborationMode;
    if (collaboration) settings.collaborationMode = { ...collaboration, settings: {
      ...collaboration.settings, model: resume.model, ...(effort ? { reasoning_effort: effort } : {}),
    } };
    if (Array.isArray(previous.disabledPluginIds)) settings.disabledPluginIds = previous.disabledPluginIds;
    if (Object.keys(settings).length > 1) await send("thread/settings/update", settings);
    return actualProvider;
  }

  function installCodexNativeThreadMetadataReader(controlRequest) {
    if (typeof window.__codexPlusReadNativeThreadMetadata === "function") return;
    // 新版冻结的 preload 仍能收发原生消息，不依赖旧 ESM broker 的发现结果。
    if (typeof controlRequest !== "function") {
      const bridge = window.electronBridge;
      if (typeof bridge?.sendMessageFromView !== "function") return;
      controlRequest = (hostId, method, params, timeoutMs) => new Promise((resolve, reject) => {
        const id = `${method}:codex-plus-metadata-${crypto.randomUUID()}`;
        const finish = (error, result) => {
          clearTimeout(timer);
          window.removeEventListener("message", listener);
          if (error) reject(error);
          else resolve(result);
        };
        const listener = (event) => {
          if (event.source != null && event.source !== window) return;
          const data = event.data;
          if (data?.type !== "mcp-response" || data.hostId !== hostId || data.message?.id !== id) return;
          finish(data.message.error ? new Error(data.message.error.message || "native thread read failed") : null,
            data.message.result);
        };
        const timer = setTimeout(() => finish(new Error("native thread metadata read timed out")), timeoutMs);
        window.addEventListener("message", listener);
        try {
          Promise.resolve(bridge.sendMessageFromView({ type: "mcp-request", hostId,
            request: { id, method, params }, coalesce: false })).catch((error) => finish(error));
        } catch (error) {
          finish(error);
        }
      });
    }
    const read = async (threadId) => {
      if (typeof threadId !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(threadId)) {
        throw new Error("native thread identity is invalid");
      }
      const result = await controlRequest("local", "thread/read", { threadId, includeTurns: false }, 4000);
      const thread = result?.thread;
      if (!thread || thread.id !== threadId || typeof thread.ephemeral !== "boolean"
          || typeof thread.modelProvider !== "string") throw new Error("native thread identity unavailable");
      return {
        id: thread.id,
        sessionId: thread.sessionId ?? null,
        ephemeral: thread.ephemeral,
        modelProvider: thread.modelProvider,
        forkedFromId: thread.forkedFromId ?? null,
        parentThreadId: thread.parentThreadId ?? null,
        isSubagent: JSON.stringify(thread.source ?? "").toLowerCase().includes("subagent"),
        statusType: thread.status?.type ?? "unknown",
      };
    };
    Object.defineProperty(window, "__codexPlusReadNativeThreadMetadata", {
      value: read, writable: false, configurable: false,
    });
  }

  function patchCodexMcpRequestTransport(module) {
    for (const candidate of Object.values(module || {})) {
      let broker = candidate;
      if (typeof candidate === "function" && typeof candidate.getInstance === "function") {
        try { broker = candidate.getInstance(); } catch { continue; }
      }
      if (!broker || typeof broker.dispatchMessage !== "function"
          || typeof broker.subscribe !== "function"
          || !(broker.handlers instanceof Map)
          || !(broker.bufferedMessages instanceof Map)
          || typeof broker.deliverOrBufferMessage !== "function") continue;
      if (broker.__codexPlusMcpRequestPatch === codexAppServerModelRequestPatchVersion) return true;

      const originalDispatchMessage = broker.dispatchMessage.bind(broker);
      const threadProviders = new Map();
      const pendingThreadOpens = new Map();
      const threadTransitions = new Map();
      const threadKey = (hostId, threadId) => `${hostId}:${threadId}`;
      const responsePayload = (value) => value?.result || value?.body?.result || value?.data?.result || value;
      broker.subscribe("mcp-response", (event) => {
        const requestId = String(event?.message?.id || "");
        const pending = pendingThreadOpens.get(requestId);
        if (!pending) return;
        pendingThreadOpens.delete(requestId);
        if (event?.message?.error) return;
        const result = responsePayload(event?.message?.result);
        const threadId = String(result?.thread?.id || result?.threadId || pending.threadId || "").trim();
        const provider = String(result?.modelProvider || result?.thread?.modelProvider || "").trim();
        if (threadId && provider) threadProviders.set(threadKey(pending.hostId, threadId), provider);
        if (threadId && pending.provider && provider && pending.provider !== provider) {
          sendCodexPlusDiagnostic("official_thread_provider_mismatch", {
            requested: pending.provider, actual: provider,
          });
        }
        const model = String(result?.model || result?.thread?.model || pending.model || "").trim();
        if (threadId && model) rememberCodexOfficialThreadModel(threadId, model, provider || pending.provider);
      });

      function controlRequest(hostId, method, params, requestTimeoutMs) {
        return new Promise((resolve, reject) => {
          const id = `${method}:${crypto.randomUUID()}`;
          const timeoutMs = requestTimeoutMs ?? (["thread/fork", "thread/resume"].includes(method) ? 120000 : 20000);
          let unsubscribe = null;
          const timer = setTimeout(() => {
            if (typeof unsubscribe === "function") unsubscribe();
            reject(Object.assign(new Error(`聊天来源请求超时：${method}；本次消息尚未发送，请重新打开聊天后重试`), {
              code: "provider_control_timeout", controlMethod: method,
            }));
          }, timeoutMs);
          unsubscribe = broker.subscribe("mcp-response", (event) => {
            if (String(event?.message?.id || "") !== id || event?.hostId !== hostId) return;
            clearTimeout(timer);
            if (typeof unsubscribe === "function") unsubscribe();
            if (event.message.error) {
              reject(new Error(String(event.message.error.message || `${method} 失败`)));
            } else {
              resolve(responsePayload(event.message.result));
            }
          });
          try {
            originalDispatchMessage("mcp-request", {
              hostId,
              request: { id, method, params },
              priority: "critical",
              source: "codex-plus-provider-switch",
              timeoutMs,
            });
          } catch (error) {
            clearTimeout(timer);
            if (typeof unsubscribe === "function") unsubscribe();
            reject(error);
          }
        });
      }

      installCodexNativeThreadMetadataReader(controlRequest);

      function rejectOriginalRequest(detail, error) {
        const requestId = detail?.request?.id;
        if (!requestId) return;
        sendCodexPlusDiagnostic("official_mcp_route_failed", {
          method: String(detail.request.method || ""),
          errorName: error?.name || "Error",
          reasonCode: error?.code === "provider_control_timeout" ? "provider_control_timeout" : "route_rejected",
          ...(error?.controlMethod ? { controlMethod: error.controlMethod } : {}),
        });
        broker.deliverOrBufferMessage({
          type: "mcp-response",
          hostId: detail.hostId,
          message: {
            id: requestId,
            error: { code: -32000, message: String(error?.message || error || "账号切换失败") },
          },
        });
      }

      async function routeRequest(type, detail) {
        const request = detail.request;
        const method = String(request.method || "");
        let params = request.params;
        const requestedModel = String(params.model || params.modelId || params.model_id || "").trim();
        const looksExtended = requestedModel.includes(":") || /\(.+\)/.test(requestedModel);
        if (looksExtended || codexRemoteSessionProviderPatchEnabled()) {
          const settingsLoaded = await loadBackendSettingsState();
          if (!settingsLoaded && looksExtended) {
            throw new Error("扩展模型来源配置不可用，请保持 Codex++ 运行并重试");
          }
          if (!Array.isArray(codexModelCatalog?.routeDescriptors)
              || (looksExtended && !codexOfficialExperienceRouteDescriptor(requestedModel))) await loadCodexModelCatalog(true);
          if ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) === true
              && looksExtended && !codexOfficialExperienceRouteDescriptor(requestedModel)) {
            throw new Error(`扩展模型 ${requestedModel} 没有可用的来源路由`);
          }
        }
        const hostId = String(detail.hostId || "");
        params = await prepareCodexGatewaySourceRequest(
          (name, body) => controlRequest(hostId, name, body), method, params, hostId,
        );
        let nextParams = normalizeCodexModelReasoningParams(method, params);
        const threadId = String(nextParams.threadId || nextParams.thread_id || "").trim();
        let targetProvider = String(nextParams.modelProvider || "").trim();
        if (["turn/start", "thread/settings/update", "turn/settings/update"].includes(method) && threadId
            && ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) === true
              || (codexPlusBackendSettings?.relayProfilesEnabled === true && codexPlusBackendSettings?.activeRelayId === "codex-plus-gateway"))) {
          const key = threadKey(hostId, threadId);
          let currentProvider = threadProviders.get(key) || "";
          let currentModel = "";
          if (!currentProvider || !targetProvider) {
            try {
              // 仅查询归属，不恢复整段历史、不初始化 MCP、不更改登录态。
              const probe = await controlRequest(hostId, "thread/read", { threadId, includeTurns: false });
              currentProvider = String(probe?.modelProvider || probe?.thread?.modelProvider || "").trim();
              currentModel = String(probe?.model || probe?.thread?.model || "").trim();
              if (currentProvider) threadProviders.set(key, currentProvider);
            } catch (error) {
              if (!String(error?.message || error).includes("no rollout found")) throw error;
            }
          }
          if (!targetProvider && currentModel) {
            nextParams = normalizeCodexModelReasoningParams(method, { ...nextParams, model: currentModel });
            targetProvider = String(nextParams.modelProvider || "").trim();
          }
          if (!currentProvider && targetProvider && targetProvider !== "openai") {
            throw new Error("无法确认会话当前账号，已停止发送；请重新打开该任务后重试");
          }
          if (currentProvider && targetProvider && currentProvider !== targetProvider) {
            const resumedProvider = await switchCodexOfficialThreadProvider(
              (requestMethod, requestParams) => controlRequest(hostId, requestMethod, requestParams),
              method, nextParams, targetProvider,
            );
            threadProviders.set(key, resumedProvider);
            notifyCodexOfficialThreadProviderChange(currentProvider, resumedProvider);
            sendCodexPlusDiagnostic("official_thread_provider_switched", {
              from: currentProvider, to: resumedProvider,
            });
          }
        }
        if (method === "thread/fork" || method === "thread/start" || method === "thread/resume") {
          pendingThreadOpens.set(String(request.id), {
            hostId,
            threadId,
            provider: targetProvider,
            model: String(nextParams.model || "").trim(),
          });
        }
        if ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) === true && targetProvider) {
          sendCodexPlusDiagnostic("official_mcp_route_selected", { method, providerId: targetProvider, model: String(nextParams.model || "") });
        }
        originalDispatchMessage(type, {
          ...detail,
          request: { ...request, params: nextParams },
        });
      }

      const patchedDispatchMessage = function codexPlusMcpRoutedDispatchMessage(type, detail) {
        const request = detail?.request;
        const method = String(request?.method || "");
        if ((type !== "mcp-request" && type !== "thread-prewarm-start")
            || !["thread/fork", "thread/start", "thread/resume", "thread/settings/update", "turn/settings/update", "turn/start"].includes(method)
            || !request?.params || typeof request.params !== "object" || Array.isArray(request.params)) {
          return originalDispatchMessage(type, detail);
        }
        const hostId = String(detail.hostId || "");
        const threadId = String(request.params.threadId || request.params.thread_id || "").trim();
        const key = threadId ? threadKey(hostId, threadId) : "";
        const previous = key ? threadTransitions.get(key) : null;
        const work = previous ? previous.catch(() => {}).then(() => routeRequest(type, detail)) : routeRequest(type, detail);
        if (key) threadTransitions.set(key, work);
        void work.catch((error) => rejectOriginalRequest(detail, error)).finally(() => {
          if (key && threadTransitions.get(key) === work) threadTransitions.delete(key);
        });
      };
      broker.dispatchMessage = patchedDispatchMessage;
      if (broker.dispatchMessage !== patchedDispatchMessage) return false;
      broker.__codexPlusMcpRequestPatch = codexAppServerModelRequestPatchVersion;
      sendCodexPlusDiagnostic("model_mcp_request_patch_installed", {});
      return true;
    }
    return false;
  }

  // Recent Codex builds send some app-server requests through a shared vscode://codex/
  // transport. The preload bridge is frozen and the old sendRequest client is
  // no longer exported, so patch the transport before it forwards the request.
  function patchCodexVscodeRequestTransport(module) {
    const threadProviders = new Map();
    const threadTransitions = new Map();
    const responsePayload = (value) => value?.result || value?.body?.result || value?.data?.result || value;
    for (const candidate of Object.values(module || {})) {
      if (typeof candidate !== "function" || typeof candidate.getInstance !== "function") continue;
      let transport;
      try { transport = candidate.getInstance(); } catch { continue; }
      if (!transport || typeof transport.sendRequest !== "function"
          || !(transport.pendingRequests instanceof Map)) continue;
      let source = "";
      try { source = Function.prototype.toString.call(transport.sendRequest); } catch {}
      if (!source.includes("vscode://codex/") || !source.includes("pendingRequests")) continue;
      if (transport.__codexPlusModelRequestPatch === codexAppServerModelRequestPatchVersion) return true;
      const originalSendRequest = transport.sendRequest.bind(transport);
      const routeRequest = async function codexPlusVscodeModelRequest(method, url, options) {
        const requestMethod = appServerModelRequestMethod(url);
        if (codexRateLimitUnlockRequestMethod(url)) {
          const rateLimitResult = await originalSendRequest(method, url, options);
          return neutralizeCodexRateLimitPayload(rateLimitResult);
        }
        if (method !== "POST" || !codexRemoteSessionProviderRequestMethod(requestMethod)
            || !options || typeof options.body !== "string") {
          return originalSendRequest(method, url, options);
        }
        let params;
        try { params = JSON.parse(options.body); } catch { return originalSendRequest(method, url, options); }
        const requestedModel = String(params?.model || params?.modelId || params?.model_id || "").trim();
        const looksExtended = requestedModel.includes(":") || /\(.+\)/.test(requestedModel);
        if (looksExtended || codexRemoteSessionProviderPatchEnabled()) {
          const settingsLoaded = await loadBackendSettingsState();
          if (!settingsLoaded && looksExtended) {
            throw new Error("扩展模型来源配置不可用，请保持 Codex++ 运行并重试");
          }
          if (!Array.isArray(codexModelCatalog?.routeDescriptors)
              || (looksExtended && !codexOfficialExperienceRouteDescriptor(requestedModel))) await loadCodexModelCatalog(true);
          if ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) === true
              && looksExtended
              && !codexOfficialExperienceRouteDescriptor(requestedModel)) {
            throw new Error(`扩展模型 ${requestedModel} 没有可用的来源路由`);
          }
        }
        params = await prepareCodexGatewaySourceRequest(
          async (name, body) => responsePayload(await originalSendRequest("POST", `vscode://codex/${name}`, {
            ...options, body: JSON.stringify(body),
          })), requestMethod, params,
        );
        const nextParams = normalizeCodexModelReasoningParams(requestMethod, params);
        const threadId = String(nextParams?.threadId || nextParams?.thread_id || "").trim();
        const targetProvider = String(nextParams?.modelProvider || "").trim();
        if (["turn/start", "thread/settings/update", "turn/settings/update"].includes(requestMethod) && threadId && targetProvider
            && ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) === true
              || (codexPlusBackendSettings?.relayProfilesEnabled === true && codexPlusBackendSettings?.activeRelayId === "codex-plus-gateway"))) {
          let currentProvider = threadProviders.get(threadId) || "";
          if (!currentProvider) {
            // 查询归属不能恢复历史或初始化新的会话运行时。
            try {
              const probe = responsePayload(await originalSendRequest("POST", "vscode://codex/thread/read", {
                ...options, body: JSON.stringify({ threadId, includeTurns: false }),
              }));
              currentProvider = String(probe?.modelProvider || probe?.thread?.modelProvider || "").trim();
              if (currentProvider) threadProviders.set(threadId, currentProvider);
            } catch (error) {
              // A new thread has no rollout until its first turn. Its provider
              // comes from thread/start and must already match the chosen model.
              if (!String(error?.message || error).includes("no rollout found")) throw error;
            }
          }
          if (!currentProvider && targetProvider !== "openai") {
            throw new Error("无法确认会话当前账号，已停止发送；请重新打开该任务后重试");
          }
          if (currentProvider && currentProvider !== targetProvider) {
            const resumedProvider = await switchCodexOfficialThreadProvider(
              async (method, params) => responsePayload(await originalSendRequest("POST", `vscode://codex/${method}`, {
                ...options, body: JSON.stringify(params),
              })), requestMethod, nextParams, targetProvider,
            );
            threadProviders.set(threadId, resumedProvider);
            notifyCodexOfficialThreadProviderChange(currentProvider, resumedProvider);
            sendCodexPlusDiagnostic("official_thread_provider_switched", {
              from: currentProvider, to: resumedProvider,
            });
          }
        }
        const result = await originalSendRequest(method, url,
          nextParams === params ? options : { ...options, body: JSON.stringify(nextParams) });
        if (requestMethod === "thread/start" || requestMethod === "thread/resume") {
          const payload = responsePayload(result);
          const startedThreadId = String(payload?.thread?.id || payload?.threadId || threadId).trim();
          const actualProvider = String(payload?.modelProvider || payload?.thread?.modelProvider || "").trim();
          if (startedThreadId && actualProvider) threadProviders.set(startedThreadId, actualProvider);
          const resumedModel = String(payload?.model || payload?.thread?.model || "").trim();
          if (startedThreadId && resumedModel) rememberCodexOfficialThreadModel(startedThreadId, resumedModel, actualProvider || targetProvider);
        }
        return result;
      };
      transport.sendRequest = function codexPlusVscodeRoutedRequest(method, url, options) {
        let threadId = "";
        if (method === "POST" && codexRemoteSessionProviderRequestMethod(appServerModelRequestMethod(url))
            && typeof options?.body === "string") {
          try {
            const params = JSON.parse(options.body);
            threadId = String(params?.threadId || params?.thread_id || "").trim();
          } catch {}
        }
        if (!threadId) return routeRequest(method, url, options);
        const previous = threadTransitions.get(threadId);
        const work = previous ? previous.catch(() => {}).then(() => routeRequest(method, url, options)) : routeRequest(method, url, options);
        threadTransitions.set(threadId, work);
        void work.catch(() => {}).finally(() => {
          if (threadTransitions.get(threadId) === work) threadTransitions.delete(threadId);
        });
        return work;
      };
      transport.__codexPlusModelRequestPatch = codexAppServerModelRequestPatchVersion;
      sendCodexPlusDiagnostic("model_vscode_request_patch_installed", {});
      return true;
    }
    return false;
  }

  // issue #2177：Codex 26.908 把 AppServerRequestClient 类藏进模块闭包且不再导出。
  // 这里先用脚本文本定位 sendRequest，launcher 侧通过 CDP 断点捕获类构造器，
  // 再在 prototype 上套用与实例版一致的 provider / quota / service-tier 补丁。
  function locateCodexAppServerClientBreakpoint(text) {
    if (typeof text !== "string" || !text) return null;
    const markerIdx = text.indexOf(codexAppServerClientCaptureMarker);
    if (markerIdx < 0) return null;
    const anchorIdx = text.lastIndexOf(codexAppServerClientCaptureAnchor, markerIdx);
    if (anchorIdx < 0 || markerIdx - anchorIdx > 220) return null;
    const braceIdx = text.indexOf("{", anchorIdx);
    if (braceIdx < 0) return null;
    let lineNumber = 0;
    let lastNewline = -1;
    for (let i = 0; i < braceIdx; i += 1) {
      if (text.charCodeAt(i) === 10) {
        lineNumber += 1;
        lastNewline = i;
      }
    }
    return { lineNumber, columnNumber: braceIdx - lastNewline - 1 };
  }

  // issue #2399：抓取目标不能再按 asset 文件名前缀写死。Codex 26.930 把
  // AppServerRequestClient 从 `app-initial-*.js` 搬到了 `app-shared-*.js`，
  // 按前缀找资产必然 anchor_missing，模型白名单解锁在整条 app-server 路径上失效。
  // 加一项前缀只治当前这一版，下次改名又会失灵，所以改成**按内容找类定义**：
  // 遍历已加载的 app asset，谁包含 marker 文本谁就是目标，两代产物都能覆盖。
  // 排序只做「可能的更靠前」的启发式，不影响正确性；命中全靠文本匹配。
  const codexAppServerClientBundleHints = ["app-shared-", "app-initial-", "app-main-", "chatg"];
  const codexAppServerClientAssetFetchLimit = 24;

  // 已经拉过全文、确认不含类定义的资产。重试时只看新出现的候选，
  // 不会因为多试几轮就把同一批大文件反复 fetch（#1960 / #2330 的教训）。
  const codexAppServerClientCheckedUrls = new Set();

  function codexAppServerClientAssetCandidateUrls() {
    const urls = codexAppAssetCandidateUrls().filter((url) => !codexAppServerClientCheckedUrls.has(url));
    const rank = (url) => {
      const name = (url.split("/").pop() || "").toLowerCase();
      const hint = codexAppServerClientBundleHints.findIndex((part) => name.includes(part));
      return hint < 0 ? codexAppServerClientBundleHints.length : hint;
    };
    // 按「像主 bundle」在前、体积（URL 长度做代理）大的在前排序，再截断。
    // 全量 fetch 所有 app asset 会重蹈 #1960 的覆辙，所以限制尝试数量。
    return urls
      .slice()
      .sort((left, right) => rank(left) - rank(right) || right.length - left.length)
      .slice(0, codexAppServerClientAssetFetchLimit);
  }

  // 返回 { url, location }；找不到返回 null。diagnostics 由调用方按命中/未命中归类。
  async function locateCodexAppServerClientInAssets() {
    const urls = codexAppServerClientAssetCandidateUrls();
    if (urls.length === 0) return { urls, hit: null };
    // 先走廉价的 asset loader（有 30s 失败冷却与重试上限），脚本里已经内联了
    // asset 名时能直接命中，省掉整轮 fetch；不可用就退回到全量文本匹配。
    const hinted = resolveCodexAppServerClientHintedUrl();
    const ordered = hinted && !codexAppServerClientCheckedUrls.has(hinted)
      ? [hinted, ...urls.filter((url) => url !== hinted)]
      : urls;
    for (const url of ordered) {
      try {
        const text = await fetch(url).then((response) => response.ok ? response.text() : "");
        if (!text) continue;
        const location = locateCodexAppServerClientBreakpoint(text);
        if (location) return { urls, hit: { url, location } };
        codexAppServerClientCheckedUrls.add(url);
      } catch {
        // 单个 asset 拉取失败不影响其余候选，继续下一个；没记成已检查，下一轮还会再试。
      }
    }
    return { urls, hit: null };
  }

  // 兼容旧路径：asset 名确实内联在入口脚本里时，用 loader 的缓存直接拿到 URL，
  // 不必为了它把所有 asset 拉一遍。找不到就返回空串，交给内容匹配兜底。
  function resolveCodexAppServerClientHintedUrl() {
    for (const prefix of codexAppServerClientBundleHints) {
      const url = codexAppAssetUrl(prefix);
      if (url) return url;
    }
    return "";
  }

  // 定位常常跑在类所在的资产「可见」之前：Codex 26.10xx 由 app-main 静态 import app-initial，
  // 开了 Fast 时 import map 又把它指向 Blob，资源记录里没有原 URL（缓冲区 250 条也早就满了），
  // 要等 Fast 模块执行完置上 __codexPlusNativeFastCompiled 才能拿到。之前首轮只看到 5 个候选、
  // 报 anchor_missing 后本页再也不试，launcher 等不到坐标就放弃捕获，依赖原型补丁的功能全部失效。
  // 所以未命中时按退避重试；总时长控制在 launcher 90s 的坐标等待窗口之内。
  const codexAppServerClientCaptureRetryDelaysMs = [1000, 2000, 4000, 8000, 15000, 15000, 15000];
  let codexAppServerClientCaptureStarted = false;
  let codexAppServerClientCaptureAttempts = 0;
  async function installCodexAppServerClientCapture() {
    if (codexAppServerClientCaptureStarted || window.__codexPlusAppServerClientCapture) return;
    codexAppServerClientCaptureStarted = true;
    codexAppServerClientCaptureAttempts += 1;
    try {
      if (typeof fetch !== "function") return;
      const { urls, hit } = await locateCodexAppServerClientInAssets();
      if (!hit) {
        const retryDelay = codexAppServerClientCaptureRetryDelaysMs[codexAppServerClientCaptureAttempts - 1];
        if (retryDelay !== undefined) {
          window.setTimeout(() => {
            codexAppServerClientCaptureStarted = false;
            void installCodexAppServerClientCapture().catch(() => {});
          }, retryDelay);
          return;
        }
        // 区分「一个候选资产都没有」和「有资产但没有类定义」：
        // 前者是页面还没加载完，后者才是 Codex 真的改了产物形状。
        sendCodexPlusDiagnostic("app_server_client_capture_locate_failed", {
          reason: urls.length === 0 && codexAppServerClientCheckedUrls.size === 0 ? "asset_url_missing" : "anchor_missing",
          candidateCount: urls.length,
          checkedCount: codexAppServerClientCheckedUrls.size,
          attempts: codexAppServerClientCaptureAttempts,
        });
        return;
      }
      const urlRegex = hit.url.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      window.__codexPlusAppServerClientCapture = { urlRegex, ...hit.location };
      sendCodexPlusDiagnostic("app_server_client_capture_located", {
        lineNumber: hit.location.lineNumber,
        columnNumber: hit.location.columnNumber,
        asset: (hit.url.split("/").pop() || "").split("?")[0],
        attempts: codexAppServerClientCaptureAttempts,
      });
    } catch (error) {
      codexAppServerClientCaptureStarted = false;
      sendCodexPlusDiagnostic("app_server_client_capture_locate_failed", {
        errorName: error?.name || "",
        errorMessage: error?.message || String(error),
      });
    }
  }

  function installCodexAppServerClientPrototypePatch() {
    if (window.__codexPlusAppServerClientPrototypePatchInstalled === codexAppServerModelRequestPatchVersion) return true;
    const wanted = codexPlusModelUnlockEnabled()
      || codexPluginMarketplacePatchEnabled()
      || (codexPlusBackendSettingsLoaded && codexRemoteSessionProviderPatchEnabled())
      || codexPlusSettings().quotaResume || codexPlusSettings().serviceTierControls
      || codexPlusSettings().sessionDelete || codexPlusSettings().sideChatSave
      || (typeof codexPlusExtensionWantsRequestMiddleware === "function" && codexPlusExtensionWantsRequestMiddleware());
    if (!wanted) return false;
    const klass = window.__codexPlusAppServerClientClass;
    if (!klass || typeof klass !== "function" || !klass.prototype) return false;
    const proto = klass.prototype;
    if (proto.__codexPlusModelRequestPatch === codexAppServerModelRequestPatchVersion) {
      window.__codexPlusAppServerClientPrototypePatchInstalled = codexAppServerModelRequestPatchVersion;
      return true;
    }
    try {
      const descriptor = Object.getOwnPropertyDescriptor(proto, "sendRequest");
      if (!descriptor || descriptor.writable === false) {
        sendCodexPlusDiagnostic("app_server_client_prototype_patch_skipped", {});
        window.__codexPlusAppServerClientPrototypePatchInstalled = codexAppServerModelRequestPatchVersion;
        return false;
      }
    } catch {
      window.__codexPlusAppServerClientPrototypePatchInstalled = codexAppServerModelRequestPatchVersion;
      return false;
    }
    const originalSendRequest = proto.__codexPlusModelOriginalSendRequest || proto.sendRequest;
    proto.__codexPlusModelOriginalSendRequest = originalSendRequest;
    proto.__codexPlusThreadModels = proto.__codexPlusThreadModels || new Map();
    proto.sendRequest = async function codexPlusModelPatchedSendRequest(method, params, options) {
      const client = this;
      registerCodexQuotaResumeClient(client);
      client.__codexPlusThreadModels = client.__codexPlusThreadModels || proto.__codexPlusThreadModels || new Map();
      registerNativeHostClient(client);
      const managed = codexPlusPluginNativeInterceptClient(method, params, options,
        (nextOptions) => originalSendRequest.call(client, method, params, nextOptions), client.hostId);
      if (managed) return await managed;
      const requestMethod = appServerModelRequestMethod(String(method || ""), params);
      let providerRefreshFailed = false;
      const managedAggregateTurn = requestMethod === "turn/start"
        && client.hostId === "local" && !!codexManagedSessionConnection();
      if (codexRemoteSessionProviderRequestMethod(requestMethod)
          && codexRemoteSessionProviderPatchEnabled()
          && window.__codexSessionDeleteBridge
          && !managedAggregateTurn) {
        const settingsLoaded = await loadBackendSettingsState(codexAppServerPreparationTimeoutMs);
        providerRefreshFailed = !settingsLoaded;
        if (providerRefreshFailed) {
          sendCodexPlusDiagnostic("remote_session_provider_refresh_failed", {});
        }
      }
      if (codexRemoteSessionProviderRequestMethod(requestMethod)
          && codexRemoteSessionProviderOverrideEnabled()
          && !Array.isArray(codexModelCatalog?.routeDescriptors)) {
        await loadCodexModelCatalog();
      }
      const providerParams = providerRefreshFailed
        ? params
        : applyCodexRemoteSessionProviderOverride(requestMethod, params);
      const resumeParams = await prepareCodexPureApiResume(String(method || ""), providerParams, client.hostId, providerRefreshFailed, originalSendRequest.bind(client));
      const connectedParams = await prepareCodexManagedSessionConnection(String(method || ""), resumeParams, client.hostId, originalSendRequest.bind(client));
      const nextParams = applyCodexServiceTierRequestOnly(requestMethod, connectedParams);
      const quotaResumeTarget = codexQuotaResumeAppServerRequestTarget(method, nextParams);
      if (quotaResumeTarget?.requestMethod === "turn/start") {
        rememberCodexQuotaResumeTurnRequest(
          client,
          quotaResumeTarget.params,
          quotaResumeTarget.threadIdHint,
        );
      } else if (quotaResumeTarget) {
        mergeCodexQuotaResumeThreadSettings(
          client,
          quotaResumeTarget.params,
          quotaResumeTarget.threadIdHint,
        );
      }
      const boundOriginal = originalSendRequest.bind(client);
      const modelContextRefresh = await refreshCodexThreadModelBeforeTurn(
        client,
        boundOriginal,
        method,
        nextParams,
        options
      );
      if ((codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) !== true) {
        const managedTurn = await sendCodexManagedSessionTurn(client, originalSendRequest.bind(client), String(method || ""), nextParams, options, modelContextRefresh);
        if (managedTurn) return managedTurn.result;
      }
      let result;
      try {
        result = await sendAppServerRequestThroughExchange(client, method, nextParams, () => originalSendRequest.call(client, method, nextParams, options));
        verifyCodexPureApiResumeResult(String(method || ""), nextParams, result, client.hostId);
        noteCodexManagedSessionStart(String(method || ""), nextParams, result, client.hostId);
      } catch (error) {
        if (requestMethod === "turn/start" && codexQuotaResumeValueHasMarker(error)) {
          const failedTurnParams = quotaResumeTarget?.params;
          const threadId = validThreadScrollSessionKey(failedTurnParams?.threadId || failedTurnParams?.conversationId || "");
          if (threadId) {
            const runtime = codexQuotaResumeRuntime();
            runtime.active.delete(threadId);
            runtime.inFlight.delete(threadId);
            runtime.pending.set(threadId, { at: Date.now() });
            trimCodexQuotaResumeMap(runtime.pending);
            refreshCodexQuotaResumeSoon();
          }
        }
        throw error;
      }
      if (codexRateLimitUnlockRequestMethod(requestMethod)) {
        neutralizeCodexRateLimitPayload(result);
      }
      const threadState = codexThreadModelRequestState(String(method || ""), nextParams, result);
      if (modelContextRefresh !== false && threadState.threadId && threadState.model
          && ["thread/start", "thread/resume", "turn/start"].includes(threadState.requestMethod)) {
        client.__codexPlusThreadModels.set(threadState.threadId, threadState.model);
      }
      if (!codexPlusModelUnlockEnabled() || !["list-models-for-host", "model/list"].includes(requestMethod)) return result;
      if (!codexPlusModelNames().length) await loadCodexModelCatalog();
      return patchAppServerModelResult(requestMethod, result, modelResponseHostId(params, client));
    };
    // 所有请求最终都经 createRequest(method, params, options) 组装：拓展的 codex/request 中间件挂在这里。
    // createRequest 是同步的，与 codex/request 的契约一致（中间件同步返回，返回 Promise 视为出错并放行）。
    if (typeof proto.createRequest === "function") {
      const originalCreateRequest = proto.__codexPlusOriginalCreateRequest || proto.createRequest;
      proto.__codexPlusOriginalCreateRequest = originalCreateRequest;
      proto.createRequest = function codexPlusPatchedCreateRequest(method, params, ...rest) {
        let nextParams = params;
        if (typeof codexPlusExtensionWantsRequestMiddleware === "function" && codexPlusExtensionWantsRequestMiddleware()
            && typeof codexPlusApplyExtensionRequestMiddleware === "function") {
          nextParams = codexPlusApplyExtensionRequestMiddleware(String(method || ""), params);
        }
        return originalCreateRequest.call(this, method, nextParams, ...rest);
      };
    }
    if (typeof proto.prewarmThreadStart === "function") {
      const originalPrewarmThreadStart = proto.__codexPlusServiceTierOriginalPrewarmThreadStart
        || proto.prewarmThreadStart;
      proto.__codexPlusServiceTierOriginalPrewarmThreadStart = originalPrewarmThreadStart;
      proto.prewarmThreadStart = async function codexPlusServiceTierPrewarmThreadStart(params, options) {
        return await prewarmCodexManagedSession(this, originalPrewarmThreadStart.bind(this), params, options);
      };
    }
    proto.__codexPlusModelRequestPatch = codexAppServerModelRequestPatchVersion;
    window.__codexPlusAppServerClientPrototypePatchInstalled = codexAppServerModelRequestPatchVersion;
    sendCodexPlusDiagnostic("app_server_client_prototype_patch_installed", {});
    return true;
  }
  const appServerModelRequestPatchMaxMisses = 8;
  const appServerModelRequestPatchMaxRetryDelayMs = 30000;
  let appServerModelRequestPatchMissCount = 0;
  let appServerModelRequestPatchDisabled = false;
  let appServerModelRequestPatchPromise = null;
  let appServerModelRequestPatchRetryTimer = 0;
  let appServerModelRequestPatchRetryDelayMs = 250;

  function scheduleAppServerModelRequestPatchRetry() {
    if (!codexRemoteSessionProviderPatchEnabled() && !codexPlusSettings().quotaResume && !codexPlusSettings().serviceTierControls) return;
    if (appServerModelRequestPatchRetryTimer) return;
    // issue #2256/#2255：固定 250ms 重试在 Codex 改 asset 命名后变成每秒 4 轮的全量
    // rescan（每轮 fetch 全部 app asset）。改为指数退避，miss 计满后由熔断停掉。
    appServerModelRequestPatchRetryTimer = window.setTimeout(() => {
      appServerModelRequestPatchRetryTimer = 0;
      installAppServerModelRequestPatch();
    }, appServerModelRequestPatchRetryDelayMs);
    appServerModelRequestPatchRetryDelayMs = Math.min(appServerModelRequestPatchRetryDelayMs * 4, appServerModelRequestPatchMaxRetryDelayMs);
  }

  function noteAppServerModelRequestPatchMiss(event, detail) {
    appServerModelRequestPatchMissCount += 1;
    // installAppServerModelRequestPatch() runs on every model-whitelist
    // refresh tick (~120ms). On Codex builds where the app-server module was
    // renamed/removed (e.g. 26.623+, issue #1324) this layer never succeeds
    // and would otherwise emit the same diagnostic on every tick forever.
    // Report the first miss so telemetry still captures the cause, then stay
    // quiet, and finally disable this layer once it is clearly unavailable.
    // This is a graceful fallback: the remaining whitelist layers (Statsig
    // config / React state / response JSON patch) keep injecting the custom
    // models on their own.
    if (appServerModelRequestPatchMissCount === 1) {
      sendCodexPlusDiagnostic(event, detail);
    }
    // issue #2256：provider 重试路径以前在这里提前 return，绕过下面的 maxMisses
    // 熔断，失败变成 250ms 无限重试（每轮全量 rescan 全部 app assets）。
    // 现在两个路径统一计数：先按 maxMisses 熔断，未熔断时再走指数退避重试。
    if (appServerModelRequestPatchMissCount >= appServerModelRequestPatchMaxMisses && !appServerModelRequestPatchDisabled) {
      appServerModelRequestPatchDisabled = true;
      clearTimeout(appServerModelRequestPatchRetryTimer);
      appServerModelRequestPatchRetryTimer = 0;
      sendCodexPlusDiagnostic("model_app_server_request_patch_skipped", {
        misses: appServerModelRequestPatchMissCount,
        lastEvent: event,
      });
      return;
    }
    if (!appServerModelRequestPatchDisabled) {
      scheduleAppServerModelRequestPatchRetry();
    }
  }

  function installAppServerModelRequestPatch() {
    if (window.__codexPlusAppServerModelRequestPatchInstalled === codexAppServerModelRequestPatchVersion) return;
    if (appServerModelRequestPatchDisabled) return;
    if (appServerModelRequestPatchRetryTimer) return;
    if (appServerModelRequestPatchPromise) return;
    const patch = async () => {
      try {
        const { modules, candidates, sources, discovery } = await loadAppServerRequestCandidates();
        if (modules.length === 0) {
          noteAppServerModelRequestPatchMiss("model_app_server_request_patch_skipped", {
            reason: "app_server_request_assets_missing",
          });
          return;
        }
        let patchedCount = 0;
        let providerTransportPatched = false;
        let vscodeTransportPatched = false;
        let mcpTransportPatched = false;
        for (const module of modules) {
          if (patchCodexUsageHttpTransport(module)) patchedCount += 1;
          if (patchCodexMcpRequestTransport(module)) {
            patchedCount += 1;
            providerTransportPatched = true;
            mcpTransportPatched = true;
          }
          if (patchCodexVscodeRequestTransport(module)) {
            patchedCount += 1;
            providerTransportPatched = true;
            vscodeTransportPatched = true;
          }
        }
        for (const candidate of candidates) {
          if (!vscodeTransportPatched && candidate && typeof candidate === "object"
              && Object.values(candidate).slice(0, 50).some((value) =>
                typeof value === "function" && typeof value.getInstance === "function")
              && patchCodexVscodeRequestTransport(candidate)) {
            patchedCount += 1;
            providerTransportPatched = true;
            vscodeTransportPatched = true;
          }
          if (patchAppServerModelRequestClient(candidate)) {
            patchedCount += 1;
            providerTransportPatched = true;
          }
        }
        const officialExperience = (codexPlusBackendSettings?.officialExperienceActive ?? codexPlusBackendSettings?.officialExperience?.enabled) === true;
        const providerReady = !codexRemoteSessionProviderPatchEnabled()
          || (officialExperience ? mcpTransportPatched : providerTransportPatched);
        if (patchedCount > 0 && providerReady) {
          clearTimeout(appServerModelRequestPatchRetryTimer);
          appServerModelRequestPatchRetryTimer = 0;
          appServerModelRequestPatchMissCount = 0;
          appServerModelRequestPatchRetryDelayMs = 250;
          window.__codexPlusAppServerModelRequestPatchInstalled = codexAppServerModelRequestPatchVersion;
          sendCodexPlusDiagnostic("model_app_server_request_patch_installed", {
            moduleCount: modules.length,
            candidateCount: candidates.length,
            patchedCount,
            mcpTransportPatched,
            vscodeTransportPatched,
            sources,
            discovery,
          });
        } else {
          noteAppServerModelRequestPatchMiss("model_app_server_request_patch_not_found", {
            moduleCount: modules.length,
            candidateCount: candidates.length,
            sources,
            discovery,
          });
        }
      } catch (error) {
        noteAppServerModelRequestPatchMiss("model_app_server_request_patch_failed", {
          errorName: error?.name || "",
          errorMessage: error?.message || String(error),
        });
      }
    };
    appServerModelRequestPatchPromise = patch().finally(() => {
      appServerModelRequestPatchPromise = null;
    });
    void appServerModelRequestPatchPromise;
  }

  function ensureCodexModelWhitelistInstalls() {
    installCodexThreadGoalForwarding();
    const extensionWantsRequests = typeof codexPlusExtensionWantsRequestMiddleware === "function"
      && codexPlusExtensionWantsRequestMiddleware();
    if (codexPlusModelUnlockEnabled()
        || codexPluginMarketplacePatchEnabled()
        || (codexPlusBackendSettingsLoaded && codexRemoteSessionProviderPatchEnabled())
        || codexPlusSettings().quotaResume || codexPlusSettings().serviceTierControls
        || codexPlusSettings().sessionDelete || codexPlusSettings().sideChatSave
        || extensionWantsRequests) {
      installAppServerModelRequestPatch();
      void installCodexAppServerClientCapture().catch(() => {});
    }
    // 启动器平时只在模型解锁 / 会话删除开启时捕获客户端类；只有拓展需要时，请它按需补开（每页一次）。
    if (extensionWantsRequests && !window.__codexPlusAppServerClientClass && !window.__codexPlusExtensionRequestCaptureAsked) {
      window.__codexPlusExtensionRequestCaptureAsked = true;
      void postJson("/extensions/request-capture", {}).catch(() => {});
    }
    void installDictationSupportPatch();
    if (!codexPlusModelUnlockEnabled()) return;
    installModelJsonResponsePatch();
    patchAppServerModelMessages();
  }

  function runCodexModelWhitelistRefreshPass() {
    if (!codexPlusModelUnlockEnabled() || !codexPlusModelNames().length) return false;
    try {
      patchStatsigModelWhitelist();
      installCodexModelQueryCachePatch();
      installAppServerModelRequestPatch();
    } catch (error) {
      window.__codexPlusModelPatchFailures = window.__codexPlusModelPatchFailures || [];
      window.__codexPlusModelPatchFailures.push(String(error?.stack || error));
    }
    return false;
  }

  function scheduleCodexModelWhitelistRefresh(durationMs = 2500) {
    if (!codexPlusModelUnlockEnabled()) return;
    codexModelWhitelistRefreshUntil = Math.max(codexModelWhitelistRefreshUntil, Date.now() + durationMs);
    if (codexModelWhitelistRefreshTimer) return;
    sendCodexPlusDiagnostic("model_whitelist_refresh_scheduled", { durationMs });
    const tick = () => {
      codexModelWhitelistRefreshTimer = 0;
      runCodexModelWhitelistRefreshPass();
      if (Date.now() < codexModelWhitelistRefreshUntil) {
        codexModelWhitelistRefreshTimer = window.setTimeout(tick, 120);
      }
    };
    tick();
  }

  function refreshCodexModelWhitelistFromScan(mutations) {
    ensureCodexModelWhitelistInstalls();
    if (!codexPlusModelNames().length) {
      loadCodexModelCatalog();
      return;
    }
    runCodexModelWhitelistRefreshPass();
  }

  function threadIdVariants(sessionId) {
    if (typeof sessionId !== "string" || !sessionId.trim()) return [];
    const id = sessionId.trim();
    const bareId = id.startsWith("local:") ? id.slice("local:".length) : id;
    return uniqueValues([id, bareId, `local:${bareId}`]);
  }

  function sessionKey(sessionId) {
    const variants = threadIdVariants(sessionId);
    const bareId = variants.find((id) => !id.startsWith("local:"));
    return bareId || variants[0] || "";
  }

  function uuidV7TimestampMs(sessionId) {
    const id = sessionKey(sessionId).replaceAll("-", "");
    if (!/^[0-9a-fA-F]{12}/.test(id)) return 0;
    const timestamp = Number.parseInt(id.slice(0, 12), 16);
    return Number.isFinite(timestamp) ? timestamp : 0;
  }
