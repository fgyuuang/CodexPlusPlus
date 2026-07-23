export type RelayAggregateStrategy = "failover" | "conversationRoundRobin" | "requestRoundRobin" | "weightedRoundRobin";

export type RelayProfileLike = {
  id: string;
  name: string;
  model: string;
  modelList: string;
};

export type RelayAggregateDispatchTarget = {
  profileId: string;
  targetModel: string;
};

export type RelayAggregateModelMapping = {
  codexModel: string;
  targets: RelayAggregateDispatchTarget[];
};

export type AggregateEffectiveModelMapping = RelayAggregateModelMapping & {
  source: "explicit" | "implicit";
};

export type RelayAggregateConfig = {
  strategy: RelayAggregateStrategy;
  modelMappingsEnabled: boolean;
  members: Array<{ profileId: string; weight: number }>;
  modelMappings: RelayAggregateModelMapping[];
};

export const DEFAULT_CODEX_MODEL_MAPPING_KEYS = [
  "gpt-5.6-sol",
  "gpt-5.6-terra",
  "gpt-5.6-luna",
  "gpt-5.5",
  "gpt-5.4",
  "gpt-5.4-mini",
  "gpt-5.3-codex",
] as const;

export function relayProfileModels(profile: RelayProfileLike): string[] {
  const seen = new Set<string>();
  return profile.modelList
    .split(/\r?\n|,/)
    .concat(profile.model || "")
    .map((item) => item.trim())
    .filter((item) => item.length > 0)
    .filter((item) => {
      if (seen.has(item)) return false;
      seen.add(item);
      return true;
    });
}

export function aggregateProviderLabel(profile: RelayProfileLike, model: string): string {
  return `${profile.name || profile.id}:${model}`;
}

export function aggregateCodexAlias(codexModel: string, profile: RelayProfileLike, targetModel: string): string {
  void codexModel;
  return aggregateProviderLabel(profile, targetModel.trim());
}

export function looksLikeCodexModelKey(model: string): boolean {
  const normalized = model.trim().toLowerCase();
  return normalized.startsWith("gpt-") || normalized.startsWith("codex-");
}

export function aggregateMemberProfileMap(memberProfiles: RelayProfileLike[]): Map<string, RelayProfileLike> {
  return new Map(memberProfiles.map((profile) => [profile.id, profile] as const));
}

export function aggregateEffectiveMappings(
  aggregate: RelayAggregateConfig,
  memberProfiles: RelayProfileLike[],
): AggregateEffectiveModelMapping[] {
  const profileById = aggregateMemberProfileMap(memberProfiles);
  const explicit = aggregate.modelMappings
    .map((mapping) => ({
      codexModel: mapping.codexModel.trim(),
      targets: mapping.targets
        .filter((target) => profileById.has(target.profileId))
        .map((target) => ({
          profileId: target.profileId,
          targetModel: target.targetModel.trim(),
        }))
        .filter((target) => target.targetModel),
      source: "explicit" as const,
    }))
    .filter((mapping) => mapping.codexModel);

  if (!aggregate.modelMappingsEnabled) return explicit;

  const explicitKeys = new Set(explicit.map((mapping) => mapping.codexModel));
  const implicit: AggregateEffectiveModelMapping[] = [];
  for (const profile of memberProfiles) {
    for (const model of relayProfileModels(profile)) {
      const codexModel = model.trim();
      if (!codexModel || !looksLikeCodexModelKey(codexModel)) continue;
      const existing = implicit.find((item) => item.codexModel === codexModel);
      const target = { profileId: profile.id, targetModel: model };
      if (existing) existing.targets.push(target);
      else implicit.push({ codexModel, targets: [target], source: "implicit" });
    }
  }

  return [
    ...explicit,
    ...implicit.filter((mapping) => !explicitKeys.has(mapping.codexModel)),
  ];
}

export function aggregateDisplayModelEntries(
  aggregate: RelayAggregateConfig,
  memberProfiles: RelayProfileLike[],
): Array<{ alias: string; codexModel: string; target: string; viaMapping: boolean; providerId: string }> {
  const profileById = aggregateMemberProfileMap(memberProfiles);
  const memberOrder = new Map(aggregate.members.map((member, index) => [member.profileId, index] as const));
  const effectiveMappings = aggregateEffectiveMappings(aggregate, memberProfiles);
  return effectiveMappings.flatMap((mapping) =>
    [...mapping.targets]
      .sort((left, right) => (
        (memberOrder.get(left.profileId) ?? Number.MAX_SAFE_INTEGER)
        - (memberOrder.get(right.profileId) ?? Number.MAX_SAFE_INTEGER)
      ))
      .flatMap((target) => {
      const profile = profileById.get(target.profileId);
      const codexModel = mapping.codexModel.trim();
      const targetModel = target.targetModel.trim();
      if (!profile || !codexModel || !targetModel) return [];
      return [{
        alias: aggregateCodexAlias(codexModel, profile, targetModel),
        codexModel,
        target: targetModel,
        viaMapping: mapping.source === "explicit",
        providerId: profile.id,
      }];
      }),
  );
}

export function aggregatePersistedMappingsFromEffective(
  mappings: AggregateEffectiveModelMapping[],
): RelayAggregateModelMapping[] {
  return mappings
    .filter((mapping) => mapping.source === "explicit")
    .map((mapping) => ({
      codexModel: mapping.codexModel.trim(),
      targets: mapping.targets
        .map((target) => ({
          profileId: target.profileId.trim(),
          targetModel: target.targetModel.trim(),
        }))
        .filter((target) => target.profileId && target.targetModel),
    }))
    .filter((mapping) => mapping.codexModel);
}

export function aggregateRepeatedModelKeys(memberProfiles: RelayProfileLike[]): string[] {
  const providersByModel = new Map<string, Set<string>>();
  for (const profile of memberProfiles) {
    for (const model of relayProfileModels(profile)) {
      if (looksLikeCodexModelKey(model)) continue;
      if (!providersByModel.has(model)) providersByModel.set(model, new Set());
      providersByModel.get(model)?.add(profile.id);
    }
  }
  return Array.from(providersByModel.entries())
    .filter(([, providers]) => providers.size > 1)
    .map(([model]) => model);
}

export function aggregateMappingKeyOptions(
  effectiveMappings: AggregateEffectiveModelMapping[],
  memberProfiles: RelayProfileLike[],
  currentCodexModel = "",
): string[] {
  return Array.from(
    new Set([
      ...DEFAULT_CODEX_MODEL_MAPPING_KEYS,
      ...effectiveMappings.map((entry) => entry.codexModel),
      ...aggregateRepeatedModelKeys(memberProfiles),
      currentCodexModel.trim(),
    ].filter(Boolean)),
  );
}
