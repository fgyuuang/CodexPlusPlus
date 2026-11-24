import assert from "node:assert/strict";
import { test } from "node:test";
import { summarizeWeixinFailures } from "./weixin-queue-model.ts";

test("counts only pending messages behind failures from the same peer", () => {
  const failed = { peerId: "a", state: "failed", key: "failure" };
  const messages = [
    { peerId: "a", state: "pending" },
    failed,
    { peerId: "b", state: "pending" },
    { peerId: "a", state: "processing" },
    { peerId: "a", state: "pending" },
  ];
  assert.deepEqual(summarizeWeixinFailures(messages), { failedMessages: [failed], blockedCount: 1 });
  assert.deepEqual(summarizeWeixinFailures(messages.filter(item => item !== failed)), {
    failedMessages: [], blockedCount: 0,
  });
});
