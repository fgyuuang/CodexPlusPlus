import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const source = readFileSync(new URL("../../../assets/extensions/local-services/panel.js", import.meta.url), "utf8");
const html = readFileSync(new URL("../../../assets/extensions/local-services/panel.html", import.meta.url), "utf8");
class Element {
  id = ""; value = ""; textContent = ""; checked = false; disabled = false; hidden = true; required = false;
  dataset: Record<string, string> = {}; children: Element[] = [];
  onclick: (() => unknown) | undefined;
  onsubmit: ((event: { preventDefault(): void }) => unknown) | undefined;
  appendChild(child: Element) { this.children.push(child); if (!this.value && child.value) this.value = child.value; }
  replaceChildren() { this.children = []; this.value = ""; }
}
async function panel(service: "cliproxy" | "newapi") {
  const elements = new Map<string, Element>();
  for (const match of html.matchAll(/id="([^"]+)"/g)) elements.set(match[1], new Element());
  const buttons = [...html.matchAll(/data-action="([^"]+)"/g)].map((match) => {
    const button = new Element(); button.dataset.action = match[1]; return button;
  });
  const controls: Element[] = [...elements.values(), ...buttons];
  const calls: { route: string; payload: { action: string; request: Record<string, unknown> } }[] = [];
  const navigation: string[] = [];
  let failure = "";
  const status = { status: "ok", message: "已刷新", healthy: true, apiKey: undefined, hasApiKey: true, baseUrl: "http://127.0.0.1:8317/v1" };
  vm.runInNewContext(source, {
    window: { codexPlusManager: {
      call: async (route: string, payload: typeof calls[number]["payload"]) => {
        calls.push({ route, payload });
        if (failure) return { status: "failed", message: failure };
        return payload.action === "models" ? { status: "ok", models: ["safe-model", "<img src=x>", "safe-model"] } : status;
      },
      navigate: (route: string) => navigation.push(route),
    } },
    document: {
      body: { dataset: { service } },
      getElementById: (id: string) => elements.get(id) || controls.find((item) => item.id === id),
      createElement: () => { const item = new Element(); controls.push(item); return item; },
      querySelectorAll: (selector: string) => selector === "[data-action]" ? buttons : controls,
    },
  });
  await new Promise(setImmediate);
  return { elements, buttons, calls, navigation, controls, fail: (message: string) => { failure = message; } };
}

for (const service of ["cliproxy", "newapi"] as const) {
  test(`${service}: opening is read-only; save secret clears it and reuses native storage`, async () => {
    const p = await panel(service);
    assert.deepEqual(p.calls.map((item) => item.payload.action), ["status"]);
    assert.equal(p.calls[0].route, `/local-integrations/${service}`);
    assert.equal(p.elements.get("apiKey")!.value, "");
    p.elements.get("apiKey")!.value = "test-secret";
    p.elements.get("credentials")!.onsubmit!({ preventDefault() {} });
    await new Promise(setImmediate);
    assert.deepEqual(p.calls.map((item) => item.payload.action), ["status", "saveKey", "status"]);
    assert.equal(p.calls[1].payload.request.apiKey, "test-secret");
    assert.equal(p.elements.get("apiKey")!.value, "");
    assert.ok(!p.elements.get("notice")!.textContent.includes("test-secret"));
    assert.ok(p.controls.every((item) => !item.disabled));
  });
  test(`${service}: models are text, apply is explicit, failure unlocks controls`, async () => {
    const p = await panel(service);
    await p.elements.get("refreshModels")!.onclick!();
    assert.equal(p.elements.get("model")!.children.length, 2);
    assert.equal(p.elements.get("model")!.children[1].textContent, "<img src=x>");
    assert.ok(!p.calls.some((item) => item.payload.action === "test" || item.payload.action === "apply"));
    await p.elements.get("apply")!.onclick!();
    const applied = p.calls.find((item) => item.payload.action === "apply")!;
    assert.equal(applied.payload.request.model, "safe-model");
    assert.deepEqual(Array.from(applied.payload.request.models as string[]), ["safe-model", "<img src=x>"]);
    p.fail("连接失败");
    await p.elements.get("testApi")!.onclick!();
    assert.equal(p.elements.get("notice")!.textContent, "连接失败");
    assert.ok(p.controls.every((item) => !item.disabled));
    p.elements.get("openPool")!.onclick!();
    assert.deepEqual(p.navigation, ["gatewayProviders"]);
  });
}
