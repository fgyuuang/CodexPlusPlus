/* 管理器专用沙箱页面；宿主按当前包清单重新授权每次调用。 */
(function () {
  "use strict";
  const api = window.codexPlusManager;
  const service = document.body.dataset.service;
  const cli = service === "cliproxy";
  const route = "/local-integrations/" + service;
  const byId = (id) => document.getElementById(id);
  const fields = cli ? [
    ["installRoot", "安装目录"], ["binaryPath", "程序路径（可留空）"],
    ["configPath", "配置路径（可留空）"], ["baseUrl", "API 地址"],
  ] : [
    ["projectRoot", "Compose 项目目录"], ["composeFile", "Compose 文件（可留空）"],
    ["dockerExecutable", "Docker 程序（可留空）"], ["apiServiceName", "API 服务名（可留空）"],
    ["baseUrl", "API 地址（可为远程服务）"],
  ];
  let models = [];
  let busy = false;
  byId("title").textContent = cli ? "CLIProxyAPI 本地服务" : "NewAPI 本地服务";
  byId("apiKey").required = !cli;
  for (const id of cli ? ["cliActions", "managementKeyLabel", "channelLabel", "disableOfficial"] : ["newapiActions"]) byId(id).hidden = false;
  if (cli) byId("modelHint").textContent = "通用与官方通道使用独立供应商，不加入聚合轮转。官方通道仅接受宿主官方目录确认的模型。";
  for (const [name, title] of fields) {
    const label = document.createElement("label");
    const input = document.createElement("input");
    label.textContent = title;
    input.id = name;
    input.required = name === "baseUrl" || name === (cli ? "installRoot" : "projectRoot");
    label.appendChild(input);
    byId("connectionFields").appendChild(label);
  }
  async function call(action, request = {}) {
    const result = await api.call(route, { action, request });
    if (!result || result.status !== "ok") throw new Error(result?.message || "操作未完成");
    return result;
  }
  function renderStatus(result) {
    byId("status").textContent = result.healthy ? "运行正常" : result.running ? "进程运行中，API 尚未就绪" : "服务未运行或尚未配置";
    byId("details").textContent = [result.version, result.baseUrl,
      cli ? (result.managedProcess ? "当前由 Codex++ 管理" : "独立服务") : `Compose 服务 ${result.runningServiceCount ?? 0}/${result.serviceCount ?? 0}`,
      result.profileInstalled ? "已有供应商接入" : "尚未接入供应商"].filter(Boolean).join(" · ");
    for (const [name] of fields) byId(name).value = result[name] ?? "";
    byId("autoStart").checked = !!result.autoStart;
    byId("keyState").textContent = (result.hasApiKey ? "API 凭据已保存；" : "尚未保存 API 凭据；") +
      (cli && result.hasManagementKey ? "管理密钥已保存。" : "密钥仅保存在宿主加密存储中。");
  }
  async function status() { renderStatus(await call("status")); }
  async function refreshModels() {
    const result = await call("models");
    const selected = byId("model").value;
    models = [...new Set((result.models || []).filter((item) => typeof item === "string" && item.trim()))];
    byId("model").replaceChildren();
    for (const model of models) {
      const option = document.createElement("option");
      option.value = model;
      option.textContent = model;
      byId("model").appendChild(option);
    }
    if (models.includes(selected)) byId("model").value = selected;
    return result;
  }
  async function run(operation) {
    if (busy) return;
    busy = true;
    const controls = [...document.querySelectorAll("button,input,select")];
    for (const control of controls) control.disabled = true;
    byId("notice").textContent = "正在处理…";
    try {
      const result = await operation();
      byId("notice").textContent = result?.message || "已完成";
    } catch (error) {
      byId("notice").textContent = error instanceof Error ? error.message : "操作失败";
    } finally {
      busy = false;
      for (const control of controls) control.disabled = false;
    }
  }
  for (const button of document.querySelectorAll("[data-action]")) button.onclick = () => run(async () => {
    const action = button.dataset.action;
    const result = await call(action);
    if (["status", "start", "stop", "restart", "install", "disable", "disableOfficial"].includes(action)) await status();
    return result;
  });
  byId("connection").onsubmit = (event) => {
    event.preventDefault();
    void run(async () => {
      const request = Object.fromEntries(fields.map(([name]) => [name, byId(name).value.trim()]));
      const result = await call("saveConnection", request);
      models = [];
      byId("model").replaceChildren();
      await status();
      return result;
    });
  };
  byId("credentials").onsubmit = (event) => {
    event.preventDefault();
    void run(async () => {
      const result = await call("saveKey", { apiKey: byId("apiKey").value, ...(cli ? { managementKey: byId("managementKey").value } : {}) });
      byId("apiKey").value = "";
      byId("managementKey").value = "";
      models = [];
      byId("model").replaceChildren();
      await status();
      return result;
    });
  };
  byId("saveAutoStart").onclick = () => run(async () => {
    const result = await call("autoStart", { enabled: byId("autoStart").checked });
    await status();
    return result;
  });
  byId("refreshModels").onclick = () => run(refreshModels);
  byId("testApi").onclick = () => run(() => call("test", { model: byId("model").value }));
  byId("apply").onclick = () => run(async () => {
    if (!models.length) await refreshModels();
    if (!models.length) throw new Error("服务没有可用模型，请检查 API 地址和凭据。");
    const result = await call("apply", { model: byId("model").value, models, ...(cli ? { channel: byId("channel").value } : {}) });
    await status();
    return result;
  });
  byId("openPool").onclick = () => api.navigate("gatewayProviders");
  void run(status);
})();
