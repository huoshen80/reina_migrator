//! 迁移器主模块
//!
//! 负责整体迁移流程编排：连接数据库 → 预加载旧数据 → 事务写入新数据。

mod backup;
mod convert;
mod playnite;
mod process;

use anyhow::Result;
use sea_orm::prelude::*;
use sea_orm::ActiveValue::NotSet;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set, TransactionTrait};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;

use crate::config::Config;
use crate::db::connection::{connect_new_db, connect_old_db};
use crate::{reina, whitecloud};

use self::convert::{
    build_custom_data_json, build_daily_stats_json, build_launch_fields, resolve_event_duration,
    timestamp_to_date,
};

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
    println!("Reina Migrator - 数据迁移工具");

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
        process::ProcessStatus::AlreadyStopped => {}
        process::ProcessStatus::StoppedAfterPrompt => {
            println!("ReinaManager 已退出，继续迁移。");
        }
        process::ProcessStatus::Cancelled => {
            println!("迁移已取消，目标数据库未修改。");
            pause_before_exit()?;
            return Ok(());
        }
    }

    match &source {
        MigrationSource::Whitecloud => {
            println!("Whitecloud 数据库: {}", Config::old_database_path()?)
        }
        MigrationSource::Playnite { export_path } => {
            println!("Playnite 导出文件: {}", export_path.display())
        }
    }
    println!("ReinaManager 数据库: {}", new_database_path);

    // 3. 连接数据库
    println!("连接数据库...");
    let new_db = connect_new_db(new_database_path).await?;

    if matches!(source, MigrationSource::Playnite { .. }) {
        playnite::validate_target_schema(&new_db).await?;
    }

    // 4. 备份新数据库
    backup::backup_database(&new_db, new_database_path).await?;

    // 5. 执行数据迁移
    println!("开始数据迁移...");
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
    println!("关闭数据库连接...");
    new_db.close().await?;

    println!("🎉 数据迁移完成！");
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
    let old_games = whitecloud::games::Entity::find().all(old_db).await?;
    println!("找到 {} 个游戏需要迁移", old_games.len());

    let preloaded = PreloadedData::load(old_db).await?;
    let txn = new_db.begin().await?;

    for old_game in &old_games {
        println!("迁移游戏: {:?}", old_game.name);

        let (localpath, executable) = build_launch_fields(&old_game.game_dir, &old_game.exe_path);
        let custom_data = build_custom_data_json(&old_game.name)?;

        let new_game = reina::games::ActiveModel {
            id: NotSet,
            id_type: Set("Whitecloud".to_string()),
            date: NotSet,
            localpath: Set(localpath),
            executable: Set(executable),
            launch_type: NotSet,
            steam_launch_id: NotSet,
            savepath: Set(old_game.save_dir.clone()),
            autosave: NotSet,
            maxbackups: NotSet,
            clear: Set(Some(1)),
            le_launch: NotSet,
            magpie: NotSet,
            custom_data: Set(custom_data),
            created_at: NotSet,
            updated_at: NotSet,
        };

        let inserted = new_game.insert(&txn).await?;
        println!("已插入游戏 ID: {}", inserted.id);

        if let Some(uuid) = &old_game.uuid {
            let events = preloaded
                .events_by_game
                .get(uuid.as_str())
                .map(|v| v.as_slice());
            let histories = preloaded
                .histories_by_game
                .get(uuid.as_str())
                .map(|v| v.as_slice());
            migrate_game_sessions(&txn, events, histories, inserted.id).await?;
        }
    }

    txn.commit().await?;
    Ok(())
}

// ─────────────────────────── 会话迁移 ───────────────────────────

/// 迁移指定游戏的所有会话记录，并生成统计信息
async fn migrate_game_sessions<C: ConnectionTrait>(
    db: &C,
    play_events: Option<&[whitecloud::event::Model]>,
    histories: Option<&[whitecloud::history::Model]>,
    new_game_id: i32,
) -> Result<()> {
    let events = match play_events {
        Some(e) if !e.is_empty() => e,
        _ => return Ok(()),
    };

    let mut session_batch = Vec::new();
    let mut total_time = 0i64;
    let mut session_count = 0i32;
    let mut last_played: Option<i64> = None;
    let mut daily_stats: HashMap<String, i64> = HashMap::new();

    for event in events {
        let duration = resolve_event_duration(event, histories);
        if duration <= 0 {
            continue;
        }

        let end_time = (event.time.unwrap() / 1000.0) as i64;
        let start_time = end_time - duration * 60;
        let date = timestamp_to_date(start_time);

        session_batch.push(reina::game_sessions::ActiveModel {
            session_id: Default::default(),
            game_id: Set(new_game_id),
            start_time: Set(start_time as i32),
            end_time: Set(end_time as i32),
            duration: Set(duration as i32),
            date: Set(date.clone()),
        });

        total_time += duration;
        session_count += 1;
        last_played = Some(end_time.max(last_played.unwrap_or(0)));
        *daily_stats.entry(date).or_insert(0) += duration;
    }

    // 批量插入会话
    if !session_batch.is_empty() {
        reina::game_sessions::Entity::insert_many(session_batch)
            .exec(db)
            .await?;
    }

    // 写入统计记录
    if session_count > 0 {
        let stats = reina::game_statistics::ActiveModel {
            game_id: Set(new_game_id),
            total_time: Set(Some(total_time as i32)),
            session_count: Set(Some(session_count)),
            last_played: Set(last_played.map(|t| t as i32)),
            daily_stats: Set(Some(build_daily_stats_json(daily_stats)?)),
        };
        stats.insert(db).await?;
    }

    Ok(())
}
