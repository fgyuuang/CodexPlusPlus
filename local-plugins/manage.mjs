// 独立插件打包/安装工具。仅 Node 内置库；不操作账号、服务或拓展启用配置。
import * as fs from "node:fs";
import * as path from "node:path";
import { createHash, randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";

const ids = ["local-cliproxy", "local-newapi"];
const files = ["codexpp.json", "manager.html", "README.md"];
const checksumFile = "checksums.json";
const hash = (value) => createHash("sha256").update(value).digest("hex");
function plain(target) {
  let cursor = path.resolve(target);
  for (;;) {
    if (fs.lstatSync(cursor, { throwIfNoEntry: false })?.isSymbolicLink()) throw new Error("拒绝符号链接或目录联接");
    const parent = path.dirname(cursor);
    if (parent === cursor) break;
    cursor = parent;
  }
}
function child(root, name) {
  const result = path.resolve(root, name);
  const relative = path.relative(path.resolve(root), result);
  if (!relative || relative.startsWith("..") || path.isAbsolute(relative)) throw new Error("目标越出允许目录");
  plain(result);
  return result;
}
function read(root, name) {
  const target = child(root, name);
  if (!fs.lstatSync(target).isFile()) throw new Error("包内必须为普通文件");
  return fs.readFileSync(target);
}
export function validate(directory) {
  plain(directory);
  const names = fs.readdirSync(directory).sort();
  if (JSON.stringify(names) !== JSON.stringify([...files, checksumFile].sort())) throw new Error("包文件集不符合契约");
  const manifest = JSON.parse(read(directory, "codexpp.json"));
  const service = manifest.id?.replace("local-", "");
  if (!ids.includes(manifest.id) || !/^\d+\.\d+\.\d+$/.test(manifest.version)) throw new Error("未知包或版本");
  const allowed = ["id", "name", "version", "description", "engines", "manager", "permissions"];
  if (Object.keys(manifest).some((key) => !allowed.includes(key))) throw new Error("不允许新增执行能力");
  const manager = manifest.manager;
  if (!manager || manager.page !== "manager.html" || manager.nav !== "tools"
      || Object.keys(manager).some((key) => !["page", "title", "nav", "icon"].includes(key))) throw new Error("管理面板契约不符");
  if (JSON.stringify(manifest.permissions) !== JSON.stringify({ routes: [`/local-integrations/${service}`] })) throw new Error("服务权限不符");
  if (read(directory, "manager.html").length > 1024 * 1024) throw new Error("管理面板超过宿主 1 MiB 上限");
  const checksums = JSON.parse(read(directory, checksumFile));
  if (checksums.hostContract !== "local-integrations-v1" || Object.keys(checksums.files || {}).sort().join() !== [...files].sort().join()) throw new Error("缺少宿主契约或完整哈希");
  for (const name of files) if (hash(read(directory, name)) !== checksums.files[name]) throw new Error(`包完整性检查失败：${name}`);
  return { id: manifest.id, version: manifest.version, hostContract: checksums.hostContract };
}

export function build(source, output) {
  source = path.resolve(source); output = path.resolve(output);
  plain(source); plain(output);
  if (fs.existsSync(output)) throw new Error("输出目录已存在，拒绝覆盖");
  const template = read(path.join(source, "local-services"), "panel.html").toString("utf8");
  const script = read(path.join(source, "local-services"), "panel.js").toString("utf8");
  const contents = ids.map((id) => ({ id, entries: {
    "codexpp.json": read(path.join(source, id), "codexpp.json"),
    "README.md": read(path.join(source, id), "README.md"),
    "manager.html": Buffer.from(template.replaceAll("__LOCAL_SERVICE__", id.replace("local-", "")).replace("/*__LOCAL_SERVICE_PANEL__*/", () => script)),
  } }));
  fs.mkdirSync(output, { recursive: true });
  const packages = [];
  for (const { id, entries } of contents) {
    const directory = child(output, id);
    fs.mkdirSync(directory);
    const hashes = {};
    for (const name of files) {
      fs.writeFileSync(child(directory, name), entries[name], { flag: "wx" });
      hashes[name] = hash(entries[name]);
    }
    fs.writeFileSync(child(directory, checksumFile), JSON.stringify({ hostContract: "local-integrations-v1", files: hashes }, null, 2), { flag: "wx" });
    packages.push({ ...validate(directory), directory });
  }
  return packages;
}

export function install(packageDirectory, configDirectory, apply = false) {
  const info = validate(packageDirectory);
  const config = path.resolve(configDirectory);
  plain(config);
  const destination = child(config, `user_scripts/${info.id}`);
  const existing = fs.existsSync(destination);
  if (existing) validate(destination); // 已修改或非本工具管理的目录不自动覆盖。
  const result = { ...info, destination, existing, applied: false, backup: null };
  if (!apply) return result;
  const token = `${info.id}-${Date.now()}-${randomUUID()}`;
  const staging = child(config, `local-plugin-staging/${token}`);
  const backup = child(config, `local-plugin-backups/${token}`);
  fs.mkdirSync(staging, { recursive: true });
  for (const name of [...files, checksumFile]) fs.copyFileSync(child(packageDirectory, name), child(staging, name), fs.constants.COPYFILE_EXCL);
  validate(staging);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  if (existing) {
    fs.mkdirSync(path.dirname(backup), { recursive: true });
    plain(destination); plain(backup);
    fs.renameSync(destination, backup);
    result.backup = backup;
  }
  try {
    plain(staging); plain(destination);
    fs.renameSync(staging, destination);
  } catch (error) {
    if (existing && !fs.existsSync(destination)) fs.renameSync(backup, destination);
    throw error;
  }
  result.applied = true;
  return result;
}

function main(args) {
  const action = args.shift();
  const options = new Map();
  let apply = false;
  while (args.length) {
    const name = args.shift();
    if (name === "--apply") { apply = true; continue; }
    if (!["--source", "--output", "--package", "--config-dir"].includes(name) || !args[0] || args[0].startsWith("--")) throw new Error("未知或缺少参数");
    options.set(name, args.shift());
  }
  if (action === "build" && options.has("--output")) {
    const source = options.get("--source") || fileURLToPath(new URL("../assets/extensions/", import.meta.url));
    return build(source, options.get("--output"));
  }
  if (action === "install" && options.has("--package") && options.has("--config-dir")) return install(options.get("--package"), options.get("--config-dir"), apply);
  if (action === "verify" && options.has("--package")) return validate(options.get("--package"));
  throw new Error("用法：build --output DIR [--source DIR] | verify --package DIR | install --package DIR --config-dir DIR [--apply]；回滚使用备份目录作为 package。");
}
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { console.log(JSON.stringify(main(process.argv.slice(2)), null, 2)); }
  catch (error) { console.error(error.message); process.exitCode = 1; }
}
