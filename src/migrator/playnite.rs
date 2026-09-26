//! Playnite 导出数据迁移。

use anyhow::{Context, Result};
use reqwest::header::CONTENT_TYPE;
use sea_orm::ActiveValue::{self, NotSet, Set};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, IntoActiveModel,
    Statement, TransactionTrait,
};
use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::dedup::{self, ImportedStatistics, MatchResult, StatisticsSlot, TargetIndex};
use crate::playnite::{ExportDocument, ExportGame};
use crate::reina;

const MAX_COVER_BYTES: u64 = 20 * 1024 * 1024;

#[derive(Debug)]
struct LaunchFields {
    localpath: Option<String>,
    executable: Option<String>,
    launch_type: String,
    steam_launch_id: Option<String>,
}

#[derive(Debug, Default)]
struct MigrationSummary {
    imported: usize,
    statistics_filled: usize,
    unchanged: usize,
    ambiguous: usize,
    invalid: usize,
    unidentified: usize,
    covers_failed: usize,
}

#[derive(Debug, Default, Serialize)]
struct CustomData {
    #[serde(skip_serializing_if = "Option::is_none")]
    image: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    aliases: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    developer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nsfw: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_review: Option<String>,
}

pub async fn validate_target_schema(db: &DatabaseConnection) -> Result<()> {
    let rows = db
        .query_all(Statement::from_string(
            DatabaseBackend::Sqlite,
            "PRAGMA table_info(games)".to_string(),
        ))
        .await?;
    let columns: HashSet<String> = rows
        .into_iter()
        .map(|row| row.try_get::<String>("", "name"))
        .collect::<Result<_, _>>()?;
    let required = [
        "id_type",
        "date",
        "localpath",
        "executable",
        "launch_type",
        "steam_launch_id",
        "custom_data",
        "created_at",
        "updated_at",
    ];
    let missing: Vec<&str> = required
        .into_iter()
        .filter(|column| !columns.contains(*column))
        .collect();
    if !missing.is_empty() {
        return Err(anyhow::anyhow!(
            "目标数据库不是受支持的 ReinaManager v0.29.1+ 结构，games 缺少列: {}",
            missing.join(", ")
        ));
    }

    let stats_exists = db
        .query_one(Statement::from_sql_and_values(
            DatabaseBackend::Sqlite,
            "SELECT 1 AS found FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
            ["game_statistics".into()],
        ))
        .await?
        .is_some();
    if !stats_exists {
        return Err(anyhow::anyhow!("目标数据库缺少 game_statistics 表"));
    }

    Ok(())
}

pub async fn migrate(
    document: ExportDocument,
    db: &DatabaseConnection,
    database_url: &str,
) -> Result<()> {
    println!(
        "读取到 {} 个 Playnite 游戏（Playnite {}，导出于 {}）",
        document.games.len(),
        document.playnite_version,
        document.exported_at
    );

    let covers_root = database_file(database_url)?
        .parent()
        .context("无法获取 ReinaManager 数据目录")?
        .join("covers");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("ReinaMigrator/Playnite")
        .build()?;
    let mut identities = TargetIndex::load(db).await?;
    let transaction = db.begin().await?;
    let mut created_cover_files = Vec::new();

    let migration_result = migrate_games(
        &document.games,
        &transaction,
        &client,
        &covers_root,
        &mut identities,
        &mut created_cover_files,
    )
    .await;

    match migration_result {
        Ok(summary) => {
            if let Err(error) = transaction.commit().await {
                cleanup_cover_files(&created_cover_files);
                return Err(error.into());
            }
            println!(
                "Playnite 迁移结果：新建 {}，补统计 {}，未修改 {}，匹配歧义 {}，无效 {}，无标识新建 {}，封面失败 {}",
                summary.imported,
                summary.statistics_filled,
                summary.unchanged,
                summary.ambiguous,
                summary.invalid,
                summary.unidentified,
                summary.covers_failed
            );
            Ok(())
        }
        Err(error) => {
            let _ = transaction.rollback().await;
            cleanup_cover_files(&created_cover_files);
            Err(error)
        }
    }
}

