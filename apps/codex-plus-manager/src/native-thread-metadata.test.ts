import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { getEventListeners } from "node:events";
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
    forkedFromId: null, parentThreadId: null, isSubagent: false, statusType: "unknown" });
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

async function bridgeReader(send: (message: any) => unknown) {
  const source = await readFile(new URL("../../../assets/inject/renderer-inject/70-model-catalog.js", import.meta.url), "utf8");
  const start = source.indexOf("  function installCodexNativeThreadMetadataReader(");
  const end = source.indexOf("\n  function patchCodexMcpRequestTransport(", start);
  const window = new EventTarget() as EventTarget & Record<string, any>;
  window.electronBridge = Object.freeze({ sendMessageFromView: send });
  const install = new Function("window", `${source.slice(start, end)}\nreturn installCodexNativeThreadMetadataReader;`)(window);
  install();
  return { window, install };
}

test("frozen preload supports metadata reads without discovering legacy modules", async () => {
  const requests: any[] = [];
  const { window, install } = await bridgeReader((message) => {
    requests.push(message);
    queueMicrotask(() => {
      window.dispatchEvent(new MessageEvent("message", { data: { type: "mcp-response", hostId: "other",
        message: { id: message.request.id, result: { thread: { id: "wrong" } } } } }));
      window.dispatchEvent(new MessageEvent("message", { data: { type: "mcp-response", hostId: "local",
        message: { id: message.request.id, result: { thread: { id, ephemeral: true, modelProvider: "custom",
          preview: "private" } } } } }));
    });
  });
  const original = window.__codexPlusReadNativeThreadMetadata;
  install();
  assert.equal(window.__codexPlusReadNativeThreadMetadata, original);
  assert.deepEqual(await original(id), { id, sessionId: null, ephemeral: true, modelProvider: "custom",
    forkedFromId: null, parentThreadId: null, isSubagent: false, statusType: "unknown" });
  assert.equal(requests.length, 1);
  assert.equal(requests[0].type, "mcp-request");
  assert.equal(requests[0].hostId, "local");
  assert.equal(requests[0].request.method, "thread/read");
  assert.deepEqual(requests[0].request.params, { threadId: id, includeTurns: false });
  const source = await readFile(new URL("../../../assets/inject/renderer-inject/70-model-catalog.js", import.meta.url), "utf8");
  assert.match(source, /function installCodexModelRequestBridgePatch\(\) \{\s*installCodexNativeThreadMetadataReader\(\);/);
});

test("metadata bridge rejects native and async transport errors without retrying", async () => {
  let calls = 0;
  const { window } = await bridgeReader(() => { calls++; return Promise.reject(new Error("transport closed")); });
  await assert.rejects(window.__codexPlusReadNativeThreadMetadata(id), /transport closed/);
  await assert.rejects(window.__codexPlusReadNativeThreadMetadata("invalid"), /invalid/);
  assert.equal(calls, 1);
  const native = await bridgeReader((message) => {
    queueMicrotask(() => native.window.dispatchEvent(new MessageEvent("message", {
      data: { type: "mcp-response", hostId: "local", message: { id: message.request.id, error: { message: "not found" } } },
    })));
  });
  await assert.rejects(native.window.__codexPlusReadNativeThreadMetadata(id), /not found/);
  assert.equal(getEventListeners(native.window, "message").length, 0);
  const sync = await bridgeReader(() => { throw new Error("send failed"); });
  await assert.rejects(sync.window.__codexPlusReadNativeThreadMetadata(id), /send failed/);
  assert.equal(getEventListeners(sync.window, "message").length, 0);
});

test("metadata read timeout removes the pending listener without retrying", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  let calls = 0;
  const { window } = await bridgeReader(() => { calls++; });
  const result = window.__codexPlusReadNativeThreadMetadata(id);
  const rejected = assert.rejects(result, /timed out/);
  assert.equal(getEventListeners(window, "message").length, 1);
  t.mock.timers.tick(4000);
  await rejected;
  assert.equal(getEventListeners(window, "message").length, 0);
  assert.equal(calls, 1);
});
