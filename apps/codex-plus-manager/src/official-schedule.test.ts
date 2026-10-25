import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";

test("scheduled quota resume is structured, bounded, and never applies to fixed accounts", async () => {
  const source = await readFile(new URL("../../../assets/inject/renderer-inject/10-style.js", import.meta.url), "utf8");
  const start = source.indexOf("  function codexScheduledQuotaError(");
  const end = source.indexOf("  function observeCodexQuotaResumeEvent(", start);
  const callbacks: (() => void)[] = [];
  const resumed: string[] = [];
  const runtime = { templates: new Map([["thread", { modelProvider: "codex_plus_official_schedule" }]]), pending: new Map([["thread", {}]]), active: new Set<string>() };
  const run = new Function("runtime", "callbacks", "resumed", `
    const codexQuotaResumeRuntime = () => runtime;
    const codexPlusBackendSettings = { officialExperience: { enabled: true, accountScheduleEnabled: true, accountScheduleOrder: ["account2", "account1"] } };
    const window = { setTimeout: callback => callbacks.push(callback) };
    const trimCodexQuotaResumeMap = () => {};
    const resumeCodexQuotaInterruptedTask = threadId => resumed.push(threadId);
    ${source.slice(start, end)}
    return maybeResumeCodexScheduledQuotaTask;
  `)(runtime, callbacks, resumed) as (threadId: string, error: unknown) => boolean;
  assert.equal(run("thread", { code: "rate_limit_exceeded" }), false);
  assert.equal(run("thread", { message: "usage_limit_reached" }), false);
  assert.equal(run("thread", { code: "usage_limit_reached" }), true);
  assert.equal(run("thread", { code: "usage_limit_reached" }), false);
  callbacks[0]();
  assert.deepEqual(resumed, ["thread"]);
  runtime.templates.set("fixed", { modelProvider: "codex_plus_official_account2" });
  assert.equal(run("fixed", { codexErrorInfo: "usageLimitExceeded" }), false);
  runtime.templates.set("other", { modelProvider: "codex_plus_official_schedule" });
  assert.equal(run("other", { errorInfo: "usageLimitExceeded" }), true);
});