async fn migrate_games<C: ConnectionTrait>(
    games: &[ExportGame],
    db: &C,
    client: &reqwest::Client,
    covers_root: &Path,
    identities: &mut TargetIndex,
    created_cover_files: &mut Vec<PathBuf>,
) -> Result<MigrationSummary> {
    let mut summary = MigrationSummary::default();

    for game in games {
        let Some(name) = non_empty(&game.name) else {
            eprintln!("跳过名称为空的 Playnite 游戏，导出 ID: {}", game.id);
            summary.invalid += 1;
            continue;
        };
        let launch = resolve_launch_fields(game);
        let keys = dedup::identities(
            launch.steam_launch_id.as_deref(),
            launch.localpath.as_deref(),
            launch.executable.as_deref(),
        );
        match identities.find(&keys) {
            MatchResult::Unique(game_id) => {
                let filled = if let Some(statistics) = build_statistics(game) {
                    if statistics.total_time > 0 {
                        StatisticsSlot::load(db, game_id)
                            .await?
                            .write(db, game_id, statistics)
                            .await?
                    } else {
                        false
                    }
                } else {
                    false
                };
                if filled {
                    summary.statistics_filled += 1;
                    println!("已补充重复游戏的游玩统计: {name}（目标 ID {game_id}）");
                } else {
                    summary.unchanged += 1;
                    println!(
                        "重复游戏未修改: {name}（目标 ID {game_id}，已有统计或来源无有效时长）"
                    );
                }
                continue;
            }
            MatchResult::Ambiguous => {
                summary.ambiguous += 1;
                eprintln!("游戏 {name} 匹配到多个 ReinaManager 条目，跳过以避免误合并");
                continue;
            }
            MatchResult::Missing => {}
        }
        if keys.is_empty() {
            summary.unidentified += 1;
            eprintln!("游戏 {name} 没有 Steam ID 或完整启动路径，再次迁移可能重复导入");
        }
        if game
            .play_action
            .as_ref()
            .is_some_and(|action| !action.arguments.trim().is_empty())
        {
            eprintln!("游戏 {name} 的启动参数无法写入 ReinaManager，将忽略该参数");
        }

        let mut custom_data = build_custom_data(game);
        let custom_data_json = serde_json::to_string(&custom_data)?;
        let inserted = reina::games::ActiveModel {
            id: NotSet,
            id_type: Set("Playnite".to_string()),
            date: Set(non_empty_owned(&game.release_date)),
            localpath: Set(launch.localpath.clone()),
            executable: Set(launch.executable.clone()),
            launch_type: Set(launch.launch_type.clone()),
            steam_launch_id: Set(launch.steam_launch_id.clone()),
            savepath: NotSet,
            autosave: NotSet,
            maxbackups: NotSet,
            clear: Set(Some(map_play_status(&game.completion_status))),
            le_launch: NotSet,
            magpie: NotSet,
            custom_data: Set(Some(custom_data_json)),
            created_at: set_i32_or_default(timestamp_i32(game.added.as_ref())),
            updated_at: set_i32_or_default(timestamp_i32(game.modified.as_ref())),
        }
        .insert(db)
        .await
        .with_context(|| format!("写入 Playnite 游戏失败: {name}"))?;
        let inserted_id = inserted.id;

        if let Some(statistics) = build_statistics(game) {
            StatisticsSlot::Vacant
                .write(db, inserted_id, statistics)
                .await?;
        }

        if let Some(cover) = non_empty(&game.cover) {
            match import_cover(client, cover, covers_root, inserted_id).await {
                Ok((identifier, cover_file)) => {
                    created_cover_files.push(cover_file);
                    custom_data.image = Some(identifier);
                    let mut active = inserted.into_active_model();
                    active.custom_data = Set(Some(serde_json::to_string(&custom_data)?));
                    active.update(db).await?;
                }
                Err(error) => {
                    eprintln!("游戏 {name} 的封面迁移失败，将保留游戏资料：{error}");
                    summary.covers_failed += 1;
                }
            }
        }

        identities.insert(inserted_id, &keys);
        summary.imported += 1;
        println!("已迁移游戏: {name}");
    }

    Ok(summary)
}

