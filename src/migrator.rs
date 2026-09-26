//! 迁移器主模块
//!
//! 负责整体迁移流程编排：连接数据库 → 预加载旧数据 → 事务写入新数据。

mod backup;
mod convert;
mod dedup;
mod playnite;
mod process;

use anyhow::Result;
use sea_orm::prelude::*;
use sea_orm::ActiveValue::NotSet;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, Set,
    TransactionTrait,
};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;

use crate::config::Config;
use crate::db::connection::{connect_new_db, connect_old_db};
use crate::{reina, whitecloud};

use self::convert::{
    build_custom_data_json, build_daily_stats_json, build_launch_fields, resolve_event_duration,
    timestamp_to_date,
};
use self::dedup::{GameIdentity, ImportedStatistics, MatchResult, StatisticsSlot, TargetIndex};

/// 迁移数据来源。
pub enum MigrationSource {
    Whitecloud,
    Playnite { export_path: PathBuf },
}

// ─────────────────────────── 预加载数据 ───────────────────────────

/// Whitecloud 旧数据的预加载缓存（按游戏 UUID 分组）
struct PreloadedData {
    events_by_game: HashMap<String, Vec<whitecloud::event::Model>>,
    histories_by_game: HashMap<String, Vec<whitecloud::history::Model>>,
}

impl PreloadedData {
    /// 一次性加载所有 PlayEvent 和 History，按游戏 UUID 分组
    async fn load(old_db: &DatabaseConnection) -> Result<Self> {
        let all_events = whitecloud::event::Entity::find()
            .filter(whitecloud::event::Column::EventType.eq("PlayEvent"))
            .all(old_db)
            .await?;
        let all_histories = whitecloud::history::Entity::find().all(old_db).await?;

        let mut events_by_game: HashMap<String, Vec<whitecloud::event::Model>> = HashMap::new();
        for event in all_events {
            if let Some(uuid) = event.game.clone() {
                events_by_game.entry(uuid).or_default().push(event);
            }
        }

        let mut histories_by_game: HashMap<String, Vec<whitecloud::history::Model>> =
            HashMap::new();
        for history in all_histories {
            if let Some(uuid) = history.game.clone() {
                histories_by_game.entry(uuid).or_default().push(history);
            }
        }

        Ok(Self {
            events_by_game,
            histories_by_game,
        })
    }
}

// ─────────────────────────── 迁移入口 ───────────────────────────

/// 执行完整的 Whitecloud → Reina 数据迁移流程
pub async fn run_migration() -> Result<()> {
    let new_database_path = Config::new_database_path()?;
    run_migration_to(&new_database_path).await
}

/// 将 Whitecloud 数据迁移到指定的 ReinaManager 数据库
pub async fn run_migration_to(new_database_path: &str) -> Result<()> {
    run_migration_from_to(MigrationSource::Whitecloud, new_database_path).await
}

/// 将指定来源迁移到 ReinaManager 数据库。
pub async fn run_migration_from_to(source: MigrationSource, new_database_path: &str) -> Result<()> {
    crate::log_info!("Reina Migrator - 数据迁移工具");

    // 1. 在等待用户关闭程序前再次校验目标数据库，避免选择后路径发生变化
    Config::validate_database_url(new_database_path)?;
    let playnite_export = match &source {
        MigrationSource::Whitecloud => None,
        MigrationSource::Playnite { export_path } => {
            Some(crate::playnite::ExportDocument::read(export_path)?)
        }
    };

    // 2. 等待用户手动关闭 ReinaManager，避免丢失尚未保存的数据
    match process::wait_for_reina_manager_exit()? {
        process::ProcessStatus::AlreadyStopped => {
            tracing::info!("ReinaManager 未运行，可以开始迁移");
        }
        process::ProcessStatus::StoppedAfterPrompt => {
            crate::log_info!("ReinaManager 已退出，继续迁移。");
        }
        process::ProcessStatus::Cancelled => {
            crate::log_info!("迁移已取消，目标数据库未修改。");
            pause_before_exit()?;
            return Ok(());
        }
    }

    match &source {
        MigrationSource::Whitecloud => {
            crate::log_info!("Whitecloud 数据库: {}", Config::old_database_path()?)
        }
        MigrationSource::Playnite { export_path } => {
            crate::log_info!("Playnite 导出文件: {}", export_path.display())
        }
    }
    crate::log_info!("ReinaManager 数据库: {}", new_database_path);

    // 3. 连接数据库
    crate::log_info!("连接数据库...");
    let new_db = connect_new_db(new_database_path).await?;

    if matches!(source, MigrationSource::Playnite { .. }) {
        playnite::validate_target_schema(&new_db).await?;
    }

    // 4. 备份新数据库
    backup::backup_database(&new_db, new_database_path).await?;

    // 5. 执行数据迁移
    crate::log_info!("开始数据迁移...");
    match source {
        MigrationSource::Whitecloud => {
            let old_database_path = Config::old_database_path()?;
            let old_db = connect_old_db(&old_database_path).await?;
            migrate_games(&old_db, &new_db).await?;
            old_db.close().await?;
        }
        MigrationSource::Playnite { .. } => {
            playnite::migrate(
                playnite_export.expect("Playnite 来源应已完成导出文件解析"),
                &new_db,
                new_database_path,
            )
            .await?;
        }
    }

    // 6. 关闭数据库连接
    crate::log_info!("关闭数据库连接...");
    new_db.close().await?;

    crate::log_info!("🎉 数据迁移完成！");
    println!();
    println!("现在您可以重新启动 ReinaManager 查看迁移的数据。");
    pause_before_exit()?;

    Ok(())
}

