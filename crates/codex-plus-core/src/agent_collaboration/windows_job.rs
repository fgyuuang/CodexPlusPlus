//! 仅包含本次 WorkBuddy CLI 的 Windows Job；关闭句柄回收其全部工具后代。

use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

use anyhow::Context;
use tokio::process::Child;

#[repr(C)]
#[derive(Default)]
struct BasicLimits {
    per_process_user_time: i64,
    per_job_user_time: i64,
    flags: u32,
    minimum_working_set: usize,
    maximum_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[repr(C)]
#[derive(Default)]
struct ExtendedLimits {
    basic: BasicLimits,
    io_counters: [u64; 6],
    process_memory: usize,
    job_memory: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
    fn SetInformationJobObject(job: *mut c_void, class: i32, info: *const c_void, size: u32)
    -> i32;
    fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
}

/// OwnedHandle 可随 worker 移动；没有命名 Job、共享句柄或按 PID 再打开进程。
pub(super) struct OwnedJob {
    _handle: OwnedHandle,
}

impl OwnedJob {
    pub(super) fn assign(child: &Child) -> anyhow::Result<Self> {
        let process = child.raw_handle().context("CLI 已退出，无法绑定任务进程")?;
        // SAFETY: 无名称、无继承属性，Job 仅由当前 worker 持有。
        let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        anyhow::ensure!(!raw.is_null(), std::io::Error::last_os_error());
        // SAFETY: CreateJobObjectW 返回的有效句柄转移至唯一 OwnedHandle。
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let limits = ExtendedLimits {
            basic: BasicLimits {
                flags: 0x2000, // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE；不允许 breakaway。
                ..BasicLimits::default()
            },
            ..ExtendedLimits::default()
        };
        // SAFETY: repr(C) 结构与 JOBOBJECT_EXTENDED_LIMIT_INFORMATION 对齐，缓冲区有效。
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_raw_handle(),
                9, // JobObjectExtendedLimitInformation
                (&limits as *const ExtendedLimits).cast(),
                std::mem::size_of::<ExtendedLimits>() as u32,
            )
        };
        anyhow::ensure!(configured != 0, std::io::Error::last_os_error());
        // SAFETY: process 来自仍由本次调用持有的 Child，不通过可复用 PID 选取进程。
        let assigned = unsafe { AssignProcessToJobObject(handle.as_raw_handle(), process) };
        anyhow::ensure!(assigned != 0, std::io::Error::last_os_error());
        Ok(Self { _handle: handle })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead;
    use std::process::Stdio;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::process::Command;
    use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
    };

    const MODE: &str = "CODEX_PLUS_WORKBUDDY_JOB_TEST_MODE";
    const PID_FILE: &str = "CODEX_PLUS_WORKBUDDY_JOB_TEST_PID_FILE";
    const HELPER: &str = "agent_collaboration::windows_job::tests::controlled_child_process";

    fn command(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", HELPER, "--nocapture", "--test-threads=1"]);
        command.env(MODE, mode);
        command.creation_flags(crate::windows_integration::CREATE_NO_WINDOW);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        command
    }

    #[test]
    fn controlled_child_process() {
        let Ok(mode) = std::env::var(MODE) else {
            return;
        };
        if mode == "parent" {
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line).unwrap();
            assert_eq!(line, "start\n");
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            use std::os::windows::process::CommandExt;
            command
                .args(["--exact", HELPER, "--nocapture", "--test-threads=1"])
                .env(MODE, "leaf")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(crate::windows_integration::CREATE_NO_WINDOW);
            let mut child = command.spawn().unwrap();
            std::fs::write(std::env::var_os(PID_FILE).unwrap(), child.id().to_string()).unwrap();
            let _ = child.wait();
        } else {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    #[tokio::test]
    async fn closing_owned_job_reaps_parent_and_grandchild_without_touching_independent_process() {
        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("grandchild.pid");
        let mut independent = command("leaf").spawn().unwrap();
        let mut parent = command("parent").env(PID_FILE, &pid_file).spawn().unwrap();
        let job = OwnedJob::assign(&parent).unwrap();
        parent
            .stdin
            .take()
            .unwrap()
            .write_all(b"start\n")
            .await
            .unwrap();
        let pid = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(text) = std::fs::read_to_string(&pid_file)
                    && let Ok(pid) = text.parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        // 在关闭 Job 前取得真实后代句柄，后续不按可能复用的 PID 判断生死。
        let grandchild = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.unwrap();
        drop(job);
        tokio::time::timeout(Duration::from_secs(10), parent.wait())
            .await
            .unwrap()
            .unwrap();
        let result = unsafe { WaitForSingleObject(grandchild, 10_000) };
        let _ = unsafe { CloseHandle(grandchild) };
        assert_eq!(result, WAIT_OBJECT_0);
        assert!(independent.try_wait().unwrap().is_none());
        independent.kill().await.unwrap();
    }
}
