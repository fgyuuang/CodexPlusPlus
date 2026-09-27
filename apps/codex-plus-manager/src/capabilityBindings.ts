export type CapabilityCandidate<SourceKind extends string> = {
  routingSlug: string;
  sourceKind: SourceKind;
  sourceId: string;
  upstreamModel: string;
};

export type CapabilityBinding<SourceKind extends string> = CapabilityCandidate<SourceKind> & {
  capabilitySlug: string;
};

export function bindUnmappedSourceModels<SourceKind extends string>(
  bindings: CapabilityBinding<SourceKind>[],
  candidates: CapabilityCandidate<SourceKind>[],
  officialModels: string[],
  capabilitySlug: string,
): CapabilityBinding<SourceKind>[] {
  if (!officialModels.some((model) => model.toLowerCase() === capabilitySlug.toLowerCase())) return bindings;
  const next = [...bindings];
  const assigned = new Set(bindings.map((binding) => binding.routingSlug.toLowerCase()));
  for (const candidate of candidates) {
    const key = candidate.routingSlug.toLowerCase();
    const exactModel = candidate.routingSlug.includes("(")
      ? candidate.routingSlug.split("(", 1)[0]
      : candidate.upstreamModel;
    if (assigned.has(key) || officialModels.some((model) => model.toLowerCase() === exactModel.toLowerCase())) continue;
    next.push({
      routingSlug: candidate.routingSlug,
      sourceKind: candidate.sourceKind,
      sourceId: candidate.sourceId,
      upstreamModel: candidate.upstreamModel,
      capabilitySlug,
    });
    assigned.add(key);
  }
  return next;
}
