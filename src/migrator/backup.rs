//! 数据库备份
//!
//! 迁移前自动备份目标数据库，防止数据丢失。

use anyhow::Result;
use chrono::Utc;

/// 备份数据库文件到同目录 `backups/` 下
///
/// 文件名格式：`reina_manager_2025-08-20T07-47-19-178Z.db`
pub fn backup_database(db_path: &str) -> Result<()> {
    let actual_path = db_path.strip_prefix("sqlite:").unwrap_or(db_path);
    let db_file = std::path::Path::new(actual_path);

    if !db_file.exists() {
        println!("新数据库文件不存在，跳过备份");
        return Ok(());
    }

    let backup_dir = db_file.parent().unwrap().join("backups");
    std::fs::create_dir_all(&backup_dir)?;

    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ");
    let backup_path = backup_dir.join(format!("reina_manager_{}.db", timestamp));
    std::fs::copy(db_file, &backup_path)?;

    println!("已备份数据库到: {}", backup_path.display());
    Ok(())
}
