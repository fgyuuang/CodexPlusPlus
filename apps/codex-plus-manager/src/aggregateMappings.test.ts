import assert from "node:assert/strict";
import test from "node:test";

import {
  aggregateDisplayModelEntries,
  type RelayAggregateConfig,
  type RelayProfileLike,
} from "./aggregateMappings.ts";

test("aggregate display lists provider-specific GPT models in member order", () => {
  const members: RelayProfileLike[] = [
    { id: "provider-a", name: "供应商一", model: "gpt-5.4", modelList: "gpt-5.4" },
    { id: "provider-b", name: "供应商二", model: "vendor-gpt-5.4", modelList: "vendor-gpt-5.4" },
  ];
  const aggregate: RelayAggregateConfig = {
    strategy: "failover",
    modelMappingsEnabled: true,
    members: [
      { profileId: "provider-a", weight: 1 },
      { profileId: "provider-b", weight: 1 },
    ],
    modelMappings: [{
      codexModel: "gpt-5.4",
      targets: [
        { profileId: "provider-b", targetModel: "vendor-gpt-5.4" },
        { profileId: "provider-a", targetModel: "gpt-5.4" },
      ],
    }],
  };

  assert.deepEqual(
    aggregateDisplayModelEntries(aggregate, members).map((entry) => entry.alias),
    ["供应商一:gpt-5.4", "供应商二:vendor-gpt-5.4"],
  );
});
