import assert from "node:assert/strict";
import { test } from "node:test";
import { officialUsageRows, officialUsageStatus } from "./official-usage.ts";

test("usage labels actual windows, remaining and resets instead of assuming primary is 5h", () => {
  const rows = officialUsageRows({ primary: { usedPercent: 20, windowMinutes: 10080, resetsAt: 2000 }, secondary: null });
  assert.equal(rows[0].label, "7天额度");
  assert.equal(rows[0].remaining, 80);
  assert.equal(rows[0].resetsAt, 2000);
  assert.equal(rows.length, 1);
  assert.deepEqual(officialUsageRows(null), []);
});

test("shows secondary and additional limits without inventing unlimited quota", () => {
  const rows = officialUsageRows({ primary: { usedPercent: 100, windowMinutes: 300 }, secondary: { usedPercent: 16, windowMinutes: 10080 }, additionalRateLimits: [{ limit_name: "Review", rate_limit: { primary_window: { used_percent: 25, limit_window_seconds: 3600, reset_at: 2000 } } }] });
  assert.deepEqual(rows.map(row => row.remaining), [0, 84, 75]);
  assert.equal(rows[0].label, "5小时额度");
  assert.match(rows[2].label, /Review/);
  assert.equal(officialUsageRows({ primary: { usedPercent: Number.NaN } })[0].remaining, null);
});

test("failed refresh retains snapshot but marks it stale", () => {
  assert.match(officialUsageStatus({ fetchedAt: 1000, error: "network" }, 1100), /刷新失败/);
  assert.match(officialUsageStatus({ fetchedAt: 1000 }, 1400), /缓存/);
  assert.match(officialUsageStatus(null, 1000), /未查询/);
});
