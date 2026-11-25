use std::fs::OpenOptions;
use std::path::Path;

use anyhow::Context;
use fs2::FileExt;

const FILE_NAME: &str = "codex-plus-gateway-connection-generation";

pub fn read(home: &Path) -> anyhow::Result<String> {
    Ok(crate::gateway::restore::read_optional(&home.join(FILE_NAME))?.unwrap_or_default())
}

/// 同一进程内的串行化：fs2 的 advisory lock 在 Windows 上按进程记账，
/// 同一进程的多个线程同时 lock 不会互相阻塞，只靠它会并发写同一个 `.tmp`。
static PUBLISH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn publish(home: &Path) -> anyhow::Result<String> {
    let _process_guard = PUBLISH_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    std::fs::create_dir_all(home)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join(format!("{FILE_NAME}.lock")))?;
    lock.lock_exclusive()
        .context("获取本地服务连接代际写锁失败")?;
    let generation = uuid::Uuid::new_v4().to_string();
    // Windows 上刚写完的临时文件可能被杀毒/索引短暂占用，替换会拿到 ERROR_ACCESS_DENIED；
    // 代次本身是可重试的（只换一个 uuid），这里做几次短退避重试，避免让连接代次发布失败。
    let target = home.join(FILE_NAME);
    let mut last_error = None;
    for attempt in 0..5 {
        match crate::settings::atomic_write(&target, generation.as_bytes()) {
            Ok(()) => {
                last_error = None;
                break;
            }
            Err(error) => {
                last_error = Some(error);
                std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1)));
            }
        }
    }
    FileExt::unlock(&lock)?;
    if let Some(error) = last_error {
        return Err(error.context("写入网关连接代次失败"));
    }
    Ok(generation)
}
