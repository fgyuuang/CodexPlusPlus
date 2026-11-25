/** 入门清单只读取现有配置；请求测试针对具体供应商模型，不代表整条路由都已验证。 */
import { agentVisible, type AgentFocus } from "./agent-focus-model.ts";
import {
  activeAutoGroup,
  agentPoolProviders,
  globMatches,
  GROUP_MODEL_PREFIX,
  isPattern,
  modelRoutable,
  qualifiedProvider,
  servingProviders,
  type GatewayAgentView,
  type GatewayGroupEntry,
  type GatewayPayload,
  type GatewayPoolEntry,
} from "./gateway-model.ts";

export const SETUP_STORAGE_KEY = "codex-plus-plus.setup";
/**
 * 旧版「使用模式」问卷的本地记录。使用模式已经取消（所有功能始终显示），这条记录只用来认出老用户：
 * 答过那份问卷的人不再被自动弹出的向导打断。
 */
export const LEGACY_FOCUS_STORAGE_KEY = "codex-plus-plus.agent-focus";
const SETUP_VERSION = 1;

/** completed：走完了；dismissed：关掉不再显示；later：向导出现过、还没做完，侧栏留着进度卡。 */
export type SetupState = "completed" | "dismissed" | "later";

export function serializeSetupState(state: SetupState): string {
  return JSON.stringify({ version: SETUP_VERSION, state });
}

function savedSetupState(storedSetup: string | null): SetupState | null {
  try {
    const saved: unknown = JSON.parse(storedSetup ?? "null");
    if (saved && typeof saved === "object" && "version" in saved && "state" in saved
      && typeof saved.version === "number" && saved.version >= SETUP_VERSION
      && (saved.state === "completed" || saved.state === "dismissed" || saved.state === "later")) return saved.state;
  } catch { /* 无有效记录时按首次启动处理。 */ }
  return null;
}

/** 侧栏要不要有进度卡：没走完、也没关掉。看过旧导览的老用户不因新增引导再被打断；设置里的主动重开不受此规则限制。 */
export function shouldShowSetup(storedSetup: string | null, storedTour: string | null): boolean {
  if (storedTour?.trim()) return false;
  const state = savedSetupState(storedSetup);
  return state !== "completed" && state !== "dismissed";
}

/**
 * 向导要不要自动打开：只在它从没出现过的时候。出现过一次（记了 later）之后只留进度卡，
 * 答过旧版使用模式问卷的老用户也不再弹。
 */
export function shouldAutoOpenSetup(storedSetup: string | null, storedTour: string | null, storedLegacyFocus: string | null): boolean {
  return shouldShowSetup(storedSetup, storedTour) && savedSetupState(storedSetup) === null && !storedLegacyFocus?.trim();
}

export type SetupProbeCandidate = {
  id: string;
  providerId: string;
  providerName: string;
  model: string;
};

export type SetupGatewayIssue = "loading" | "agent_missing" | "no_providers" | "not_connected"
  | "drift" | "helper_stopped" | "no_model" | "unroutable";

export type SetupGatewayState = {
  agent: GatewayAgentView | null;
  model: string;
  providersConfigured: boolean;
  bindingReady: boolean;
  issue: SetupGatewayIssue | null;
  candidates: SetupProbeCandidate[];
  fingerprint: string;
  multipleTargets: boolean;
};

export type SetupProbe = { fingerprint: string; candidateId: string; ok: boolean };

export function hasCodexEnhancements(config: {
  settings: { enhancementsEnabled: boolean; codexAppStandaloneQuotaUnlock: boolean };
} | null): boolean {
  if (!config) return false;
  // 普通拓展注入也受增强总开关控制；独立额度解锁是启动器支持的例外。
  return config.settings.enhancementsEnabled || config.settings.codexAppStandaloneQuotaUnlock;
}

function chatModel(entry: GatewayPoolEntry, model: string): boolean {
  const lower = model.trim().toLowerCase();
  return Boolean(lower) && !(entry.imageModels ?? []).some((image) => image.toLowerCase() === lower);
}