fn resolve_launch_fields(game: &ExportGame) -> LaunchFields {
    if game.source.to_lowercase().contains("steam") {
        if let Some(steam_launch_id) = dedup::normalize_steam_id(&game.provider_id) {
            return LaunchFields {
                localpath: non_empty_owned(&game.install_directory),
                executable: None,
                launch_type: "steam".to_string(),
                steam_launch_id: Some(steam_launch_id),
            };
        }
    }

    let install_directory = non_empty_owned(&game.install_directory);
    let executable_path = game
        .play_action
        .as_ref()
        .and_then(|action| resolve_executable_path(action, install_directory.as_deref()));
    let executable = executable_path
        .as_ref()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned());
    let localpath = executable_path
        .as_ref()
        .and_then(|path| path.parent())
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.to_string_lossy().into_owned())
        .or(install_directory);

    LaunchFields {
        localpath,
        executable,
        launch_type: "local".to_string(),
        steam_launch_id: None,
    }
}

fn resolve_executable_path(
    action: &crate::playnite::ExportPlayAction,
    install_directory: Option<&str>,
) -> Option<PathBuf> {
    let action_path = non_empty(&action.path).map(PathBuf::from)?;
    if action_path.is_absolute() {
        return Some(action_path);
    }

    let base = non_empty(&action.working_directory)
        .map(PathBuf::from)
        .map(|working_directory| {
            if working_directory.is_absolute() {
                working_directory
            } else if let Some(install_directory) = install_directory {
                PathBuf::from(install_directory).join(working_directory)
            } else {
                working_directory
            }
        })
        .or_else(|| install_directory.map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("."));
    Some(base.join(action_path))
}

fn build_custom_data(game: &ExportGame) -> CustomData {
    let name = non_empty_owned(&game.name);
    let aliases = non_empty(&game.sorting_name)
        .filter(|alias| !alias.eq_ignore_ascii_case(game.name.trim()))
        .map(|alias| vec![alias.to_string()]);
    let tags = merged_tags(game);
    let developer = join_non_empty(&game.developers);
    let nsfw = is_nsfw(game).then_some(true);
    let user_rating = game
        .user_score
        .filter(|score| (1..=100).contains(score))
        .map(|score| f64::from(score) / 10.0);

    CustomData {
        image: None,
        name,
        aliases,
        summary: non_empty_owned(&game.summary),
        tags: (!tags.is_empty()).then_some(tags),
        developer,
        nsfw,
        user_rating,
        user_review: non_empty_owned(&game.notes),
    }
}

fn merged_tags(game: &ExportGame) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for value in game
        .tags
        .iter()
        .chain(&game.genres)
        .chain(&game.categories)
        .chain(&game.platforms)
    {
        if let Some(value) = non_empty(value) {
            let key = value.to_lowercase();
            if seen.insert(key) {
                result.push(value.to_string());
            }
        }
    }
    result
}

fn is_nsfw(game: &ExportGame) -> bool {
    game.age_ratings
        .iter()
        .chain(&game.tags)
        .map(|value| value.trim().to_lowercase())
        .any(|value| {
            value == "ao"
                || value.contains("adults only")
                || value.contains("18+")
                || value.contains("r18")
        })
}

fn map_play_status(status: &str) -> i32 {
    match status.trim().to_lowercase().as_str() {
        "played" | "beaten" | "completed" => 2,
        "playing" => 3,
        "on hold" => 4,
        "abandoned" => 5,
        "not played" | "plan to play" => 1,
        _ => 1,
    }
}

fn build_statistics(game: &ExportGame) -> Option<ImportedStatistics> {
    if game.playtime_seconds == 0 && game.play_count == 0 && game.last_activity.is_none() {
        return None;
    }

    let rounded_minutes = game.playtime_seconds / 60 + u64::from(game.playtime_seconds % 60 >= 30);
    let total_time = i32::try_from(rounded_minutes).ok()?;
    let session_count = i32::try_from(game.play_count).ok()?;

    Some(ImportedStatistics {
        total_time,
        session_count,
        last_played: timestamp_i32(game.last_activity.as_ref()),
        daily_stats: "[]".to_string(),
    })
}

async fn import_cover(
    client: &reqwest::Client,
    source: &str,
    covers_root: &Path,
    game_id: i32,
) -> Result<(String, PathBuf)> {
    let normalized_source = source.to_ascii_lowercase();
    let (bytes, extension) =
        if normalized_source.starts_with("http://") || normalized_source.starts_with("https://") {
            download_cover(client, source).await?
        } else {
            read_local_cover(Path::new(source))?
        };

    let identifier = format!("{}_{}", extension, uuid::Uuid::new_v4().simple());
    let cover_dir = covers_root.join(format!("game_{game_id}"));
    fs::create_dir_all(&cover_dir)
        .with_context(|| format!("无法创建封面目录: {}", cover_dir.display()))?;
    let cover_file = cover_dir.join(format!("cover_{game_id}_{identifier}"));
    fs::write(&cover_file, bytes)
        .with_context(|| format!("无法写入封面文件: {}", cover_file.display()))?;
    Ok((identifier, cover_file))
}

