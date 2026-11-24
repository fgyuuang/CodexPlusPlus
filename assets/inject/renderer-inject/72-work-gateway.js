  const codexPlusWorkGatewayProject = 'form button[data-composer-navigation-target="workspace-project"]';
  const codexPlusWorkGatewayMarker = '[data-codex-plus-ext="work-gateway"]';
  const codexPlusWorkGatewayProjects = new WeakMap();

  function codexPlusWorkGatewayProps(node, matches) {
    const key = Object.keys(node || {}).find((name) => name.startsWith("__reactFiber$"));
    for (let fiber = node?.[key], depth = 0; fiber && depth < 60; fiber = fiber.return, depth += 1) {
      if (matches(fiber.memoizedProps)) return fiber.memoizedProps;
    }
    return null;
  }

  function codexPlusWorkGatewayDirectory(project) {
    const state = codexPlusWorkGatewayProps(project?.form || project?.closest?.("form"), (p) => !!p?.state?.runLocationState)?.state;
    const run = state?.runLocationState;
    if (!project?.isConnected || state?.isTemporaryChat || state?.composerProps?.conversationOrigin !== "tpp"
        || state.composerProps.workspaceAgent || state.composerProps.targetGizmoId
        || !["cloud", "local"].includes(run?.effectiveLocation)
        || run.unavailableReasons?.local != null
        || (run.effectiveLocation === "cloud" && run.unavailableReasons?.local !== null && run.canChooseLocation !== true)) return "";
    const props = codexPlusWorkGatewayProps(project, (p) => p?.isWorkHome === true && typeof p.isLocalWorkComposer === "boolean");
    return props?.hostId === "local" && props.isLocalWorkComposer === (run.effectiveLocation === "local") && typeof props.cwd === "string"
      && /^(?:[A-Za-z]:[\\/]|\/(?!\/))/.test(props.cwd) ? props.cwd : "";
  }

  function codexPlusWorkGatewayModels() {
    if (!codexGatewaySessionRoutingEnabled()) return [];
    const models = codexModelCatalog.models;
    const metadata = codexModelCatalog.modelMetadata;
    if (!Array.isArray(models)) return [];
    return models.filter((model) => {
      if (typeof model !== "string" || metadata?.[model]?.hidden === true) return false;
      // `/` also occurs in bare upstream model names. Only the catalog can attest a provider pin.
      const slash = model.indexOf("/");
      const providerPin = slash > 0 && slash < model.length - 1
        && metadata?.[model]?.codex_plus_group_id === model.slice(0, slash);
      return (/^(?:group|codex-plus-auto)\/\S/.test(model) || /^[^:/]+:\S/.test(model) || providerPin)
        && codexRemoteSessionTargetProvider(model);
    });
  }

  function codexPlusWorkGatewayMenuEntries() {
    return codexPlusWorkGatewayModels().map(model => {
      const metadata = codexModelCatalog.modelMetadata?.[model];
      const label = codexPlusModelDisplayName(metadata, model);
      const parsed = codexPlusModelPickerSplitLabel(label);
      return {
        model,
        label: parsed.name || label,
        group: metadata?.codex_plus_group || (model.startsWith("group/") ? "group" : model.startsWith("codex-plus-auto/") ? "auto" : "gateway"),
        favorite: metadata?.codex_plus_favorite === true,
      };
    }).sort((a, b) => Number(b.favorite) - Number(a.favorite));
  }

  async function codexPlusOpenWorkGatewayDraft(project, model) {
    const cwd = codexPlusWorkGatewayDirectory(project);
    const target = () => codexPlusWorkGatewayModels().includes(model) ? codexRemoteSessionTargetProvider(model) : "";
    const modelProvider = target();
    if (!cwd || !modelProvider) return false;
    const client = await codexPlusAppServerClient("local");
    const connection = await codexGatewayConnectionGeneration();
    if (!connection.active || codexPlusWorkGatewayDirectory(project) !== cwd || target() !== modelProvider) return false;
    // 只开本机空白对话，不转移 Work 的正文、附件、工具或权限设置。
    const result = await client.sendRequest("thread/start", { cwd, model, modelProvider }, { timeoutMs: 20000 });
    const id = result?.thread?.id;
    if (!/^[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}$/i.test(id || "") || result.model !== model || result.modelProvider !== modelProvider
        || codexPlusWorkGatewayDirectory(project) !== cwd || target() !== modelProvider) return false;
    window.dispatchEvent(new MessageEvent("message", { data: { type: "navigate-to-route", path: `/local/${id}` } }));
    return true;
  }

  function installCodexPlusWorkGateway() {
    const project = [...document.querySelectorAll(codexPlusWorkGatewayProject)]
      .find((button) => button.getClientRects().length && codexPlusWorkGatewayDirectory(button));
    const models = project && codexPlusWorkGatewayModels();
    const host = models?.length ? project.parentElement : null;
    for (const node of document.querySelectorAll(codexPlusWorkGatewayMarker)) {
      if (node.parentElement !== host || node.dataset.generation !== String(codexPlusRuntimeGeneration)
          || codexPlusWorkGatewayProjects.get(node) !== project) {
        node.querySelector("[popover]")?.hidePopover();
        node.remove();
      }
      else return;
    }
    if (!host || typeof HTMLElement.prototype.showPopover !== "function") return;
    const zh = /^zh\b/i.test(document.documentElement.lang || navigator.language);
    const label = zh ? "网关模型" : "Gateway models";
    const action = zh ? "在本机 Codex 中打开新对话" : "Open a new local Codex chat";
    const wrapper = document.createElement("span");
    wrapper.dataset.codexPlusExt = "work-gateway";
    wrapper.dataset.generation = String(codexPlusRuntimeGeneration);
    codexPlusWorkGatewayProjects.set(wrapper, project);
    const button = document.createElement("button");
    button.type = "button";
    button.className = project.className;
    button.textContent = label;
    button.title = action;
    button.setAttribute("aria-label", `${label}: ${action}`);
    button.setAttribute("aria-expanded", "false");
    button.setAttribute("aria-haspopup", "dialog");
    const panel = document.createElement("div");
    panel.className = "codex-plus-menu-popover";
    panel.setAttribute("popover", "auto");
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-label", button.getAttribute("aria-label"));
    Object.assign(panel.style, { position: "fixed", inset: "auto", margin: "0", border: "0", overflow: "hidden",
      width: "min(340px, calc(100vw - 16px))", maxHeight: "min(320px, 45vh)", display: "none" });
    button.popoverTargetElement = panel;
    const keyboard = (event) => {
      if (event.target?.classList?.contains("codex-plus-work-gateway-search")
          && panel.contains(event.target) && !["Tab", "Escape"].includes(event.key)) event.stopPropagation();
    };
    panel.addEventListener("beforetoggle", (event) => {
      const open = event.newState === "open";
      panel.style.display = open ? "flex" : "none";
      button.dataset.state = open ? "open" : "closed";
      button.setAttribute("aria-expanded", String(open));
      if (!open) {
        window.removeEventListener("keydown", keyboard, true);
        return;
      }
      window.addEventListener("keydown", keyboard, true);
      const rect = button.getBoundingClientRect();
      panel.style.left = `${Math.max(8, Math.min(rect.left, innerWidth - 348))}px`;
      const above = rect.top;
      const below = innerHeight - rect.bottom;
      const openAbove = above >= 270 || above >= below;
      panel.style.top = openAbove ? "auto" : `${rect.bottom + 4}px`;
      panel.style.bottom = openAbove ? `${innerHeight - rect.top + 4}px` : "auto";
      panel.style.maxHeight = `min(320px, ${Math.max(0, (openAbove ? above : below) - 12)}px)`;
      panel.replaceChildren();
      const search = document.createElement("input");
      search.type = "search";
      search.placeholder = zh ? "查找模型或供应商" : "Find a model or provider";
      search.setAttribute("aria-label", search.placeholder);
      search.className = "codex-plus-work-gateway-search";
      const list = document.createElement("div");
      list.className = "codex-plus-work-gateway-list";
      const groupLabels = { group: zh ? "模型组" : "Model groups", auto: zh ? "自动组" : "Automatic groups", gateway: label };
      const render = () => {
        const query = search.value.trim().toLocaleLowerCase();
        const groups = new Map();
        for (const entry of codexPlusWorkGatewayMenuEntries()) {
          const group = Object.hasOwn(groupLabels, entry.group) ? groupLabels[entry.group] : entry.group;
          if (!`${entry.label} ${entry.model} ${group}`.toLocaleLowerCase().includes(query)) continue;
          if (!groups.has(group)) groups.set(group, []);
          groups.get(group).push(entry);
        }
        list.replaceChildren();
        if (!groups.size) {
          const empty = document.createElement("p");
          empty.className = "codex-plus-work-gateway-heading";
          empty.textContent = zh ? "没有匹配的网关模型" : "No matching gateway models";
          list.append(empty);
        }
        for (const [group, entries] of groups) {
          const section = document.createElement("section");
          section.setAttribute("aria-label", group);
          const heading = document.createElement("div");
          heading.className = "codex-plus-work-gateway-heading";
          heading.textContent = group;
          section.append(heading);
          for (const entry of entries) section.append(makeItem(entry));
          list.append(section);
        }
      };
      function makeItem(entry) {
        const { model } = entry;
        const item = document.createElement("button");
        item.type = "button";
        item.className = "codex-plus-menu-item";
        const name = document.createElement("span");
        name.className = "codex-plus-menu-item-label";
        name.textContent = entry.label;
        item.title = model;
        item.append(name);
        item.addEventListener("click", async () => {
          if (button.disabled) return;
          panel.hidePopover();
          button.disabled = true;
          try {
            if (await codexPlusOpenWorkGatewayDraft(project, model)) return;
          } catch { /* 失败时不改派到云端。 */ }
          finally { button.disabled = false; }
          showToast(zh ? "未打开本机网关对话，请检查当前项目与网关连接。" : "Could not open a local gateway chat. Check the project and gateway connection.", null);
        });
        return item;
      }
      search.addEventListener("input", render);
      panel.append(search, list);
      render();
      requestAnimationFrame(() => { if (button.getAttribute("aria-expanded") === "true") search.focus({ preventScroll: true }); });
    });
    wrapper.append(button, panel);
    project.after(wrapper);
  }