function probeCandidates(payload: GatewayPayload, model: string, exclude: string[]): SetupProbeCandidate[] {
  const candidates = new Map<string, SetupProbeCandidate>();
  const providers = agentPoolProviders(payload, exclude);
  const add = (entry: GatewayPoolEntry, upstream: string) => {
    if (!chatModel(entry, upstream)) return;
    const id = JSON.stringify([entry.id, upstream.toLowerCase()]);
    if (!candidates.has(id)) candidates.set(id, { id, providerId: entry.id, providerName: entry.name, model: upstream });
  };
  const expand = (value: string, visiting: string[]) => {
    const target = value.trim();
    const group = target.toLowerCase().startsWith(GROUP_MODEL_PREFIX)
      ? payload.groups.find((item) => item.id.toLowerCase() === target.slice(GROUP_MODEL_PREFIX.length).toLowerCase())
      : activeAutoGroup(payload, target);
    if (group) {
      if (group.disabled) return;
      // 与后端一致，组最多嵌套四层；手动模式只尝试选中的成员。
      if (visiting.length >= 4 || visiting.includes(group.id)) return;
      const members = group.mode === "manual"
        ? [group.members.find((member) => member.model.toLowerCase() === group.pick?.trim().toLowerCase()) ?? group.members[0]].filter(Boolean)
        : group.members;
      for (const member of members) expand(member.model, [...visiting, group.id]);
      return;
    }
    if (target.toLowerCase().startsWith(GROUP_MODEL_PREFIX)) return;
    if (isPattern(target)) {
      for (const entry of providers) for (const upstream of entry.models) {
        const qualified = `${entry.id}/${upstream}`;
        if (globMatches(target, upstream)
          || (globMatches(target, qualified) && !servingProviders(payload, qualified, exclude).length)) add(entry, upstream);
      }
      return;
    }
    // 带斜杠的完整模型名优先按裸名路由，避免把 openai/gpt 等误拆成供应商前缀。
    const serving = servingProviders(payload, target, exclude);
    if (serving.length) {
      for (const entry of serving) add(entry, entry.models.find((known) => known.toLowerCase() === target.toLowerCase()) ?? target);
      return;
    }
    const qualified = qualifiedProvider(payload, target, exclude);
    if (qualified) add(qualified.entry, qualified.model);
  };
  expand(model, []);
  return [...candidates.values()];
}

function groupConfiguration(group: GatewayGroupEntry) {
  return {
    id: group.id,
    disabled: group.disabled === true,
    mode: group.mode,
    sticky: group.sticky ?? "cache",
    pick: group.pick ?? "",
    members: group.members.map(({ model, weight }) => ({ model, weight })),
  };
}

/** 内存中的配置版本标记；地址和代理可能含凭据，因此不把原文作为标记返回。 */
function configurationFingerprint(value: unknown): string {
  const text = JSON.stringify(value);
  let hash = 0xcbf29ce484222325n;
  for (let index = 0; index < text.length; index += 1) {
    hash = BigInt.asUintN(64, (hash ^ BigInt(text.charCodeAt(index))) * 0x100000001b3n);
  }
  return hash.toString(16);
}

export function setupGatewayState(focus: AgentFocus, payload: GatewayPayload | null, selectedAgentId: string): SetupGatewayState {
  const agentId = focus === "codex" ? "codex" : selectedAgentId;
  const agent = payload?.agents.find((item) => item.agent === agentId && agentVisible(focus, item.agent)) ?? null;
  const binding = payload?.gateway.bindings[agentId];
  const exclude = binding?.poolExclude ?? [];
  const providers = agentPoolProviders(payload, exclude);
  const model = agent?.defaultModel.trim() ?? "";
  const candidates = payload && agent && model ? probeCandidates(payload, model, exclude) : [];
  const providersConfigured = providers.some((entry) => entry.models.some((name) => chatModel(entry, name))) || candidates.length > 0;
  const issue: SetupGatewayIssue | null = !payload ? "loading"
    : !agent ? "agent_missing"
    : !providersConfigured ? "no_providers"
    : !agent.enabled || binding?.enabled === false ? "not_connected"
    : agent.drift ? "drift"
    : !payload.helperRunning ? "helper_stopped"
    : !model ? "no_model"
    : !modelRoutable(payload, model, exclude) || !candidates.length ? "unroutable"
    : null;
  const fingerprint = configurationFingerprint({
    focus,
    agentId,
    port: payload?.port,
    helperRunning: payload?.helperRunning,
    agent: agent ? { enabled: agent.enabled, model, baseUrl: agent.baseUrl, drift: Boolean(agent.drift) } : null,
    binding: binding ? {
      enabled: binding.enabled,
      model: binding.model,
      extraModels: binding.extraModels,
      claudeRoles: binding.claudeRoles,
      unknownModel: binding.unknownModel,
      autoReviewModel: binding.autoReviewModel,
      titleModel: binding.titleModel,
      contextWindow: binding.contextWindow,
      claudeEnableEffort: binding.claudeEnableEffort,
      poolExclude: exclude,
    } : null,
    keyPolicy: payload?.gateway.keyPolicy.mode,
    autoModels: payload?.gateway.autoModels !== false,
    providers: providers.map((entry) => ({
      id: entry.id,
      baseUrl: entry.baseUrl,
      protocol: entry.protocol,
      official: entry.official,
      models: entry.models,
      imageModels: entry.imageModels,
      account: entry.account,
      keyCount: entry.keyCount,
      keys: entry.keys?.map(({ id, masked }) => ({ id, masked })),
      proxy: entry.proxy,
      keyRpm: entry.keyRpm,
      keyConcurrency: entry.keyConcurrency,
      stripImageGenerationTool: entry.stripImageGenerationTool,
      modelMetadata: entry.modelMetadata,
      modelWindows: entry.modelWindows,
      contextWindow: entry.contextWindow,
    })),
    groups: payload?.groups.map(groupConfiguration),
    autoGroups: payload?.autoGroups?.map((group) => ({ ...groupConfiguration(group), hidden: group.hidden })),
  });
  return { agent, model, providersConfigured, bindingReady: issue === null, issue, candidates, fingerprint, multipleTargets: candidates.length > 1 };
}