async fn download_cover(client: &reqwest::Client, url: &str) -> Result<(Vec<u8>, String)> {
    let response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_COVER_BYTES)
    {
        return Err(anyhow::anyhow!("远程封面超过 20 MiB 限制"));
    }
    let extension = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(extension_from_content_type)
        .context("远程封面 Content-Type 不受支持")?;
    let bytes = response.bytes().await?;
    if bytes.len() as u64 > MAX_COVER_BYTES {
        return Err(anyhow::anyhow!("远程封面超过 20 MiB 限制"));
    }
    Ok((bytes.to_vec(), extension.to_string()))
}

fn read_local_cover(path: &Path) -> Result<(Vec<u8>, String)> {
    let metadata =
        fs::metadata(path).with_context(|| format!("无法读取本地封面: {}", path.display()))?;
    if !metadata.is_file() {
        return Err(anyhow::anyhow!("本地封面不是文件: {}", path.display()));
    }
    if metadata.len() > MAX_COVER_BYTES {
        return Err(anyhow::anyhow!("本地封面超过 20 MiB 限制"));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_lowercase)
        .filter(|value| {
            matches!(
                value.as_str(),
                "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
            )
        })
        .context("本地封面格式不受支持")?;
    Ok((fs::read(path)?, extension))
}

fn extension_from_content_type(content_type: &str) -> Option<&'static str> {
    match content_type
        .split(';')
        .next()?
        .trim()
        .to_lowercase()
        .as_str()
    {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/webp" => Some("webp"),
        "image/gif" => Some("gif"),
        "image/bmp" => Some("bmp"),
        _ => None,
    }
}

fn cleanup_cover_files(files: &[PathBuf]) {
    for file in files {
        if let Err(error) = fs::remove_file(file) {
            eprintln!("回滚时无法删除封面 {}：{error}", file.display());
            continue;
        }
        if let Some(parent) = file.parent() {
            let _ = fs::remove_dir(parent);
        }
    }
}

fn database_file(database_url: &str) -> Result<PathBuf> {
    let path = database_url
        .strip_prefix("sqlite:")
        .context("目标数据库 URL 必须使用 sqlite: 前缀")?;
    Ok(PathBuf::from(path))
}

fn timestamp_i32(value: Option<&chrono::DateTime<chrono::Utc>>) -> Option<i32> {
    value.and_then(|value| i32::try_from(value.timestamp()).ok())
}

fn set_i32_or_default(value: Option<i32>) -> ActiveValue<Option<i32>> {
    value.map_or(NotSet, |value| Set(Some(value)))
}

