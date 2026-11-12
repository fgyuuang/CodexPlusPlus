# 独立本地插件包

这套工具把 CLIProxyAPI / NewAPI 面板生成可独立安装的用户拓展包。仅使用 Node 内置库，不安装依赖，不运行 Cargo/Vite，不改登录、供应商或拓展启用配置，不启动服务或应用。

## 打包与检查

在仓库根目录用 Node 24 执行；输出目录必须不存在，避免覆盖上次产物：

```powershell
node .\local-plugins\manage.mjs build --output .\output\local-plugins-v1
node .\local-plugins\manage.mjs verify --package .\output\local-plugins-v1\local-cliproxy
node --test .\local-plugins\manage.test.mjs
```

每个包包含 codexpp.json、manager.html、README.md 和 checksums.json。页面脚本已内联，安装后不依赖仓库路径。SHA256 用于完整性核对，不等于发布者签名；只安装可信来源。

当前默认从 assets/extensions 读取初始源码。需要单独维护时，可把 local-cliproxy、local-newapi、local-services 这三个源码目录放入自己的插件仓库，使用 `build --source <插件源码根目录> --output <新产物目录>`。界面及操作编排更新无需重编译主程序；新增宿主原生能力仍需主程序更新。bundled 中的初始副本暂留为兼容回退。

## 安装、更新与回滚

必须先成功安装并启动带 `local-integrations-v1` 适配的本 fork。单纯上游 v2.2.0 的版本号不能保证具备该能力；工具不会通过任意命令绕过缺失的宿主接口。

默认只预览；确认目标配置目录后加 `--apply`，安装路径为该目录下的 user_scripts/<包 id>：

```powershell
$pluginConfig = Join-Path $env:APPDATA 'Codex++'
node .\local-plugins\manage.mjs install --package .\output\local-plugins-v1\local-cliproxy --config-dir $pluginConfig
node .\local-plugins\manage.mjs install --package .\output\local-plugins-v1\local-cliproxy --config-dir $pluginConfig --apply
```

更新使用同一命令。旧包完整保存在 local-plugin-backups 目录，不会被当成另一插件扫描；输出中的 backup 即回滚来源。回滚时以该备份目录作为 `--package` 再安装，当前版本也会先备份。工具不删除文件，失败残留的 staging 保留供检查。未知文件、手工改动导致哈希不匹配、符号链接/目录联接均拒绝覆盖。

安装后在管理器刷新拓展列表，打开来源为「本地」的用户包面板。当前仍有两个兼容边界：连接页按钮和 CLI 自启动绑定 bundled key。工具不会自动停用 bundled 副本或改 user_scripts.json；不要把用户包已安装当作默认入口及自启动迁移完成。可手动停用重复面板，但停用 bundled CLI 时现有自启动也会被阻止，手动启动仍可从用户包操作。

首次宿主发布仍需完整编译；后续保持这两项服务权限契约不变时，插件包可独立更新。安装及回滚均保留现有 DPAPI 密钥、外部账户文件、受管供应商 ID 和历史。
