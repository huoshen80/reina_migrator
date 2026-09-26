//! 将每次迁移的诊断信息写入独立日志文件。

use anyhow::{Context, Result};
use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 在程序所在目录创建本次运行的日志，并初始化文件日志订阅器。
pub fn initialize() -> Result<PathBuf> {
    let executable = std::env::current_exe().context("无法定位迁移器程序文件")?;
    let directory = executable.parent().context("无法获取迁移器程序所在目录")?;
    let (path, file) = create_log_file(directory)?;

    tracing_subscriber::fmt()
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .with_target(false)
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .map_err(|error| anyhow::anyhow!("无法初始化文件日志: {error}"))?;
    Ok(path)
}

fn create_log_file(directory: &Path) -> Result<(PathBuf, File)> {
    fs::create_dir_all(directory)
        .with_context(|| format!("无法创建日志目录: {}", directory.display()))?;

    let timestamp = Local::now().format("%Y%m%d_%H%M%S_%3f");
    let path = directory.join(format!(
        "reina_migrator_{timestamp}_{}.log",
        std::process::id()
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("无法创建日志文件: {}", path.display()))?;
    Ok((path, file))
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        println!("{message}");
        tracing::info!("{message}");
    }};
}

#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => {{
        let message = format!($($arg)*);
        eprintln!("{message}");
        tracing::warn!("{message}");
    }};
}

#[cfg(test)]
mod tests {
    use super::create_log_file;
    use std::fs;
    use std::sync::Mutex;

    #[test]
    fn records_events_in_a_per_run_file() {
        let directory =
            std::env::temp_dir().join(format!("reina-migrator-log-test-{}", uuid::Uuid::new_v4()));
        let (path, file) = create_log_file(&directory).unwrap();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(Mutex::new(file))
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("测试迁移开始");
            tracing::warn!("测试跳过原因");
        });

        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains("测试迁移开始"));
        assert!(contents.contains("测试跳过原因"));
        fs::remove_dir_all(directory).unwrap();
    }
}
