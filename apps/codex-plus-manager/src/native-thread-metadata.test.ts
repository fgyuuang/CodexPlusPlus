import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { test } from "node:test";

const id = "01a180b9-575b-7c00-9d9a-b121603c8eb3";

async function reader(result: unknown, calls: unknown[][]) {
  const source = await readFile(new URL("../../../assets/inject/renderer-inject/70-model-catalog.js", import.meta.url), "utf8");
  const start = source.indexOf("  function installCodexNativeThreadMetadataReader(");
  const end = source.indexOf("\n  function patchCodexMcpRequestTransport(", start);
  assert.ok(start >= 0 && end > start);
  const window = {};
  new Function("window", "controlRequest", `${source.slice(start, end)}\ninstallCodexNativeThreadMetadataReader(controlRequest);`)(window, async (...args: unknown[]) => {
    calls.push(args);
    return result;
  });
  return window as { __codexPlusReadNativeThreadMetadata: (id: string) => Promise<Record<string, unknown>> };
}

test("auxiliary identity proof reads the same local native app-server and returns no conversation data", async () => {
  const calls: unknown[][] = [];
  const window = await reader({ thread: { id, sessionId: id, ephemeral: true, modelProvider: "openai",
    forkedFromId: null, parentThreadId: null, source: "vscode", turns: [{ prompt: "private" }],
    preview: "private", auth: "secret", name: "private" } }, calls);
  const metadata = await window.__codexPlusReadNativeThreadMetadata(id);
  assert.deepEqual(calls, [["local", "thread/read", { threadId: id, includeTurns: false }, 4000]]);
  assert.deepEqual(metadata, { id, sessionId: id, ephemeral: true, modelProvider: "openai",
    forkedFromId: null, parentThreadId: null, isSubagent: false });
  assert.equal(JSON.stringify(metadata).includes("private"), false);
  assert.equal(JSON.stringify(metadata).includes("secret"), false);
  assert.equal(Object.getOwnPropertyDescriptor(window, "__codexPlusReadNativeThreadMetadata")?.writable, false);
});

test("invalid identities and mismatched native responses cannot create routing proofs", async () => {
  const calls: unknown[][] = [];
  const window = await reader({ thread: { id: "other", ephemeral: true, modelProvider: "openai" } }, calls);
  await assert.rejects(window.__codexPlusReadNativeThreadMetadata("not-a-thread"), /invalid/);
  assert.equal(calls.length, 0);
  await assert.rejects(window.__codexPlusReadNativeThreadMetadata(id), /unavailable/);
  for (const thread of [{ id, modelProvider: "openai" }, { id, ephemeral: true }]) {
    const incomplete = await reader({ thread }, []);
    await assert.rejects(incomplete.__codexPlusReadNativeThreadMetadata(id), /unavailable/);
  }
});