fn join_non_empty(values: &[String]) -> Option<String> {
    let values: Vec<&str> = values.iter().filter_map(|value| non_empty(value)).collect();
    (!values.is_empty()).then(|| values.join(", "))
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn non_empty_owned(value: &str) -> Option<String> {
    non_empty(value).map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::super::dedup::normalize_steam_id;
    use super::{map_play_status, migrate, resolve_launch_fields, validate_target_schema};
    use crate::playnite::{ExportDocument, ExportGame, ExportPlayAction};
    use chrono::{TimeZone, Utc};
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, Statement};
    use std::fs;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn temporary_directory() -> PathBuf {
        std::env::temp_dir().join(format!("reina-playnite-migration-test-{}", Uuid::new_v4()))
    }

    fn sample_document(cover: &std::path::Path) -> ExportDocument {
        ExportDocument {
            format: "reina-playnite-export".to_string(),
            version: 1,
            exported_at: Utc.with_ymd_and_hms(2026, 9, 3, 12, 0, 0).unwrap(),
            playnite_version: "10.40".to_string(),
            games: vec![ExportGame {
                id: "playnite-id".to_string(),
                name: "Example Game".to_string(),
                sorting_name: "Example, The".to_string(),
                summary: "Summary".to_string(),
                cover: cover.to_string_lossy().into_owned(),
                developers: vec!["Developer".to_string()],
                publishers: vec!["Publisher".to_string()],
                tags: vec!["Visual Novel".to_string()],
                genres: vec!["Adventure".to_string()],
                categories: vec!["Visual Novel".to_string()],
                platforms: vec!["Windows".to_string()],
                age_ratings: vec!["18+".to_string()],
                user_score: Some(85),
                release_date: "2025-04-03".to_string(),
                notes: "Review".to_string(),
                completion_status: "Abandoned".to_string(),
                source: "Local".to_string(),
                provider_id: String::new(),
                install_directory: r"D:\Games\Example".to_string(),
                added: Some(Utc.with_ymd_and_hms(2025, 1, 2, 3, 4, 5).unwrap()),
                modified: Some(Utc.with_ymd_and_hms(2025, 2, 3, 4, 5, 6).unwrap()),
                playtime_seconds: 3_630,
                play_count: 4,
                last_activity: Some(Utc.with_ymd_and_hms(2026, 8, 20, 10, 30, 0).unwrap()),
                play_action: Some(ExportPlayAction {
                    path: "Example.exe".to_string(),
                    working_directory: String::new(),
                    arguments: String::new(),
                    tracking_mode: 0,
                    tracking_path: String::new(),
                }),
            }],
        }
    }

    async fn create_target_database(
        directory: &std::path::Path,
    ) -> (sea_orm::DatabaseConnection, String) {
        fs::create_dir_all(directory).unwrap();
        let database_path = directory.join("reina_manager.db");
        let connection_url = format!("sqlite:{}?mode=rwc", database_path.display());
        let database = Database::connect(&connection_url).await.unwrap();
        database
            .execute_unprepared(
                r#"
                CREATE TABLE games (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    id_type TEXT NOT NULL,
                    date TEXT,
                    localpath TEXT,
                    executable TEXT,
                    launch_type TEXT NOT NULL DEFAULT 'local',
                    steam_launch_id TEXT,
                    savepath TEXT,
                    autosave INTEGER DEFAULT 0,
                    maxbackups INTEGER DEFAULT 20,
                    clear INTEGER DEFAULT 1,
                    le_launch INTEGER DEFAULT 0,
                    magpie INTEGER DEFAULT 0,
                    custom_data TEXT,
                    created_at INTEGER DEFAULT (strftime('%s', 'now')),
                    updated_at INTEGER DEFAULT (strftime('%s', 'now'))
                );
                CREATE TABLE game_statistics (
                    game_id INTEGER PRIMARY KEY,
                    total_time INTEGER,
                    session_count INTEGER,
                    last_played INTEGER,
                    daily_stats TEXT
                );
                CREATE TABLE game_sessions (
                    session_id INTEGER PRIMARY KEY AUTOINCREMENT,
                    game_id INTEGER NOT NULL,
                    start_time INTEGER NOT NULL,
                    end_time INTEGER NOT NULL,
                    duration INTEGER NOT NULL,
                    date TEXT NOT NULL
                );
                CREATE TABLE game_sources (
                    game_id INTEGER NOT NULL,
                    source TEXT NOT NULL,
                    external_id TEXT,
                    data TEXT,
                    PRIMARY KEY (game_id, source)
                );
                "#,
            )
            .await
            .unwrap();
        (database, format!("sqlite:{}", database_path.display()))
    }

    #[test]
    fn maps_all_default_playnite_statuses() {
        assert_eq!(map_play_status("Not Played"), 1);
        assert_eq!(map_play_status("Plan to Play"), 1);
        assert_eq!(map_play_status("Played"), 2);
        assert_eq!(map_play_status("Beaten"), 2);
        assert_eq!(map_play_status("Completed"), 2);
        assert_eq!(map_play_status("Playing"), 3);
        assert_eq!(map_play_status("On Hold"), 4);
        assert_eq!(map_play_status("Abandoned"), 5);
        assert_eq!(map_play_status("自定义状态"), 1);
    }

    #[test]
    fn accepts_only_positive_decimal_steam_ids() {
        assert_eq!(normalize_steam_id(" 000730 ").as_deref(), Some("730"));
        assert_eq!(normalize_steam_id("0"), None);
        assert_eq!(normalize_steam_id("steam"), None);
    }

    #[test]
    fn maps_a_steam_provider_to_reina_steam_launch_fields() {
        let document = sample_document(std::path::Path::new("cover.png"));
        let mut game = document.games.into_iter().next().unwrap();
        game.source = "Steam".to_string();
        game.provider_id = "000730".to_string();

        let launch = resolve_launch_fields(&game);

        assert_eq!(launch.launch_type, "steam");
        assert_eq!(launch.steam_launch_id.as_deref(), Some("730"));
        assert_eq!(launch.localpath.as_deref(), Some(r"D:\Games\Example"));
        assert_eq!(launch.executable, None);
    }

    #[tokio::test]
    async fn rejects_a_pre_v029_target_schema() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database
            .execute_unprepared(
                "CREATE TABLE games (id INTEGER PRIMARY KEY, id_type TEXT NOT NULL); \
                 CREATE TABLE game_statistics (game_id INTEGER PRIMARY KEY);",
            )
            .await
            .unwrap();

        let error = validate_target_schema(&database).await.unwrap_err();

        assert!(error.to_string().contains("v0.29.1+"));
        database.close().await.unwrap();
    }

    #[tokio::test]
    async fn migrates_full_metadata_statistics_and_cover_without_sources_or_sessions() {
        let directory = temporary_directory();
        let source_cover = directory.join("playnite-cover.png");
        fs::create_dir_all(&directory).unwrap();
        fs::write(&source_cover, b"image").unwrap();
        let (database, database_url) = create_target_database(&directory).await;

        validate_target_schema(&database).await.unwrap();
        migrate(sample_document(&source_cover), &database, &database_url)
            .await
            .unwrap();

        let game = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM games".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(game.try_get::<String>("", "id_type").unwrap(), "Playnite");
        assert_eq!(game.try_get::<i32>("", "clear").unwrap(), 5);
        assert_eq!(
            game.try_get::<String>("", "localpath").unwrap(),
            r"D:\Games\Example"
        );
        assert_eq!(
            game.try_get::<String>("", "executable").unwrap(),
            "Example.exe"
        );
        let custom_data: serde_json::Value =
            serde_json::from_str(&game.try_get::<String>("", "custom_data").unwrap()).unwrap();
        assert_eq!(custom_data["user_rating"], 8.5);
        assert_eq!(custom_data["nsfw"], true);
        assert_eq!(custom_data["tags"].as_array().unwrap().len(), 3);
        let image_identifier = custom_data["image"].as_str().unwrap();
        let cover_path = directory
            .join("covers")
            .join("game_1")
            .join(format!("cover_1_{image_identifier}"));
        assert!(cover_path.is_file());

        let statistics = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM game_statistics".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(statistics.try_get::<i32>("", "total_time").unwrap(), 61);
        assert_eq!(statistics.try_get::<i32>("", "session_count").unwrap(), 4);
        assert_eq!(
            statistics.try_get::<String>("", "daily_stats").unwrap(),
            "[]"
        );

        for table in ["game_sources", "game_sessions"] {
            let row = database
                .query_one(Statement::from_string(
                    DatabaseBackend::Sqlite,
                    format!("SELECT COUNT(*) AS count FROM {table}"),
                ))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.try_get::<i64>("", "count").unwrap(), 0);
        }

        migrate(sample_document(&source_cover), &database, &database_url)
            .await
            .unwrap();
        let count = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM games".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "count").unwrap(), 1);

        database.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn fills_only_empty_statistics_on_a_matching_existing_game() {
        let directory = temporary_directory();
        let (database, database_url) = create_target_database(&directory).await;
        database
            .execute_unprepared(
                r#"INSERT INTO games (id, id_type, localpath, executable, savepath, custom_data)
                   VALUES (7, 'Whitecloud', 'd:\games\example\', 'EXAMPLE.EXE', 'D:\Saves', '{"name":"Existing"}');
                   INSERT INTO game_statistics (game_id, total_time, session_count, last_played, daily_stats)
                   VALUES (7, 0, 0, 123, '[]');"#,
            )
            .await
            .unwrap();

        let mut document = sample_document(std::path::Path::new("unused.png"));
        document.games[0].cover.clear();
        migrate(document, &database, &database_url).await.unwrap();

        let game = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM games WHERE id = 7".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(game.try_get::<String>("", "savepath").unwrap(), r"D:\Saves");
        assert_eq!(
            game.try_get::<String>("", "custom_data").unwrap(),
            r#"{"name":"Existing"}"#
        );
        let statistics = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM game_statistics WHERE game_id = 7".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(statistics.try_get::<i32>("", "total_time").unwrap(), 61);
        assert_eq!(statistics.try_get::<i32>("", "session_count").unwrap(), 4);
        assert_eq!(statistics.try_get::<i32>("", "last_played").unwrap(), 123);

        let mut later = sample_document(std::path::Path::new("unused.png"));
        later.games[0].cover.clear();
        later.games[0].playtime_seconds = 9_000;
        migrate(later, &database, &database_url).await.unwrap();
        let count = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM games".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "count").unwrap(), 1);
        let statistics = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT total_time FROM game_statistics WHERE game_id = 7".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(statistics.try_get::<i32>("", "total_time").unwrap(), 61);

        database.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn does_not_fill_statistics_when_the_target_has_sessions() {
        let directory = temporary_directory();
        let (database, database_url) = create_target_database(&directory).await;
        database
            .execute_unprepared(
                r#"INSERT INTO games (id, id_type, localpath, executable)
                   VALUES (7, 'Whitecloud', 'D:\Games\Example', 'Example.exe');
                   INSERT INTO game_sessions (game_id, start_time, end_time, duration, date)
                   VALUES (7, 100, 160, 1, '2026-01-01');"#,
            )
            .await
            .unwrap();
        let mut document = sample_document(std::path::Path::new("unused.png"));
        document.games[0].cover.clear();
        migrate(document, &database, &database_url).await.unwrap();

        let statistics = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT * FROM game_statistics WHERE game_id = 7".to_string(),
            ))
            .await
            .unwrap();
        assert!(statistics.is_none());

        database.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn imports_games_without_a_strong_identity_each_time() {
        let directory = temporary_directory();
        let (database, database_url) = create_target_database(&directory).await;
        let mut document = sample_document(std::path::Path::new("unused.png"));
        document.games[0].cover.clear();
        document.games[0].install_directory.clear();
        document.games[0].play_action = None;
        let mut repeated = sample_document(std::path::Path::new("unused.png"));
        repeated.games[0].cover.clear();
        repeated.games[0].install_directory.clear();
        repeated.games[0].play_action = None;
        migrate(document, &database, &database_url).await.unwrap();
        migrate(repeated, &database, &database_url).await.unwrap();

        let count = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM games".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "count").unwrap(), 2);

        database.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn fills_empty_statistics_by_steam_id() {
        let directory = temporary_directory();
        let (database, database_url) = create_target_database(&directory).await;
        database
            .execute_unprepared(
                "INSERT INTO games (id, id_type, steam_launch_id) VALUES (7, 'custom', '000730')",
            )
            .await
            .unwrap();
        let mut document = sample_document(std::path::Path::new("unused.png"));
        document.games[0].source = "Steam".to_string();
        document.games[0].provider_id = "730".to_string();
        document.games[0].cover.clear();

        migrate(document, &database, &database_url).await.unwrap();

        let count = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM games".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "count").unwrap(), 1);
        let statistics = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT total_time FROM game_statistics WHERE game_id = 7".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(statistics.try_get::<i32>("", "total_time").unwrap(), 61);

        database.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn skips_an_ambiguous_existing_path() {
        let directory = temporary_directory();
        let (database, database_url) = create_target_database(&directory).await;
        database
            .execute_unprepared(
                r#"INSERT INTO games (id, id_type, localpath, executable)
                   VALUES (7, 'custom', 'D:\Games\Example', 'Example.exe');
                   INSERT INTO games (id, id_type, localpath, executable)
                   VALUES (8, 'custom', 'd:\games\example', 'EXAMPLE.EXE');"#,
            )
            .await
            .unwrap();
        let mut document = sample_document(std::path::Path::new("unused.png"));
        document.games[0].cover.clear();

        migrate(document, &database, &database_url).await.unwrap();

        let count = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM games".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(count.try_get::<i64>("", "count").unwrap(), 2);
        let statistics = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT COUNT(*) AS count FROM game_statistics".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(statistics.try_get::<i64>("", "count").unwrap(), 0);

        database.close().await.unwrap();
        fs::remove_dir_all(directory).unwrap();
    }
}