fn pause_before_exit() -> Result<()> {
    println!("按 Enter 退出...");
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(())
}

// ─────────────────────────── 游戏迁移 ───────────────────────────

/// 迁移所有游戏及其关联的会话和统计数据
async fn migrate_games(old_db: &DatabaseConnection, new_db: &DatabaseConnection) -> Result<()> {
    let old_games = whitecloud::games::Entity::find()
        .order_by_asc(whitecloud::games::Column::Id)
        .all(old_db)
        .await?;
    crate::log_info!("找到 {} 个 Whitecloud 游戏需要迁移", old_games.len());

    let preloaded = PreloadedData::load(old_db).await?;
    let groups = group_whitecloud_games(&old_games);
    let mut identities = TargetIndex::load(new_db).await?;
    let txn = new_db.begin().await?;
    let mut imported = 0;
    let mut statistics_filled = 0;
    let mut savepaths_filled = 0;
    let mut unchanged = 0;
    let mut ambiguous = 0;
    let mut unidentified = 0;

    for group in groups {
        let first = group[0];
        let name = group
            .iter()
            .filter_map(|game| game.name.as_deref())
            .find(|name| !name.trim().is_empty())
            .map(str::to_owned);
        let savepaths: Vec<&str> = group
            .iter()
            .filter_map(|game| game.save_dir.as_deref())
            .filter(|path| !path.trim().is_empty())
            .collect();
        let savepath = savepaths.first().and_then(|first| {
            if savepaths
                .iter()
                .any(|path| dedup::normalize_path(path) != dedup::normalize_path(first))
            {
                crate::log_warn!(
                    "Whitecloud 游戏 {:?} 有冲突的存档路径，本次不补存档路径",
                    name
                );
                None
            } else {
                Some((*first).to_string())
            }
        });
        let (localpath, executable) = build_launch_fields(&first.game_dir, &first.exe_path);
        let keys = dedup::identities(None, localpath.as_deref(), executable.as_deref());

        match identities.find(&keys) {
            MatchResult::Unique(game_id) => {
                let mut changed = false;
                if let Some(savepath) = savepath.as_ref() {
                    let target = reina::games::Entity::find_by_id(game_id)
                        .one(&txn)
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("重复游戏的目标 ID 不存在: {game_id}"))?;
                    if target
                        .savepath
                        .as_deref()
                        .is_none_or(|path| path.trim().is_empty())
                    {
                        let mut active = target.into_active_model();
                        active.savepath = Set(Some(savepath.clone()));
                        active.update(&txn).await?;
                        savepaths_filled += 1;
                        changed = true;
                    }
                }
                if migrate_group_sessions(&txn, &group, &preloaded, game_id).await? {
                    statistics_filled += 1;
                    changed = true;
                }
                if !changed {
                    unchanged += 1;
                }
                if changed {
                    crate::log_info!("已补充 Whitecloud 游戏: {:?}（目标 ID {game_id}）", name);
                } else {
                    crate::log_info!(
                        "重复 Whitecloud 游戏未修改: {:?}（目标 ID {game_id}）",
                        name
                    );
                }
            }
            MatchResult::Ambiguous => {
                ambiguous += 1;
                crate::log_warn!(
                    "Whitecloud 游戏 {:?} 匹配到多个 ReinaManager 条目，跳过以避免误合并",
                    name
                );
            }
            MatchResult::Missing => {
                if keys.is_empty() {
                    unidentified += 1;
                    crate::log_warn!(
                        "Whitecloud 游戏 {:?} 没有完整启动路径，再次迁移可能重复导入",
                        name
                    );
                }
                let new_game = reina::games::ActiveModel {
                    id: NotSet,
                    id_type: Set("Whitecloud".to_string()),
                    date: NotSet,
                    localpath: Set(localpath),
                    executable: Set(executable),
                    launch_type: NotSet,
                    steam_launch_id: NotSet,
                    savepath: Set(savepath),
                    autosave: NotSet,
                    maxbackups: NotSet,
                    clear: Set(Some(1)),
                    le_launch: NotSet,
                    magpie: NotSet,
                    custom_data: Set(build_custom_data_json(&name)?),
                    created_at: NotSet,
                    updated_at: NotSet,
                };
                let inserted = new_game.insert(&txn).await?;
                identities.insert(inserted.id, &keys);
                migrate_group_sessions(&txn, &group, &preloaded, inserted.id).await?;
                imported += 1;
                crate::log_info!(
                    "已迁移 Whitecloud 游戏: {:?}（目标 ID {}）",
                    name,
                    inserted.id
                );
            }
        }
    }

    txn.commit().await?;
    crate::log_info!(
        "Whitecloud 迁移结果：新建 {imported}，补统计 {statistics_filled}，补存档路径 {savepaths_filled}，未修改 {unchanged}，匹配歧义 {ambiguous}，无标识新建 {unidentified}"
    );
    Ok(())
}

