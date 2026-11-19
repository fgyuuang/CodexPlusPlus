import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { averageDuration, costText, formatRequestCost, niceCeiling, usageDayStats, workbuddyCreditsText, workbuddyModelRate, type UsageDay } from "./gateway-model.ts";

const read = (name: string) => readFileSync(new URL(`./${name}`, import.meta.url), "utf8");
const counts = { calls: 0, errors: 0, inputTokens: 0, outputTokens: 0, cachedTokens: 0, reasoningTokens: 0 };
const day = (date: string, calls: number, tokens: number): UsageDay => ({ ...counts, date, calls, inputTokens: tokens });

test("纵轴取 1 / 2 / 5 × 10ⁿ 的整数上限", () => {
  assert.equal(niceCeiling(0), 1);
  assert.equal(niceCeiling(3_700_000), 5_000_000);
  assert.equal(niceCeiling(11_000_000), 20_000_000);
  assert.equal(niceCeiling(1000), 1000);
});

test("概况：只算有请求的日子，峰值按 token", () => {
  const stats = usageDayStats([day("2026-10-01", 0, 0), day("2026-10-02", 3, 100), day("2026-10-03", 1, 300)]);
  assert.equal(stats.activeDays, 2);
  assert.equal(stats.peak?.date, "2026-10-03");
  assert.equal(stats.dailyAverage, 200);
  assert.equal(usageDayStats([day("2026-10-01", 0, 0)]).peak, null);
});

test("平均耗时只按计过时的请求算；费用跳过 0", () => {
  assert.equal(averageDuration(counts), null);
  assert.equal(averageDuration({ ...counts, calls: 5, durationMs: 6000, timedCalls: 2 }), 3000);
  assert.equal(costText([{ currency: "USD", amount: 0 }]), "");
  assert.equal(costText(undefined), "");
});

test("WorkBuddy 实际积分区分未知、零扣费和小额扣费", () => {
  assert.equal(workbuddyCreditsText(undefined), "");
  assert.equal(workbuddyCreditsText(0), "0 积分");
  assert.equal(formatRequestCost({ currency: "积分", amount: 0.000375 }), "0.000375 积分");
  assert.equal(formatRequestCost({ currency: "USD", amount: 0 }), "—");
});

test("参考倍率匹配实际模型版本，零倍率和未公布不混淆", () => {
  const rates = { "cn:flash": 0, "global:flash": 1.25 };
  assert.equal(workbuddyModelRate(rates, "flash"), "×0");
  assert.equal(workbuddyModelRate(rates, "global:flash"), "×1.25");
  assert.equal(workbuddyModelRate({ "global:flash": 1.25 }, "flash"), "");
  assert.equal(workbuddyModelRate(rates, "auto"), "");
});

test("用量页：费用并进统计格，明细是带表头的表格，没填价格时不占费用列", () => {
  const usage = read("GatewayUsage.tsx");
  // 费用是数字一排（CxStats）的第一格。
  assert.match(usage, /<CxStats\s+items=\{\[\s*\{\s*label: t\(credits \? "费用与积分" : "估算费用"\)/);
  assert.doesNotMatch(usage, /gw-usage-cost/);
  assert.match(usage, /gw-usage-row gw-usage-row-head/);
  assert.match(usage, /data-cost=\{anyCost\}/);
  assert.match(usage, /<AgentShare groups=\{summary\.byAgent\} \/>/);
  assert.match(usage, /className="gw-bar-empty"/);
  const css = read("GatewayPage.css");
  assert.match(css, /\.shell \.gw-usage-table\[data-cost="false"\] \.gw-usage-row > :nth-child\(7\)/);
});
