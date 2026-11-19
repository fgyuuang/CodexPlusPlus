# 供应商池内的账号配额窗口

按用户最新要求保留 Codex++ 原有供应商池列表，不改为账号卡片，不新增独立侧栏页面。本轮仅修改源码、做定向验证；不启动发布任务，不编译或替换安装产物。

- ChatGPT 账号展开后的额度行增加剩余比例、进度条、准确重置日期和相对时间；保留原有额度刷新操作。
- 重置次数行按到期时间排列可用明细，保留获得时间悬停提示与既有确认、持久化请求标识流程；额度及重置操作始终可见，窄窗口中独立换行。
- 供应商列表下增加账号配额时间轴，跟随供应商搜索，支持两周／5 小时视图、前后翻页及回到今天；展示当前读数对应的窗口与主动重置到期标记。后续周期明确标记推算，不补造过去的历史读数。
- 剩余比例仅用于新鲜且未到重置时刻的官方读数。过期数据提示刷新，缺失的 Credit 状态不显示为零。日期沿用设备本地时区，时间轴明确显示时区名称。
- 使用拓展工具停用了已有 `quota-reset-overview` 独立入口。工具确认停用配置已保存，但旧脚本阻止安全热重载，当前页面未强制刷新；需要后续手动热重载或重启后核对入口消失。

验证：项目现有 Node 24 运行时执行 17 项定向前端测试，全部通过。TypeScript 最终复查通过（退出码 0）。中途曾因并发修改出现 `App.tsx:6356` 的 `onOpenLocalOfficialAccounts` 属性 TS2322；随后该接口由并发修改补齐，本次未修改相关 `App.tsx` / `GatewayPage.tsx` 代码，重新检查已通过。通过 Vite 隔离示例加载实际 `ProvidersView`，使用模拟数据与模拟 Tauri 调用核对展开详情、重置明细、按周／5 小时切换、深色主题及 420px 窄窗口；浏览器无页面异常，整页无横向溢出，时间轴局部可滚动。真实账号的服务端刷新与重置行为未在此预览中执行。

主要代码：`GatewayProviders.tsx`、`ProviderQuotaPanel.tsx`、`ProviderQuotaPanel.css`、`quota-timeline.ts`。定向测试：`provider-quota-panel.test.ts`、`quota-timeline.test.ts`、`reset-card-operation.test.ts`、`group-sticky-quota.test.ts`、`provider-pool-layout.test.ts`。

隔离预览证据位于 `output/quota-preview-20261117/`，不纳入发布产物。其他既有工作区修改保持原状，待用户后续统一编译。
