import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(path.resolve(import.meta.dirname, "../../../assets/extensions/proxy-request-status/status.js"), "utf8");
const formatter = source.slice(source.indexOf("function codexPlusProxyStatusModelLabel("), source.indexOf("function codexPlusProxyStatusFormatDuration("));

test("请求状态显示实际账号来源，原始模型与路由保持不变", () => {
  const model = "codex_plus_official_schedule:gpt-6.1-sol";
  const request = Object.freeze({ model, relayName: "账号2" });
  const sandbox = vm.createContext({
    request,
    codexPlusModelMetadata: (slug: string) => slug === "gpt-6.1-sol" ? { displayName: "GPT-6.1 Sol" } : null,
    codexPlusProxyStatusText: () => ({ selectingAccount: "正在选择账号" }),
  });
  const label = vm.runInContext(`${formatter}; codexPlusProxyStatusModelLabel(request)`, sandbox);
  assert.equal(label, "GPT-6.1 Sol · 账号2");
  assert.equal(request.model, model);
});

test("普通供应商模型和未知别名不会被按名称猜测或剥除", () => {
  const request = Object.freeze({ model: "vendor:custom-model" });
  const sandbox = vm.createContext({ request, codexPlusProxyStatusText: () => ({ selectingAccount: "Selecting account" }) });
  assert.equal(vm.runInContext(`${formatter}; codexPlusProxyStatusModelLabel(request)`, sandbox), request.model);
});

test("尚未确认账号时明确等待选择，确认后显示实际来源", () => {
  const sandbox = vm.createContext({
    request: { model: "codex_plus_official_schedule:gpt-6.1-sol" },
    codexPlusProxyStatusText: () => ({ selectingAccount: "正在选择账号" }),
  });
  assert.equal(vm.runInContext(`${formatter}; codexPlusProxyStatusModelLabel(request)`, sandbox), "gpt-6.1-sol · 正在选择账号");
  vm.runInContext('request.relayName = "账号1"', sandbox);
  assert.equal(vm.runInContext('codexPlusProxyStatusModelLabel(request)', sandbox), "gpt-6.1-sol · 账号1");
  vm.runInContext('request.relayName = "账号2"', sandbox);
  assert.equal(vm.runInContext('codexPlusProxyStatusModelLabel(request)', sandbox), "gpt-6.1-sol · 账号2");
});
