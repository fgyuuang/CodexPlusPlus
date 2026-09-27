import assert from "node:assert/strict";
import test from "node:test";

import { bindUnmappedSourceModels } from "./capabilityBindings.ts";

test("batch binds only unbound models from the selected source", () => {
  const existing = [{
    routingSlug: "CLIProxyAPI:qwen-existing",
    sourceKind: "cliGeneral",
    sourceId: "managed-cliproxy",
    upstreamModel: "qwen-existing",
    capabilitySlug: "gpt-6-astra",
  }];
  const candidates = [
    { routingSlug: "CLIProxyAPI:qwen-existing", sourceKind: "cliGeneral", sourceId: "managed-cliproxy", upstreamModel: "qwen-existing" },
    { routingSlug: "CLIProxyAPI:deepseek-chat", sourceKind: "cliGeneral", sourceId: "managed-cliproxy", upstreamModel: "deepseek-chat" },
    { routingSlug: "CLIProxyAPI:gpt-5.4", sourceKind: "cliGeneral", sourceId: "managed-cliproxy", upstreamModel: "gpt-5.4" },
  ];
  const result = bindUnmappedSourceModels(existing, candidates, ["gpt-5.4", "gpt-6-astra"], "gpt-5.4");
  assert.equal(result.length, 2);
  assert.equal(result[0].capabilitySlug, "gpt-6-astra");
  assert.equal(result[1].routingSlug, "CLIProxyAPI:deepseek-chat");
  assert.equal(result[1].capabilitySlug, "gpt-5.4");
});

test("batch binding rejects templates missing from the current official catalog", () => {
  const candidates = [{ routingSlug: "deepseek:deepseek-chat", sourceKind: "aggregate", sourceId: "aggregate-new", upstreamModel: "deepseek:deepseek-chat" }];
  assert.deepEqual(bindUnmappedSourceModels([], candidates, ["gpt-5.4"], "missing"), []);
  assert.equal(bindUnmappedSourceModels([], candidates, ["gpt-5.4"], "gpt-5.4").length, 1);
});
