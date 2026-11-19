# 2026-11-17 页面启动延迟检查与源码修复

用户确认慢点为窗口出现后的页面、聊天列表加载。本轮只做诊断、源码修复与定向验证，不构建或交付产品 EXE，不重启用户应用。

## 实机证据

时间均为北京时间。读取本地 Codex++ 诊断日志及官方桌面日志，只保留事件、耗时和进程类型，不保存对话、认证信息或请求正文。

- 01:07:39.874 初始主窗口 ready-to-show；01:07:40.055 app-server 初始化成功，握手 396 ms；宿主关键启动阶段 1971 ms。
- 后续主窗口 01:07:47.498 ready-to-show，01:08:06.791 才记录 app routes mounted，官方记录耗时 19182 ms。从初始窗口出现到这时约 27 秒，符合窗口先出现、页面后就绪的反馈。
- 同一次运行 thread/list 最大 58 ms，thread/read 最大 16 ms，config/read 最大 252 ms。已有记录不支持把主要延迟归咎于聊天数据库查询。
- 同次启动的 plugin/installed 一次 4580 ms；整段运行的 app/read 最大 10786 ms、mcpServerStatus/list 最大 51200 ms。这些是额外的加载延迟线索，不能将工具初始化慢等同于页面启动慢。
- 每个桥接代际出现两条几乎同时的 capture_armed：01:07:46.913 / 46.960，01:08:11.171 / 11.223。01:12:24.527 一套捕获成功，01:12:24.563 另一套恢复暂停失败：Can only perform operation while paused。
- 源码确认自动捕获没有经过 ON_DEMAND_CAPTURE_GENERATIONS，而拓展按需捕获只对自身去重，导致同一页面同时运行两套 Debugger 会话。它们重复枚举大型脚本并竞争暂停恢复，是已确认的性能及正确性缺陷。现有日志不能把全部 19 秒准确归因于这一项。
- 当前若干 Manager 子进程带 --codex-plus-extension-mcp，是工具服务，不将它们当成多份 GUI 随意结束。
- 对当前已运行页面做只读 CDP 指标与短时 CPU 采样；这是启动后的采样，不充当新启动性能对照。未记录页面文字或存储内容，未启用旧快速启动开关、改写 Statsig 或强制刷新。

## 修复

仅修改 crates/codex-plus-core/src/bridge.rs：

1. 自动捕获与拓展按需捕获共用代际登记，同一代际最多开启一套捕获。
2. 检查当前代际和登记使用一致的锁顺序，拒绝释放后迟到的旧请求。
3. 代际结束时回收登记；页面重新注入及其他页面仍可各自正常捕获。
4. 自动捕获未启用时不占登记，保留拓展按需启动入口。

保留原有暂停归属校验、恢复与清理流程。未调整认证、选路、模型目录、会话内容或官方安装包。

## 验证与边界

- 新增 3 个回归用例：自动与按需去重、8 路并发只有一个成功登记、代际替换/独立页面/旧请求拒绝与回收。
- cargo test -p codex-plus-core --lib app_server_capture_tests -- --test-threads=1：21 通过。
- cargo test -p codex-plus-core --lib bridge:: -- --test-threads=1：63 通过、1 忽略、0 失败。包含原有用户断点保护、捕获失败恢复、超时、桥接代际及请求调度相关测试。
- rustfmt --edition 2024 --check 及该文件 git diff --check 通过。
- 测试日志：output/startup-diagnostics-20261117/bridge-tests.log。只读采样脚本：同目录 probe.mjs。

源码修复尚未进入运行中的 EXE，因此没有宣称实机启动已提速、或已解决全部页面等待。后续实际运行新版本时应核对同代际 capture_armed 是否只剩一条，并对比 ready-to-show → app routes mounted 的耗时；若仍慢，再沿原生插件清单与应用初始化耗时继续定位。独立的重启注入等待问题保留原证据，不混作本次页面问题已解决。
