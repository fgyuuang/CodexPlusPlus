import assert from "node:assert/strict";
import test from "node:test";
import { parseResetCardOperation, resetCardAction, type ResetCardOperation } from "./reset-card-operation.ts";
import { resetCardUsable, soonestResetCardExpiry, usableResetCardCount, type GatewayPoolEntry } from "./gateway-model.ts";

const ready: ResetCardOperation = { status: "ready", confirmation: "a".repeat(64) };
const pending: ResetCardOperation = { status: "pending", confirmation: "b".repeat(64) };
const damaged: ResetCardOperation = { status: "damaged", confirmation: "c".repeat(64) };

test("仅接受后端已知状态的精确操作确认标识", () => {
  for (const value of [ready, pending, damaged]) assert.deepEqual(parseResetCardOperation(value), value);
  for (const value of [null, {}, "pending", { ...pending, confirmation: "B".repeat(64) }, { ...pending, confirmation: true }, { ...pending, status: "unknown" }]) {
    assert.equal(parseResetCardOperation(value), null);
  }
});

test("新消费、待确认重试和损坏修复走不同确认路径", () => {
  assert.equal(resetCardAction(ready, { ...ready }, "consume"), "start");
  assert.equal(resetCardAction(pending, { ...pending }, "consume"), "retry");
  assert.equal(resetCardAction(ready, ready, "abandon"), null);
  assert.equal(resetCardAction(pending, pending, "abandon"), "abandon");
  assert.equal(resetCardAction(damaged, damaged, "abandon"), "abandon");
  assert.equal(resetCardAction(damaged, damaged, "consume"), null);
});

test("另一窗口变更操作后，不修改放弃目标或将重试升级成新消费", () => {
  const otherPending: ResetCardOperation = { ...pending, confirmation: "d".repeat(64) };
  assert.equal(resetCardAction(pending, otherPending, "consume"), null);
  assert.equal(resetCardAction(pending, otherPending, "abandon"), null);
  assert.equal(resetCardAction(pending, ready, "consume"), null);
  assert.equal(resetCardAction(ready, pending, "consume"), null);
  assert.equal(resetCardAction(null, ready, "consume"), null);
});

test("到期时不再信任旧 usable，未知张数保持 null", () => {
  const card = { id: "one", status: "available", usable: true, expiresAtMs: 2000 };
  assert.equal(resetCardUsable(card, 1999), true);
  assert.equal(resetCardUsable(card, 2000), false);
  const entry = { resetCredits: { observedAtMs: 1, credits: [card] } } as GatewayPoolEntry;
  assert.equal(usableResetCardCount(entry, 1999), 1);
  assert.equal(usableResetCardCount(entry, 2000), 0);
  assert.equal(soonestResetCardExpiry(entry, 2000), null);
  entry.resetCredits!.credits = [];
  assert.equal(usableResetCardCount(entry), null);
  entry.resetCredits!.availableCount = 0;
  assert.equal(usableResetCardCount(entry), 0);
});
