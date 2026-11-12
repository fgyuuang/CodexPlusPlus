import test from "node:test";
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";
import { build, install, validate } from "./manage.mjs";

const source = fileURLToPath(new URL("../assets/extensions/", import.meta.url));
const root = fileURLToPath(new URL("../output/local-plugin-tests/", import.meta.url));
fs.mkdirSync(root, { recursive: true });
const hash = (file) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
function fixture() {
  const base = fs.mkdtempSync(path.join(root, "case-"));
  return { base, packages: build(source, path.join(base, "packages")), config: path.join(base, "config") };
}
function newVersion(directory, version) {
  const manifestPath = path.join(directory, "codexpp.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.version = version;
  fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  const checksumPath = path.join(directory, "checksums.json");
  const checksums = JSON.parse(fs.readFileSync(checksumPath, "utf8"));
  checksums.files["codexpp.json"] = hash(manifestPath);
  fs.writeFileSync(checksumPath, JSON.stringify(checksums));
}

test("build produces two self-contained packages without changing product sources", () => {
  const input = path.join(source, "local-services/panel.js");
  const before = hash(input);
  const { base, packages } = fixture();
  assert.deepEqual(packages.map((item) => item.id), ["local-cliproxy", "local-newapi"]);
  for (const item of packages) {
    assert.equal(validate(item.directory).hostContract, "local-integrations-v1");
    const html = fs.readFileSync(path.join(item.directory, "manager.html"), "utf8");
    assert.ok(html.includes(`data-service="${item.id.replace("local-", "")}"`));
    assert.ok(!html.includes("/*__LOCAL_SERVICE_PANEL__*/"));
  }
  assert.equal(hash(input), before);
  assert.throws(() => build(source, path.join(base, "packages")), /拒绝覆盖/);
});

test("default install previews with zero writes and never changes enablement or accounts", () => {
  const { packages, config } = fixture();
  const preview = install(packages[0].directory, config);
  assert.equal(preview.applied, false);
  assert.ok(!fs.existsSync(config));
  const applied = install(packages[0].directory, config, true);
  assert.equal(applied.applied, true);
  assert.ok(!fs.existsSync(path.join(config, "user_scripts.json")));
  assert.ok(!fs.existsSync(path.join(config, "auth.json")));
  assert.equal(validate(applied.destination).version, "1.0.0");
});

test("update preserves old package outside discovery; rollback reinstalls that backup", () => {
  const { packages, config } = fixture();
  const first = install(packages[0].directory, config, true);
  const oldPageHash = hash(path.join(first.destination, "manager.html"));
  newVersion(packages[0].directory, "1.0.1");
  const update = install(packages[0].directory, config, true);
  assert.equal(validate(update.destination).version, "1.0.1");
  assert.equal(validate(update.backup).version, "1.0.0");
  assert.equal(hash(path.join(update.backup, "manager.html")), oldPageHash);
  assert.equal(path.relative(path.join(config, "user_scripts"), update.backup).startsWith(".."), true);
  const rollback = install(update.backup, config, true);
  assert.equal(validate(rollback.destination).version, "1.0.0");
  assert.equal(validate(rollback.backup).version, "1.0.1");
  assert.deepEqual(fs.readdirSync(path.join(config, "user_scripts")), ["local-cliproxy"]);
});

test("tampered, unknown, expanded-permission and unmanaged packages fail closed", () => {
  const { packages, config } = fixture();
  const cli = packages[0].directory;
  fs.appendFileSync(path.join(cli, "manager.html"), "tampered");
  assert.throws(() => install(cli, config, true), /完整性/);
  assert.ok(!fs.existsSync(config));
  const api = packages[1].directory;
  const manifestPath = path.join(api, "codexpp.json");
  const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
  manifest.permissions.routes.push("/settings/set");
  fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  assert.throws(() => install(api, config, true), /权限/);
  manifest.id = "../outside";
  fs.writeFileSync(manifestPath, JSON.stringify(manifest));
  assert.throws(() => validate(api), /未知包/);
});

test("unknown files and locally edited installs are preserved instead of overwritten", () => {
  const { packages, config } = fixture();
  const installed = install(packages[0].directory, config, true);
  fs.writeFileSync(path.join(installed.destination, "user-note.txt"), "keep");
  assert.throws(() => install(packages[0].directory, config, true), /文件集/);
  assert.equal(fs.readFileSync(path.join(installed.destination, "user-note.txt"), "utf8"), "keep");
});

test("junction config roots are rejected without writing through them", () => {
  const { base, packages } = fixture();
  const outside = path.join(base, "outside");
  fs.mkdirSync(outside);
  const linked = path.join(base, "linked");
  fs.symlinkSync(outside, linked, process.platform === "win32" ? "junction" : "dir");
  assert.throws(() => install(packages[0].directory, linked, true), /符号链接/);
  assert.deepEqual(fs.readdirSync(outside), []);
});

test("oversized manager pages are rejected before installing an unusable package", () => {
  const { packages, config } = fixture();
  fs.writeFileSync(path.join(packages[0].directory, "manager.html"), Buffer.alloc(1024 * 1024 + 1));
  assert.throws(() => install(packages[0].directory, config, true), /1 MiB/);
  assert.ok(!fs.existsSync(config));
});
