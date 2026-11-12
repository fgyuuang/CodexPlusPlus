//! WorkBuddy 的 CLI 入口可能是 Node 脚本，不能在 Windows 上直接当作 PE 启动。

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Context;
use tokio::process::Command;

pub(super) fn candidates() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = directories::BaseDirs::new() {
        let local = home.data_local_dir();
        for app in ["WorkBuddyCN", "WorkBuddy"] {
            paths.push(
                local
                    .join("Programs")
                    .join(app)
                    .join("resources/app.asar.unpacked/cli/bin/codebuddy"),
            );
        }
    }
    #[cfg(target_os = "macos")]
    for app in ["WorkBuddy.app", "WorkBuddy CN.app"] {
        paths.push(
            PathBuf::from("/Applications")
                .join(app)
                .join("Contents/Resources/app.asar.unpacked/cli/bin/codebuddy"),
        );
    }
    paths
}

pub(super) fn command(entry: &Path) -> anyhow::Result<Command> {
    anyhow::ensure!(
        !entry
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.eq_ignore_ascii_case("WorkBuddy.exe")
                    || name.eq_ignore_ascii_case("WorkBuddyCN.exe")
            }),
        "请选择 WorkBuddy 的 cli/bin/codebuddy 或原生 CLI，不要选择桌面主程序"
    );
    let mut prefix = Vec::new();
    std::fs::File::open(entry)?
        .take(128)
        .read_to_end(&mut prefix)?;
    let node_script = prefix.starts_with(b"#!/usr/bin/env node")
        || matches!(
            entry.extension().and_then(|ext| ext.to_str()),
            Some("js" | "cjs" | "mjs")
        );
    let mut command = if node_script {
        let node = super::executor::resolve_command("node").context(
            "WorkBuddy CLI 是 Node 脚本；请安装或将现有 Node 加入 PATH，不能直接填写桌面 EXE",
        )?;
        let mut command = Command::new(node);
        command.arg(entry);
        command
    } else {
        Command::new(entry)
    };
    #[cfg(windows)]
    command.creation_flags(crate::windows_integration::CREATE_NO_WINDOW);
    // 桌面发行包内置 lite-wb；只影响本任务进程，不改用户登录/模型/MCP 配置。
    command.env("CODEBUDDY_FORCE_LITE_WB_BUNDLE", "1");
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_entry_is_one_program_without_a_shell() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("codebuddy.exe");
        std::fs::write(&entry, b"MZ controlled fixture").unwrap();
        let command = command(&entry).unwrap();
        assert_eq!(command.as_std().get_program(), entry.as_os_str());
        assert_eq!(command.as_std().get_args().count(), 0);
    }

    #[test]
    fn script_entry_stays_a_single_argument_and_desktop_host_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("codebuddy script with spaces");
        std::fs::write(&entry, b"#!/usr/bin/env node\n// fixture only").unwrap();
        match command(&entry) {
            Ok(command) => assert_eq!(
                command.as_std().get_args().collect::<Vec<_>>(),
                [entry.as_os_str()]
            ),
            Err(error) => assert!(error.to_string().contains("Node")),
        }
        assert!(
            command(&temp.path().join("WorkBuddy.exe"))
                .unwrap_err()
                .to_string()
                .contains("桌面主程序")
        );
    }
}
