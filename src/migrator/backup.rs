//! 数据库备份
//!
//! 迁移前使用 SQLite 一致性快照备份目标数据库，防止数据丢失。

use anyhow::{Context, Result};
use chrono::Local;
use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};
use std::fs;
use std::path::{Path, PathBuf};

/// 使用 `VACUUM INTO` 创建并校验目标数据库快照。
///
/// 优先使用 `user.id = 1` 的 `db_backup_path`；未配置、配置无效或自定义
/// 目录备份失败时，回退到数据库同级的 `backups/` 目录。
pub async fn backup_database(db: &DatabaseConnection, db_url: &str) -> Result<PathBuf> {
    let db_file = database_file_from_url(db_url)?;
    verify_integrity(db, "目标数据库").await?;

    let default_dir = db_file
        .parent()
        .context("无法获取目标数据库所在目录")?
        .join("backups");
    let custom_dir = read_custom_backup_dir(db).await;

    let backup_path = create_backup_with_fallback(db, custom_dir.as_deref(), &default_dir).await?;
    println!("已备份数据库到: {}", backup_path.display());
    Ok(backup_path)
}

fn database_file_from_url(db_url: &str) -> Result<PathBuf> {
    let path = db_url
        .strip_prefix("sqlite:")
        .context("目标数据库 URL 必须使用 sqlite: 前缀")?;
    Ok(PathBuf::from(path))
}

async fn read_custom_backup_dir(db: &DatabaseConnection) -> Option<PathBuf> {
    let statement = Statement::from_string(
        DatabaseBackend::Sqlite,
        "SELECT db_backup_path FROM user WHERE id = 1 LIMIT 1".to_string(),
    );
    let row = match db.query_one(statement).await {
        Ok(row) => row,
        Err(error) => {
            eprintln!("无法读取数据库备份目录设置，将使用默认目录：{error}");
            return None;
        }
    }?;

    let custom_path = match row.try_get::<Option<String>>("", "db_backup_path") {
        Ok(path) => path,
        Err(error) => {
            eprintln!("无法解析数据库备份目录设置，将使用默认目录：{error}");
            return None;
        }
    }?;
    let custom_path = custom_path.trim();
    if custom_path.is_empty() {
        return None;
    }

    let custom_dir = PathBuf::from(custom_path);
    if custom_dir.is_dir() {
        Some(custom_dir)
    } else {
        eprintln!(
            "自定义数据库备份目录无效，将使用默认目录：{}",
            custom_dir.display()
        );
        None
    }
}

async fn create_backup_with_fallback(
    db: &DatabaseConnection,
    custom_dir: Option<&Path>,
    default_dir: &Path,
) -> Result<PathBuf> {
    if let Some(custom_dir) = custom_dir {
        match create_verified_backup(db, custom_dir).await {
            Ok(backup_path) => return Ok(backup_path),
            Err(error) => eprintln!(
                "无法备份到自定义目录 {}，将回退到默认目录：{error}",
                custom_dir.display()
            ),
        }
    }

    create_verified_backup(db, default_dir).await
}

async fn create_verified_backup(db: &DatabaseConnection, target_dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(target_dir)
        .with_context(|| format!("无法创建数据库备份目录: {}", target_dir.display()))?;

    let timestamp = Local::now().format("%Y%m%d_%H%M%S_%3f");
    let backup_path = target_dir.join(format!("reina_manager_{timestamp}.db"));
    create_snapshot(db, &backup_path).await?;

    let backup_url = format!("sqlite:{}?mode=ro", backup_path.display());
    let backup_db = Database::connect(&backup_url)
        .await
        .with_context(|| format!("无法打开备份数据库: {}", backup_path.display()))?;
    let integrity_result = verify_integrity(&backup_db, "备份数据库").await;
    backup_db.close().await?;
    integrity_result?;

    Ok(backup_path)
}

async fn create_snapshot(db: &DatabaseConnection, backup_path: &Path) -> Result<()> {
    let statement = Statement::from_sql_and_values(
        DatabaseBackend::Sqlite,
        "VACUUM INTO ?",
        [backup_path.to_string_lossy().into_owned().into()],
    );
    db.execute(statement)
        .await
        .with_context(|| format!("无法创建数据库快照: {}", backup_path.display()))?;
    Ok(())
}

async fn verify_integrity(db: &DatabaseConnection, label: &str) -> Result<()> {
    let statement = Statement::from_string(
        DatabaseBackend::Sqlite,
        "PRAGMA integrity_check".to_string(),
    );
    let row = db
        .query_one(statement)
        .await?
        .with_context(|| format!("{label}完整性检查没有返回结果"))?;
    let result: String = row.try_get("", "integrity_check")?;

    if result.eq_ignore_ascii_case("ok") {
        Ok(())
    } else {
        Err(anyhow::anyhow!("{label}完整性检查失败: {result}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!("reina-migrator-backup-test-{}", Uuid::new_v4()))
    }

    async fn create_source_database(
        directory: &Path,
        custom_backup_dir: Option<&Path>,
    ) -> (DatabaseConnection, String) {
        fs::create_dir_all(directory).unwrap();
        let db_path = directory.join("reina_manager.db");
        let db_url = format!("sqlite:{}?mode=rwc", db_path.display());
        let db = Database::connect(&db_url).await.unwrap();
        db.execute_unprepared(
            "CREATE TABLE user (id INTEGER PRIMARY KEY, db_backup_path TEXT);\
             CREATE TABLE values_table (value TEXT NOT NULL);\
             INSERT INTO values_table (value) VALUES ('kept');",
        )
        .await
        .unwrap();

        let statement = Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "INSERT INTO user (id, db_backup_path) VALUES (1, ?)",
            [custom_backup_dir
                .map(|path| path.to_string_lossy().into_owned())
                .into()],
        );
        db.execute(statement).await.unwrap();

        (db, format!("sqlite:{}", db_path.display()))
    }

    async fn assert_snapshot_contains_source_data(backup_path: &Path) {
        let backup_url = format!("sqlite:{}?mode=ro", backup_path.display());
        let backup_db = Database::connect(&backup_url).await.unwrap();
        let row = backup_db
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT value FROM values_table".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(row.try_get::<String>("", "value").unwrap(), "kept");
        backup_db.close().await.unwrap();
    }

    #[tokio::test]
    async fn uses_the_configured_backup_directory() {
        let directory = temporary_directory();
        let custom_dir = directory.join("custom-backups");
        fs::create_dir_all(&custom_dir).unwrap();
        let (db, db_url) = create_source_database(&directory, Some(&custom_dir)).await;

        let backup_path = backup_database(&db, &db_url).await.unwrap();

        assert_eq!(backup_path.parent(), Some(custom_dir.as_path()));
        assert_snapshot_contains_source_data(&backup_path).await;
        db.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn falls_back_when_the_configured_directory_is_invalid() {
        let directory = temporary_directory();
        let invalid_custom_dir = directory.join("not-a-directory");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&invalid_custom_dir, "file").unwrap();
        let (db, db_url) = create_source_database(&directory, Some(&invalid_custom_dir)).await;

        let backup_path = backup_database(&db, &db_url).await.unwrap();

        assert_eq!(
            backup_path.parent(),
            Some(directory.join("backups").as_path())
        );
        assert_snapshot_contains_source_data(&backup_path).await;
        db.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn ignores_an_unavailable_user_settings_schema() {
        let db = Database::connect("sqlite::memory:").await.unwrap();

        assert_eq!(read_custom_backup_dir(&db).await, None);

        db.close().await.unwrap();
    }
}