/** 只承认当前配置下的具体候选成功；不把单个候选的成功扩展为整个模型组均可用。 */
export function probeMatches(state: SetupGatewayState, probe: SetupProbe | null | undefined): boolean {
  return state.bindingReady && probe?.ok === true && probe.fingerprint === state.fingerprint
    && state.candidates.some((candidate) => candidate.id === probe.candidateId);
}

/** 网关接管 Codex 时放进直连列表的托管供应商（core 的 `CODEX_GATEWAY_RELAY_ID`）。 */
const GATEWAY_RELAY_ID = "codex-plus-gateway";

/** 只取启动引导用得到的字段；App 里完整的 RelayProfile / BackendSettings 结构上兼容。 */
export type SetupRelayProfile = {
  id: string;
  name: string;
  relayMode: string;
  officialMixApiKey: boolean;
  baseUrl: string;
  apiKey: string;
  noAuth: boolean;
};
export type SetupRelaySettings = {
  relayProfilesEnabled: boolean;
  activeRelayId: string;
  relayProfiles: SetupRelayProfile[];
};
/** `relay_status` 的结果：只读 Codex 的 auth.json / config.toml，不发网络请求。 */
export type SetupRelayStatus = { authenticated: boolean; configured: boolean };

export type SetupDirectMode = "official" | "mixedApi" | "pureApi" | "aggregate" | "gateway";
export type SetupDirectIssue = "loading" | "switching_off" | "no_profile" | "not_logged_in" | "api_missing" | "not_configured";
export type SetupDirectState = {
  profile: SetupRelayProfile | null;
  mode: SetupDirectMode;
  issue: SetupDirectIssue | null;
  ready: boolean;
  /** 聚合供应商会在多个成员间轮转，单条请求测不出整体是否可用，交给供应商页逐个测。 */
  testable: boolean;
  fingerprint: string;
};

export function setupDirectMode(profile: SetupRelayProfile): SetupDirectMode {
  if (profile.id === GATEWAY_RELAY_ID) return "gateway";
  if (profile.relayMode === "aggregate") return "aggregate";
  if (profile.relayMode === "pureApi") return "pureApi";
  if (profile.relayMode === "mixedApi" || profile.officialMixApiKey) return "mixedApi";
  return "official";
}

/** Codex 直连的就绪规则：官方登录看 ChatGPT 登录，混入 API 还要 Base URL 和 Key，其余看写入 Codex 的配置是否完整。 */
export function setupDirectState(settings: SetupRelaySettings | null, status: SetupRelayStatus | null): SetupDirectState {
  const profile = settings
    ? settings.relayProfiles.find((item) => item.id === settings.activeRelayId) ?? settings.relayProfiles[0] ?? null
    : null;
  const mode = profile ? setupDirectMode(profile) : "official";
  const apiFilled = Boolean(profile?.baseUrl.trim() && (profile.apiKey.trim() || profile.noAuth));
  const issue: SetupDirectIssue | null = !settings || !status ? "loading"
    : !settings.relayProfilesEnabled ? "switching_off"
    : !profile ? "no_profile"
    : mode === "official" ? (status.authenticated ? null : "not_logged_in")
    : mode === "mixedApi" ? (!status.authenticated ? "not_logged_in" : apiFilled ? null : "api_missing")
    : status.configured || (mode === "pureApi" && profile.noAuth && apiFilled) ? null : "not_configured";
  const fingerprint = configurationFingerprint({
    enabled: settings?.relayProfilesEnabled,
    profile,
    authenticated: status?.authenticated,
    configured: status?.configured,
  });
  return { profile, mode, issue, ready: issue === null, testable: mode !== "aggregate", fingerprint };
}