fn group_whitecloud_games(
    games: &[whitecloud::games::Model],
) -> Vec<Vec<&whitecloud::games::Model>> {
    let mut groups: Vec<Vec<&whitecloud::games::Model>> = Vec::new();
    let mut positions: HashMap<GameIdentity, usize> = HashMap::new();
    for game in games {
        let (localpath, executable) = build_launch_fields(&game.game_dir, &game.exe_path);
        let key = dedup::identities(None, localpath.as_deref(), executable.as_deref())
            .into_iter()
            .next();
        if let Some(key) = key {
            if let Some(position) = positions.get(&key) {
                groups[*position].push(game);
            } else {
                positions.insert(key, groups.len());
                groups.push(vec![game]);
            }
        } else {
            groups.push(vec![game]);
        }
    }
    groups
}

// ─────────────────────────── 会话迁移 ───────────────────────────

/// 合并同一启动项的真实会话，仅在目标没有游玩数据时写入。
async fn migrate_group_sessions<C: ConnectionTrait>(
    db: &C,
    games: &[&whitecloud::games::Model],
    preloaded: &PreloadedData,
    new_game_id: i32,
) -> Result<bool> {
    let slot = StatisticsSlot::load(db, new_game_id).await?;
    if !slot.is_available() {
        return Ok(false);
    }
    let mut session_batch = Vec::new();
    let mut total_time = 0i64;
    let mut last_played: Option<i32> = None;
    let mut daily_stats: HashMap<String, i64> = HashMap::new();
    let mut seen_sessions = HashSet::new();

    for game in games {
        let Some(uuid) = game.uuid.as_deref() else {
            continue;
        };
        let Some(events) = preloaded.events_by_game.get(uuid) else {
            continue;
        };
        let histories = preloaded.histories_by_game.get(uuid).map(Vec::as_slice);
        for event in events {
            let duration = resolve_event_duration(event, histories);
            let Some(end_time) = event.time.map(|time| (time / 1000.0) as i64) else {
                continue;
            };
            let Some(start_time) = duration
                .checked_mul(60)
                .and_then(|seconds| end_time.checked_sub(seconds))
            else {
                continue;
            };
            let (Ok(duration), Ok(start_time), Ok(end_time)) = (
                i32::try_from(duration),
                i32::try_from(start_time),
                i32::try_from(end_time),
            ) else {
                continue;
            };
            if duration <= 0 || !seen_sessions.insert((start_time, end_time, duration)) {
                continue;
            }
            let date = timestamp_to_date(i64::from(start_time));
            session_batch.push(reina::game_sessions::ActiveModel {
                session_id: Default::default(),
                game_id: Set(new_game_id),
                start_time: Set(start_time),
                end_time: Set(end_time),
                duration: Set(duration),
                date: Set(date.clone()),
            });
            total_time = total_time
                .checked_add(i64::from(duration))
                .ok_or_else(|| anyhow::anyhow!("Whitecloud 游戏累计时长溢出"))?;
            last_played = Some(end_time.max(last_played.unwrap_or(0)));
            *daily_stats.entry(date).or_insert(0) += i64::from(duration);
        }
    }
    if session_batch.is_empty() {
        return Ok(false);
    }
    let total_time = i32::try_from(total_time)
        .map_err(|_| anyhow::anyhow!("Whitecloud 游戏累计时长超出 ReinaManager 支持范围"))?;
    let session_count = i32::try_from(session_batch.len())
        .map_err(|_| anyhow::anyhow!("Whitecloud 游戏会话次数超出 ReinaManager 支持范围"))?;
    let statistics = ImportedStatistics {
        total_time,
        session_count,
        last_played,
        daily_stats: build_daily_stats_json(daily_stats)?,
    };
    reina::game_sessions::Entity::insert_many(session_batch)
        .exec(db)
        .await?;
    slot.write(db, new_game_id, statistics).await
}

