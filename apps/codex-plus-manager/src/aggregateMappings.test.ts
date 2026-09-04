import assert from "node:assert/strict";
import test from "node:test";

import {
  aggregateDisplayModelEntries,
  aggregateOrderedModelList,
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

test("aggregate model list keeps official models first and provider models by member order", () => {
  const members: RelayProfileLike[] = [
    { id: "provider-a", name: "供应商一", model: "gpt-5.4", modelList: "gpt-5.4\ngpt-5.6-sol" },
    { id: "provider-b", name: "供应商二", model: "vendor-gpt-5.4", modelList: "vendor-gpt-5.4" },
  ];
  const aggregate: RelayAggregateConfig = {
    strategy: "failover",
    modelMappingsEnabled: true,
    members: [
      { profileId: "provider-a", weight: 1 },
      { profileId: "provider-b", weight: 1 },
    ],
    modelMappings: [],
  };

  assert.deepEqual(aggregateOrderedModelList(aggregate, members), [
    "gpt-5.6-sol",
    "gpt-5.4",
    "供应商一:gpt-5.4",
    "供应商一:gpt-5.6-sol",
    "供应商二:vendor-gpt-5.4",
  ]);
});

test("managed CLIProxyAPI general relay keeps the CLIProxyAPI model prefix", () => {
  const members: RelayProfileLike[] = [
    {
      id: "managed-cliproxy",
      name: "CLIProxyAPI 通用中转",
      integrationType: "cliproxy",
      model: "gemini-2.5-pro",
      modelList: "gemini-2.5-pro",
    },
  ];
  const aggregate: RelayAggregateConfig = {
    strategy: "failover",
    modelMappingsEnabled: true,
    members: [{ profileId: "managed-cliproxy", weight: 1 }],
    modelMappings: [],
  };

  assert.deepEqual(aggregateOrderedModelList(aggregate, members), [
    "CLIProxyAPI:gemini-2.5-pro",
  ]);
});

test("official mixed mode keeps native models plain and appends replacement labels", () => {
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
        { profileId: "provider-a", targetModel: "gpt-5.4" },
        { profileId: "provider-b", targetModel: "vendor-gpt-5.4" },
      ],
    }],
  };

  assert.deepEqual(
    aggregateOrderedModelList(
      aggregate,
      members,
      ["gpt-5.6-sol", "gpt-5.6-terra"],
      true,
      ["CLIProxyAPI:gpt-5.6-sol"],
      ["CLIProxyAPI:gemini-2.5-pro"],
    ),
    [
      "gpt-5.6-sol",
      "gpt-5.6-terra",
      "CLIProxyAPI:gpt-5.6-sol",
      "gpt-5.4(供应商一|供应商二:vendor-gpt-5.4)",
      "CLIProxyAPI:gemini-2.5-pro",
      "供应商一:gpt-5.4",
      "供应商二:vendor-gpt-5.4",
    ],
  );
});

test("CLIProxyAPI general models follow aggregate replacements when special official handling is off", () => {
  const members: RelayProfileLike[] = [
    { id: "provider-a", name: "供应商一", model: "gpt-5.4", modelList: "gpt-5.4" },
  ];
  const aggregate: RelayAggregateConfig = {
    strategy: "failover",
    modelMappingsEnabled: true,
    members: [{ profileId: "provider-a", weight: 1 }],
    modelMappings: [],
  };

  assert.deepEqual(
    aggregateOrderedModelList(
      aggregate,
      members,
      ["gpt-5.6-sol", "gpt-5.4"],
      true,
      [],
      ["CLIProxyAPI:gpt-5.6-sol", "CLIProxyAPI:gemini-2.5-pro"],
    ),
    [
      "gpt-5.6-sol",
      "gpt-5.4",
      "gpt-5.4(供应商一)",
      "CLIProxyAPI:gpt-5.6-sol",
      "CLIProxyAPI:gemini-2.5-pro",
      "供应商一:gpt-5.4",
    ],
  );
});
