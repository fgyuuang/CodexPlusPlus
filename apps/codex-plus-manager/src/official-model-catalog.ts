import bundledCatalog from "../../../crates/codex-plus-core/assets/codex-models.json" with { type: "json" };
import gpt56Compatibility from "../../../assets/gpt56-model-metadata-compat.json" with { type: "json" };
import futureCompatibility from "../../../assets/astra-model-metadata-compat.json" with { type: "json" };

type CatalogEntry = {
  slug?: string;
  visibility?: string;
  supported_in_api?: boolean;
};

type CatalogPayload = { models?: CatalogEntry[] };

let currentVisibleModels: string[] = [];
let currentTrustedModels: string[] = [];

const compatibilityVisibleModels = uniqueSlugs([
  ...((bundledCatalog as CatalogPayload).models ?? []),
  ...((gpt56Compatibility as CatalogPayload).models ?? []),
]);
const compatibilityTrustedModels = uniqueSlugs([
  ...((bundledCatalog as CatalogPayload).models ?? []),
  ...((gpt56Compatibility as CatalogPayload).models ?? []),
  ...((futureCompatibility as CatalogPayload).models ?? []),
]);

export function setOfficialModelCatalog(models: string[], trustedModels = models): void {
  currentVisibleModels = uniqueSlugs(models.map((slug) => ({ slug })));
  currentTrustedModels = uniqueSlugs(trustedModels.map((slug) => ({ slug })));
}

export function visibleOfficialModelSlugs(): string[] {
  return [...(currentVisibleModels.length ? currentVisibleModels : compatibilityVisibleModels)];
}

export function trustedOfficialModelSlugs(): string[] {
  return [...(currentTrustedModels.length ? currentTrustedModels : compatibilityTrustedModels)];
}

export function isTrustedOfficialModel(model: string): boolean {
  const normalized = model.trim().toLowerCase();
  return trustedOfficialModelSlugs().some((candidate) => candidate.toLowerCase() === normalized);
}

function uniqueSlugs(entries: CatalogEntry[]): string[] {
  const seen = new Set<string>();
  const result: string[] = [];
  for (const entry of entries) {
    const slug = String(entry.slug ?? "").trim();
    if (!slug || seen.has(slug.toLowerCase())) continue;
    seen.add(slug.toLowerCase());
    result.push(slug);
  }
  return result;
}