#[cfg(test)]
mod tests {
    use super::migrate_games;
    use sea_orm::{ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, Statement};

    async fn whitecloud_source() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            r#"CREATE TABLE games (
                id INTEGER PRIMARY KEY, name TEXT, gameDir TEXT, saveDir TEXT, exePath TEXT,
                state INTEGER, uuid TEXT, updateTime REAL, "order" REAL,
                nativeSaveNumber REAL, startWithStrategy INTEGER
            );
            CREATE TABLE events (
                id INTEGER PRIMARY KEY, game TEXT, state TEXT, context BLOB,
                time REAL, host TEXT, type TEXT, server_id REAL
            );
            CREATE TABLE history (
                id INTEGER PRIMARY KEY, game TEXT, start REAL, "end" REAL,
                token REAL, server_id REAL
            );
            INSERT INTO games (id, name, gameDir, saveDir, exePath, uuid)
            VALUES (1, 'Game', 'D:\Games\Example', 'D:\Saves', 'Example.exe', 'first');
            INSERT INTO games (id, name, gameDir, saveDir, exePath, uuid)
            VALUES (2, 'Game', 'd:\games\example\', NULL, 'EXAMPLE.EXE', 'second');
            INSERT INTO events (id, game, context, time, type)
            VALUES (1, 'first', CAST('{"playtime":3600000}' AS BLOB), 1735693200000.0, 'PlayEvent');
            INSERT INTO events (id, game, context, time, type)
            VALUES (2, 'second', CAST('{"playtime":3600000}' AS BLOB), 1735693200000.0, 'PlayEvent');
            INSERT INTO events (id, game, context, time, type)
            VALUES (3, 'second', CAST('{"playtime":1800000}' AS BLOB), 1735696800000.0, 'PlayEvent');"#,
        )
        .await
        .unwrap();
        db
    }

    async fn reina_target() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        db.execute_unprepared(
            r#"CREATE TABLE games (
                id INTEGER PRIMARY KEY AUTOINCREMENT, id_type TEXT NOT NULL, date TEXT,
                localpath TEXT, executable TEXT, launch_type TEXT NOT NULL DEFAULT 'local',
                steam_launch_id TEXT, savepath TEXT, autosave INTEGER, maxbackups INTEGER,
                clear INTEGER, le_launch INTEGER, magpie INTEGER, custom_data TEXT,
                created_at INTEGER, updated_at INTEGER
            );
            CREATE TABLE game_statistics (
                game_id INTEGER PRIMARY KEY, total_time INTEGER, session_count INTEGER,
                last_played INTEGER, daily_stats TEXT
            );
            CREATE TABLE game_sessions (
                session_id INTEGER PRIMARY KEY AUTOINCREMENT, game_id INTEGER,
                start_time INTEGER, end_time INTEGER, duration INTEGER, date TEXT
            );"#,
        )
        .await
        .unwrap();
        db
    }

    async fn scalar(db: &DatabaseConnection, sql: &str) -> i64 {
        db.query_one(Statement::from_string(
            DatabaseBackend::Sqlite,
            sql.to_string(),
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "value")
        .unwrap()
    }

    #[tokio::test]
    async fn groups_whitecloud_sessions_and_is_idempotent_for_an_existing_game() {
        let old = whitecloud_source().await;
        let target = reina_target().await;
        target
            .execute_unprepared(
            r#"INSERT INTO games (id, id_type, localpath, executable, savepath, custom_data)
               VALUES (7, 'Playnite', 'd:\games\example', 'example.exe', '   ', '{"name":"Existing"}');
               INSERT INTO game_statistics (game_id, total_time, session_count, daily_stats)
               VALUES (7, 0, 0, '[]');"#,
            )
            .await
            .unwrap();

        migrate_games(&old, &target).await.unwrap();
        migrate_games(&old, &target).await.unwrap();

        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM games").await,
            1
        );
        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM game_sessions").await,
            2
        );
        assert_eq!(
            scalar(
                &target,
                "SELECT total_time AS value FROM game_statistics WHERE game_id = 7"
            )
            .await,
            90
        );
        let game = target
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT savepath, custom_data FROM games WHERE id = 7".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(game.try_get::<String>("", "savepath").unwrap(), r"D:\Saves");
        assert_eq!(
            game.try_get::<String>("", "custom_data").unwrap(),
            r#"{"name":"Existing"}"#
        );

        old.close().await.unwrap();
        target.close().await.unwrap();
    }

    #[tokio::test]
    async fn creates_one_whitecloud_game_for_duplicate_source_rows() {
        let old = whitecloud_source().await;
        let target = reina_target().await;

        migrate_games(&old, &target).await.unwrap();

        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM games").await,
            1
        );
        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM game_sessions").await,
            2
        );
        assert_eq!(
            scalar(&target, "SELECT total_time AS value FROM game_statistics").await,
            90
        );

        old.close().await.unwrap();
        target.close().await.unwrap();
    }

    #[tokio::test]
    async fn preserves_nonzero_reina_statistics_and_savepath() {
        let old = whitecloud_source().await;
        let target = reina_target().await;
        target
            .execute_unprepared(
                r#"INSERT INTO games (id, id_type, localpath, executable, savepath)
               VALUES (7, 'Playnite', 'D:\Games\Example', 'Example.exe', 'D:\OwnSaves');
               INSERT INTO game_statistics (game_id, total_time, session_count, daily_stats)
               VALUES (7, 15, 1, '[]');"#,
            )
            .await
            .unwrap();

        migrate_games(&old, &target).await.unwrap();

        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM games").await,
            1
        );
        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM game_sessions").await,
            0
        );
        assert_eq!(
            scalar(&target, "SELECT total_time AS value FROM game_statistics").await,
            15
        );
        let game = target
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT savepath FROM games WHERE id = 7".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            game.try_get::<String>("", "savepath").unwrap(),
            r"D:\OwnSaves"
        );

        old.close().await.unwrap();
        target.close().await.unwrap();
    }

    #[tokio::test]
    async fn skips_ambiguous_whitecloud_matches() {
        let old = whitecloud_source().await;
        let target = reina_target().await;
        target
            .execute_unprepared(
                r#"INSERT INTO games (id, id_type, localpath, executable)
                   VALUES (7, 'custom', 'D:\Games\Example', 'Example.exe');
                   INSERT INTO games (id, id_type, localpath, executable)
                   VALUES (8, 'custom', 'd:\games\example', 'EXAMPLE.EXE');"#,
            )
            .await
            .unwrap();

        migrate_games(&old, &target).await.unwrap();

        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM games").await,
            2
        );
        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM game_sessions").await,
            0
        );

        old.close().await.unwrap();
        target.close().await.unwrap();
    }

    #[tokio::test]
    async fn imports_unidentified_whitecloud_games_each_time() {
        let old = whitecloud_source().await;
        old.execute_unprepared("UPDATE games SET gameDir = NULL, exePath = NULL")
            .await
            .unwrap();
        let target = reina_target().await;

        migrate_games(&old, &target).await.unwrap();
        migrate_games(&old, &target).await.unwrap();

        assert_eq!(
            scalar(&target, "SELECT COUNT(*) AS value FROM games").await,
            4
        );

        old.close().await.unwrap();
        target.close().await.unwrap();
    }

    #[tokio::test]
    async fn does_not_choose_between_conflicting_whitecloud_savepaths() {
        let old = whitecloud_source().await;
        old.execute_unprepared(r"UPDATE games SET saveDir = 'D:\OtherSaves' WHERE id = 2")
            .await
            .unwrap();
        let target = reina_target().await;

        migrate_games(&old, &target).await.unwrap();

        let game = target
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT savepath FROM games".to_string(),
            ))
            .await
            .unwrap()
            .unwrap();
        assert!(game
            .try_get::<Option<String>>("", "savepath")
            .unwrap()
            .is_none());

        old.close().await.unwrap();
        target.close().await.unwrap();
    }
}
