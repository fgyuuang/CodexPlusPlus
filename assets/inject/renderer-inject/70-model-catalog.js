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
      statusSummary: (state = {}) => {
        const summaryState = { ...codexServiceTierState, ...state };
        return serviceTierStatusMessage(
          summaryState.controlMode,
          summaryState.threadMode,
          summaryState.effectiveMode,
          summaryState.defaultMode,
          summaryState.effectiveServiceTier,
          summaryState.serviceTierSource
        );
      },
      resolveInheritedServiceTier: () => resolveInheritedServiceTier(),
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
      setBackendSettings: (settings = {}) => {
        codexPlusBackendSettings = { ...codexPlusBackendSettings, ...settings };
        codexPlusBackendSettingsLoaded = true;
      },
      providerPatchEnabled: () => codexRemoteSessionProviderPatchEnabled(),
      providerNormalizationEnabled: () => codexRemoteSessionProviderNormalizationEnabled(),
      setServiceTierState: (state = {}) => {
        codexServiceTierState = { ...codexServiceTierState, ...state };
      },
      setThreadState: (state = {}) => {
        localStorage.setItem(codexThreadServiceTierKey, JSON.stringify({
          version: codexThreadServiceTierVersion,
          mode: "inherit",
          defaultMode: "inherit",
          entries: {},
          ...state,
        }));
      },
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
    return !!codexPlusSettings().modelWhitelistUnlock;
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
    codexModelCatalogPromise = postJson("/codex-model-catalog", {})
      .then(async (result) => {
        codexModelCatalog = result && typeof result === "object" ? result : { status: "failed", model: "", default_model: "", model_provider: "", codex_model_provider: "", provider_name: "", models: [], officialModels: [], trustedOfficialModels: [], modelMetadata: {}, sources: [], responses_api: { status: "unknown", message: "" } };
        if ((!codexModelCatalog.models || codexModelCatalog.models.length === 0) && codexModelCatalog.status === "not_configured") {
          try {
            const settingsPromise = postJson("/settings/get", {});
            const timeoutPromise = new Promise((_, reject) => setTimeout(() => reject(new Error("fallback timeout")), 3000));
            const settingsResp = await Promise.race([settingsPromise, timeoutPromise]);
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
    const supported = codexModelReasoningEffortValues(modelName);
    if (!supported.length) return providerParams;
    const effortKey = ["reasoningEffort", "reasoning_effort", "effort"].find((key) => Object.prototype.hasOwnProperty.call(target.params, key));
    const current = effortKey ? String(target.params[effortKey] || "").trim() : "";
    if (current && supported.includes(current)) return providerParams;
    const configuredDefault = String(
      codexPlusModelMetadata(modelName)?.defaultReasoningEffort
      || codexBuiltInReasoningMetadata(modelName)?.defaultEffort
      || ""
    ).trim();
    const nextEffort = supported.includes(configuredDefault) ? configuredDefault : supported[0];
    if (!effortKey && target.method !== "thread/settings/update") return providerParams;
    const nextParams = {
      ...target.params,
      [effortKey || "reasoningEffort"]: nextEffort,
    };
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
        if (codexPlusBackendSettings?.officialExperience?.enabled === true) throw error;
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
    for (const key of ["description", "defaultReasoningEffort"]) {
      if (typeof metadata[key] === "string" && metadata[key] && descriptor[key] !== metadata[key]) {
        descriptor[key] = metadata[key];
        changed = true;
      }
    }
    const displayName = codexPlusModelDisplayName(metadata, modelName, descriptor.displayName);
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

  function patchModelNameArray(models) {
    if (!stringArrayLooksPatchable(models)) return false;
    const customModels = codexPlusModelNames();
    if (!customModels.length) return false;
    let changed = false;
    customModels.forEach((modelName) => {
      if (!models.includes(modelName)) {
        models.push(modelName);
        changed = true;
      }
    });
    return changed;
  }

  function patchModelArray(models, allowEmpty = false) {
    if (!modelArrayLooksPatchable(models, allowEmpty)) return false;
    const customModels = codexPlusModelNames();
    if (!customModels.length) return false;
    let changed = false;
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
    return changed;
  }

  function patchModelContainer(value) {
    if (!value || typeof value !== "object") return false;
    let changed = false;
    if (patchModelArray(value.models, "defaultModel" in value || "availableModels" in value)) changed = true;
    if (patchModelNameArray(value.models)) changed = true;
    if (patchModelArray(value.data)) changed = true;
    if (patchModelArray(value.result)) changed = true;
    if (patchModelArray(value.pages?.[0]?.data)) changed = true;
    if (patchModelArray(value.result?.data)) changed = true;
    if (patchModelArray(value.result?.models)) changed = true;
    if (patchModelArray(value.message?.result?.data)) changed = true;
    if (patchModelArray(value.message?.result?.models)) changed = true;
    const names = codexPlusModelNames();
    if (value.availableModels instanceof Set) {
      names.forEach((name) => {
        if (!value.availableModels.has(name)) {
          value.availableModels.add(name);
          changed = true;
        }
      });
    }
    if (value.available_models instanceof Set) {
      names.forEach((name) => {
        if (!value.available_models.has(name)) {
          value.available_models.add(name);
          changed = true;
        }
      });
    }
    if (Array.isArray(value.availableModels)) {
      names.forEach((name) => {
        if (!value.availableModels.includes(name)) {
          value.availableModels.push(name);
          changed = true;
        }
      });
    }
    if (Array.isArray(value.available_models)) {
      names.forEach((name) => {
        if (!value.available_models.includes(name)) {
          value.available_models.push(name);
          changed = true;
        }
      });
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
      patchModelContainer(payload);
    } catch (error) {
      window.__codexPlusModelPatchFailures = window.__codexPlusModelPatchFailures || [];
      window.__codexPlusModelPatchFailures.push(String(error?.stack || error));
    }
    return payload;
  }

  function installModelJsonResponsePatch() {
    if (window.__codexPlusModelJsonResponsePatchInstalled === "1") return;
    window.__codexPlusModelJsonResponsePatchInstalled = "1";
    window.__codexPlusModelJsonResponseOriginals = window.__codexPlusModelJsonResponseOriginals || {};
    const originals = window.__codexPlusModelJsonResponseOriginals;
    originals.responseJson = originals.responseJson || Response.prototype.json;
    if (typeof originals.responseJson !== "function") return;
    Response.prototype.json = async function codexPlusPatchedResponseJson(...args) {
      const payload = await originals.responseJson.apply(this, args);
      return await patchModelJsonResponse(payload);
    };
  }

  function patchStatsigModelDynamicConfig(config) {
    const names = codexPlusModelNames();
    const value = config?.value;
    if (!names.length || !value || typeof value !== "object") return config;
    const availableModels = Array.isArray(value.available_models) ? [...value.available_models] : [];
    let changed = false;
    names.forEach((name) => {
      if (!availableModels.includes(name)) {
        availableModels.push(name);
        changed = true;
      }
    });
    if (!changed) return config;
    const nextValue = { ...value, available_models: availableModels };
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
      if (!client.__codexPlusModelWhitelistPatched) {
        const originalGetDynamicConfig = client.getDynamicConfig.bind(client);
        client.getDynamicConfig = (name, options) => {
          const result = originalGetDynamicConfig(name, options);
          return String(name) === "107580212" ? patchStatsigModelDynamicConfig(result) : result;
        };
        client.__codexPlusModelWhitelistPatched = true;
      }
      try {
        patchStatsigModelDynamicConfig(client.getDynamicConfig("107580212", { disableExposureLog: true }));
      } catch {
      }
    });
  }

  function patchAppServerModelMessages() {
    if (window.__codexPlusModelMessagePatchInstalled) return;
    window.__codexPlusModelMessagePatchInstalled = true;
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
            codexPlusModelListRequestIds.add(requestId);
            if (codexPlusModelListRequestIds.size > 64) {
              codexPlusModelListRequestIds.delete(codexPlusModelListRequestIds.values().next().value);
            }
            window.setTimeout(() => codexPlusModelListRequestIds.delete(requestId), 30_000);
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

  function patchMcpModelResponseData(data) {
    if (!codexPlusModelUnlockEnabled()) return false;
    if (data?.type !== "mcp-response") return false;
    const message = data.message || data.response;
    const requestId = message?.id != null ? String(message.id) : "";
    const requestMethod = String(data.requestMethod || data.request?.method || "");
    if (requestMethod !== "model/list" && !codexPlusModelListRequestIds.has(requestId)) return false;
    if (requestId) codexPlusModelListRequestIds.delete(requestId);
    let changed = false;
    if (patchModelArray(message?.result?.data, true)) changed = true;
    if (patchModelArray(message?.result?.models, true)) changed = true;
    return changed;
  }

  function appServerModelRequestMethod(method, params) {
    if (method === "send-cli-request-for-host" && params?.method) return String(params.method);
    if (method === "vscode://codex/list-plugins") return "list-plugins";
    if (method === "vscode://codex/plugin/install") return "install-plugin";
    if (method === "vscode://codex/plugin/uninstall") return "uninstall-plugin";
    const requestMethod = String(method || "").startsWith("vscode://codex/")
      ? String(method).slice("vscode://codex/".length)
      : String(method || "");
    if (requestMethod === "list-plugins" || requestMethod === "plugin/list") return "list-plugins";
    if (requestMethod === "plugin/install") return "install-plugin";
    if (requestMethod === "plugin/uninstall") return "uninstall-plugin";
    return requestMethod;
  }

  function patchAppServerModelResult(method, result) {
    if (method !== "list-models-for-host" && method !== "model/list") return result;
    try {
      if (Array.isArray(result)) patchModelArray(result, true);
      if (Array.isArray(result?.data)) patchModelArray(result.data, true);
      if (Array.isArray(result?.models)) patchModelArray(result.models, true);
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
    if (String(method || "") !== "turn/start" || !codexPerModelContextEnabled()) return null;
    const { threadId, model } = codexThreadModelRequestState(method, params);
    if (!threadId || !model) return null;
    const previousModel = client.__codexPlusThreadModels?.get(threadId) || "";
    if (!previousModel || previousModel === model) return null;
    let resumeParams = { threadId, model };
    resumeParams = applyCodexRemoteSessionProviderOverride("thread/resume", resumeParams);
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
    const originalSendRequest = client.__codexPlusModelOriginalSendRequest || client.sendRequest.bind(client);
    client.__codexPlusModelOriginalSendRequest = originalSendRequest;
    client.__codexPlusThreadModels = client.__codexPlusThreadModels || new Map();
    registerCodexQuotaResumeClient(client);
    client.sendRequest = async function codexPlusModelPatchedSendRequest(method, params, options) {
      const requestMethod = appServerModelRequestMethod(String(method || ""), params);
      let providerRefreshFailed = false;
      if (codexRemoteSessionProviderRequestMethod(requestMethod)
          && codexRemoteSessionProviderPatchEnabled()
          && window.__codexSessionDeleteBridge) {
        const settingsLoaded = await loadBackendSettingsState();
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
      const nextParams = applyCodexServiceTierRequestOnly(requestMethod, providerParams);
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
      let result;
      try {
        result = await originalSendRequest(method, nextParams, options);
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
      const threadState = codexThreadModelRequestState(requestMethod, nextParams, result);
      if (modelContextRefresh !== false && threadState.threadId && threadState.model
          && ["thread/start", "thread/resume", "turn/start"].includes(threadState.requestMethod)) {
        client.__codexPlusThreadModels.set(threadState.threadId, threadState.model);
      }
      if (!codexPlusModelUnlockEnabled()) return result;
      if (!codexPlusModelNames().length) await loadCodexModelCatalog();
      return patchAppServerModelResult(requestMethod, result);
    };
    if (typeof client.prewarmThreadStart === "function"
        && !client.__codexPlusServiceTierOriginalPrewarmThreadStart) {
      const originalPrewarmThreadStart = client.prewarmThreadStart.bind(client);
      client.__codexPlusServiceTierOriginalPrewarmThreadStart = originalPrewarmThreadStart;
      client.prewarmThreadStart = async function codexPlusServiceTierPrewarmThreadStart(params, options) {
        const nextParams = applyCodexServiceTierRequestOnly("thread/start", params);
        return originalPrewarmThreadStart(nextParams, options);
      };
    }
    client.__codexPlusModelRequestPatch = codexAppServerModelRequestPatchVersion;
    return true;
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
  async function switchCodexOfficialThreadProvider(send, method, params, targetProvider) {
    const threadId = String(params.threadId || params.thread_id || "").trim();
    if (method === "turn/settings/update") {
      throw new Error("当前回合不能更换账号来源；请等回合结束后在此对话中切换，不需要创建新对话。");
    }
    // 已加载的 resume 不会改 provider；先取得当前运行配置，避免重载丢失权限或推理档位。
    const previous = await send("thread/resume", { threadId, excludeTurns: true });
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

      function controlRequest(hostId, method, params) {
        return new Promise((resolve, reject) => {
          const id = `${method}:${crypto.randomUUID()}`;
          const timeoutMs = ["thread/fork", "thread/resume"].includes(method) ? 120000 : 20000;
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
        const params = request.params;
        const requestedModel = String(params.model || params.modelId || params.model_id || "").trim();
        const looksExtended = requestedModel.includes(":") || /\(.+\)/.test(requestedModel);
        if (looksExtended || codexRemoteSessionProviderPatchEnabled()) {
          const settingsLoaded = await loadBackendSettingsState();
          if (!settingsLoaded && looksExtended) {
            throw new Error("扩展模型来源配置不可用，请保持 Codex++ 运行并重试");
          }
          if (!Array.isArray(codexModelCatalog?.routeDescriptors)
              || (looksExtended && !codexOfficialExperienceRouteDescriptor(requestedModel))) await loadCodexModelCatalog(true);
          if (codexPlusBackendSettings?.officialExperience?.enabled === true
              && looksExtended && !codexOfficialExperienceRouteDescriptor(requestedModel)) {
            throw new Error(`扩展模型 ${requestedModel} 没有可用的来源路由`);
          }
        }
        let nextParams = normalizeCodexModelReasoningParams(method, params);
        const hostId = String(detail.hostId || "");
        const threadId = String(nextParams.threadId || nextParams.thread_id || "").trim();
        let targetProvider = String(nextParams.modelProvider || "").trim();
        if (["turn/start", "thread/settings/update", "turn/settings/update"].includes(method) && threadId
            && codexPlusBackendSettings?.officialExperience?.enabled === true) {
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
        if (codexPlusBackendSettings?.officialExperience?.enabled === true && targetProvider) {
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
          if (codexPlusBackendSettings?.officialExperience?.enabled === true
              && looksExtended
              && !codexOfficialExperienceRouteDescriptor(requestedModel)) {
            throw new Error(`扩展模型 ${requestedModel} 没有可用的来源路由`);
          }
        }
        const nextParams = normalizeCodexModelReasoningParams(requestMethod, params);
        const threadId = String(nextParams?.threadId || nextParams?.thread_id || "").trim();
        const targetProvider = String(nextParams?.modelProvider || "").trim();
        if (["turn/start", "thread/settings/update", "turn/settings/update"].includes(requestMethod) && threadId && targetProvider
            && codexPlusBackendSettings?.officialExperience?.enabled === true) {
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

  let codexAppServerClientCaptureStarted = false;

  async function installCodexAppServerClientCapture() {
    if (codexAppServerClientCaptureStarted || window.__codexPlusAppServerClientCapture) return;
    codexAppServerClientCaptureStarted = true;
    try {
      if (typeof fetch !== "function") return;
      const url = codexAppAssetUrl("app-initial-") || await codexAppAssetUrlFromScriptText("app-initial-");
      if (!url) {
        sendCodexPlusDiagnostic("app_server_client_capture_locate_failed", { reason: "asset_url_missing" });
        return;
      }
      const response = await fetch(url);
      const scriptText = response.ok ? await response.text() : "";
      const location = locateCodexAppServerClientBreakpoint(scriptText);
      if (!location) {
        sendCodexPlusDiagnostic("app_server_client_capture_locate_failed", { reason: "anchor_missing" });
        return;
      }
      const urlRegex = url.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      window.__codexPlusAppServerClientCapture = { urlRegex, ...location };
      sendCodexPlusDiagnostic("app_server_client_capture_located", {
        lineNumber: location.lineNumber,
        columnNumber: location.columnNumber,
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
      || (codexPlusBackendSettingsLoaded && codexRemoteSessionProviderPatchEnabled())
      || codexPlusSettings().quotaResume
      || codexPlusSettings().serviceTierControls;
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
      const requestMethod = appServerModelRequestMethod(String(method || ""), params);
      let providerRefreshFailed = false;
      if (codexRemoteSessionProviderRequestMethod(requestMethod)
          && codexRemoteSessionProviderPatchEnabled()
          && window.__codexSessionDeleteBridge) {
        const settingsLoaded = await loadBackendSettingsState();
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
      const nextParams = applyCodexServiceTierRequestOnly(requestMethod, providerParams);
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
      let result;
      try {
        result = await originalSendRequest.call(client, method, nextParams, options);
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
      const threadState = codexThreadModelRequestState(requestMethod, nextParams, result);
      if (modelContextRefresh !== false && threadState.threadId && threadState.model
          && ["thread/start", "thread/resume", "turn/start"].includes(threadState.requestMethod)) {
        client.__codexPlusThreadModels.set(threadState.threadId, threadState.model);
      }
      if (!codexPlusModelUnlockEnabled()) return result;
      if (!codexPlusModelNames().length) await loadCodexModelCatalog();
      return patchAppServerModelResult(requestMethod, result);
    };
    if (typeof proto.prewarmThreadStart === "function"
        && !proto.__codexPlusServiceTierOriginalPrewarmThreadStart) {
      const originalPrewarmThreadStart = proto.prewarmThreadStart;
      proto.__codexPlusServiceTierOriginalPrewarmThreadStart = originalPrewarmThreadStart;
      proto.prewarmThreadStart = async function codexPlusServiceTierPrewarmThreadStart(params, options) {
        const nextParams = applyCodexServiceTierRequestOnly("thread/start", params);
        return originalPrewarmThreadStart.call(this, nextParams, options);
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
        const officialExperience = codexPlusBackendSettings?.officialExperience?.enabled === true;
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
    if (codexPlusModelUnlockEnabled()
        || (codexPlusBackendSettingsLoaded && codexRemoteSessionProviderPatchEnabled())
        || codexPlusSettings().quotaResume
        || codexPlusSettings().serviceTierControls) {
      installAppServerModelRequestPatch();
      void installCodexAppServerClientCapture().catch(() => {});
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
