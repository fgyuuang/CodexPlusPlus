// 注入页已经由 /model-catalog 取得当前账号的目录；复用它，避免再嵌入静态模型指令与旧目录。
export function visibleOfficialModelSlugs() {
  return typeof codexModelCatalog === "undefined" ? [] : [...(codexModelCatalog.officialModels || [])];
}
