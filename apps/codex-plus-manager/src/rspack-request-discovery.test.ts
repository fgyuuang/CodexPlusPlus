import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = await readFile(new URL("../../../assets/inject/renderer-inject/20-menu.js", import.meta.url), "utf8");
const start = source.indexOf("  function codexRspackRequestModules(");
const end = source.indexOf("  async function loadAppServerRequestCandidates(", start);
assert.ok(start >= 0 && end > start);
const helper = source.slice(start, source.indexOf("  async function loadAppServerRequestModules(", start));
const collect = new Function(`${helper}\nreturn codexRspackRequestModules;`)();

class Broker {
  static getInstance() { throw new Error("discovery must not instantiate a transport"); }
  dispatchMessage() {}
  subscribe() {}
}
class Http {
  static getInstance() { throw new Error("discovery must not instantiate a transport"); }
  sendRequest() { return "vscode://codex/thread/resume"; }
}

test("Rspack discovery returns initialized request transports without executing factories or instances", () => {
  let executions = 0;
  const broker = { Broker };
  const modules = collect({ c: { one: { exports: broker }, duplicate: { exports: broker }, two: { exports: { Http } },
    unrelated: { exports: { fake: () => "mcp-request" } } }, m: { unloaded: () => { executions++; } } });
  assert.equal(modules.length, 2);
  assert.equal(modules[0].Broker, Broker);
  assert.equal(modules[1].Http, Http);
  assert.equal(executions, 0);
  assert.deepEqual(collect(undefined), []);
});

test("incomplete lazy exports cannot hide another already initialized transport", () => {
  const exports = Object.defineProperty({ Broker }, "pending", { enumerable: true, get() { throw new Error("not ready"); } });
  const modules = collect({ c: { incomplete: { exports }, another: { exports: { Http } } } });
  assert.equal(modules.length, 2);
  assert.deepEqual(Object.keys(modules[0]), ["Broker"]);
});

test("numeric entry discovery restores both provider transports and only imports this document's own scripts", async () => {
  const imported: string[] = [];
  const scripts = ["app://-/assets/348410.hash.js", "https://foreign.test/assets/123.hash.js"];
  const code = source.slice(start, end).replaceAll("await import(url)", "await importer(url)");
  const load = new Function("document", "location", "URL", "importer", `${code}
    const loadOptionalCodexAppModule = async () => null;
    const appServerFallbackAssetUrls = () => [];
    const uniqueCodexAppAssetUrls = (urls) => [...new Set(urls)];
    return loadAppServerRequestModules;
  `)({ scripts: scripts.map(src => ({ src })) }, { href: "app://-/index.html", origin: "null" }, URL,
    async (url: string) => { imported.push(url); return { __webpack_require__: { c: {
      broker: { exports: { Broker } }, http: { exports: { Http } },
    } } }; });
  const result = await load();
  assert.deepEqual(imported, [scripts[0]]);
  assert.equal(result.modules.length, 2);
  assert.deepEqual(result.sources, [scripts[0], scripts[0]]);
});
