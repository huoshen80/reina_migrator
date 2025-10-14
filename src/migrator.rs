use anyhow::Result;
use chrono::{DateTime, Utc};
use sea_orm::prelude::*;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};
use std::io::{self, Write};
use std::process::Command;

use crate::config::Config;
use crate::db::connection::{connect_new_db, connect_old_db};
use crate::{reina, whitecloud};

pub async fn run_migration() -> Result<()> {
    println!("Reina Migrator - Whitecloud 数据库迁移工具");

    // 1. 检查并关闭 ReinaManager 程序
    if check_and_close_reina_manager()? {
        println!("已关闭 ReinaManager 程序");
    }

    // 2. 获取数据库路径
    let old_database_path = Config::old_database_path()?;
    let new_database_path = Config::new_database_path()?;

    println!("旧数据库: {}", old_database_path);
    println!("新数据库: {}", new_database_path);

    // 3. 连接数据库
    println!("连接数据库...");
    let old_db = connect_old_db(&old_database_path).await?;
    let new_db = connect_new_db(&new_database_path).await?;

    // 4. 备份新数据库
    backup_database(&new_database_path)?;

    // 5. 开始数据迁移
    println!("开始数据迁移...");
    migrate_games(&old_db, &new_db).await?;

    // 6. 关闭数据库连接（确保在提示用户前释放资源）
    println!("关闭数据库连接...");
    // DatabaseConnection::close consumes the connection and is async
    old_db.close().await?;
    new_db.close().await?;

    println!("🎉 数据迁移完成！");
    println!();
    println!("现在您可以重新启动 ReinaManager 查看迁移的数据。");
    println!("按任意键退出...");

    // 等待用户按键
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;

    Ok(())
}

// 检查并关闭 ReinaManager 程序
fn check_and_close_reina_manager() -> Result<bool> {
    // 检查是否有 ReinaManager 进程在运行
    let output = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq ReinaManager.exe"])
        .output()?;

    let output_str = String::from_utf8_lossy(&output.stdout);

    if output_str.contains("ReinaManager.exe") {
        println!("检测到 ReinaManager 程序正在运行。");
        println!("⚠️  重要提醒：请先保存好您在 ReinaManager 中的数据！");
        println!();
        print!("是否关闭 ReinaManager 程序继续迁移？(y/n): ");
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if input.trim().to_lowercase() == "y" || input.trim().to_lowercase() == "yes" {
            println!("正在强制关闭 ReinaManager 程序...");

            // 强制关闭进程
            let result = Command::new("taskkill")
                .args(["/IM", "ReinaManager.exe", "/F", "/T"])
                .output();

            match result {
                Ok(_) => {
                    println!("已发送强制关闭信号，等待程序完全关闭...");

                    // 等待并确认进程完全关闭
                    for i in 1..=10 {
                        std::thread::sleep(std::time::Duration::from_secs(1));

                        let check_output = Command::new("tasklist")
                            .args(["/FI", "IMAGENAME eq ReinaManager.exe"])
                            .output()?;

                        let check_str = String::from_utf8_lossy(&check_output.stdout);
                        if !check_str.contains("ReinaManager.exe") {
                            println!("✅ ReinaManager 程序已完全关闭");
                            return Ok(true);
                        }

                        if i <= 5 {
                            print!("等待中... ({}/10)\r", i);
                            io::stdout().flush()?;
                        } else {
                            println!("程序仍在运行，继续等待... ({}/10)", i);
                        }
                    }

                    // 如果 10 秒后仍未关闭，返回错误
                    return Err(anyhow::anyhow!(
                        "无法完全关闭 ReinaManager 程序，请手动关闭后重试"
                    ));
                }
                Err(e) => {
                    println!("无法关闭程序: {}", e);
                    println!("请手动关闭 ReinaManager 程序后重新运行迁移工具。");
                    return Err(anyhow::anyhow!("无法关闭 ReinaManager 程序"));
                }
            }
        } else {
            println!("已取消迁移。请关闭 ReinaManager 程序后重新运行。");
            return Err(anyhow::anyhow!("用户取消操作"));
        }
    }

    Ok(false)
}

// 备份数据库
fn backup_database(db_path: &str) -> Result<()> {
    // 从 sqlite:path 格式中提取实际路径
    let actual_path = db_path.strip_prefix("sqlite:").unwrap_or(db_path);
    let db_file = std::path::Path::new(actual_path);

    if !db_file.exists() {
        println!("新数据库文件不存在，跳过备份");
        return Ok(());
    }

    // 创建备份目录
    let backup_dir = db_file.parent().unwrap().join("backups");
    std::fs::create_dir_all(&backup_dir)?;

    // 生成备份文件名：reina_manager_2025-08-20T07-47-19-178Z.db
    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S-%3fZ").to_string();
    let backup_filename = format!("reina_manager_{}.db", timestamp);
    let backup_path = backup_dir.join(backup_filename);

    // 复制数据库文件
    std::fs::copy(db_file, &backup_path)?;

    println!("已备份数据库到: {}", backup_path.display());
    Ok(())
}

async fn migrate_games(old_db: &DatabaseConnection, new_db: &DatabaseConnection) -> Result<()> {
    // 获取旧数据库中的所有游戏
    let old_games = whitecloud::games::Entity::find().all(old_db).await?;

    println!("找到 {} 个游戏需要迁移", old_games.len());

    for old_game in old_games {
        println!("迁移游戏: {:?}", old_game.name);

        // 构造本地路径 - 使用反斜杠连接（Windows 路径）
        let localpath =
            if let (Some(game_dir), Some(exe_path)) = (&old_game.game_dir, &old_game.exe_path) {
                Some(format!("{}\\{}", game_dir, exe_path))
            } else if let Some(game_dir) = &old_game.game_dir {
                Some(game_dir.clone())
            } else {
                old_game.exe_path.clone()
            };

        // 创建新的游戏记录
        let new_game = reina::games::ActiveModel {
            id: Default::default(), // 自动生成
            bgm_id: Set(None),
            vndb_id: Set(None),
            id_type: Set("Whitecloud".to_string()),
            date: Set(None),
            localpath: Set(localpath),
            savepath: Set(old_game.save_dir.clone()),
            autosave: Set(Some(0)),
            clear: Set(Some(0)),
            custom_name: Set(None),
            custom_cover: Set(None),
            created_at: Set(Some(chrono::Utc::now().timestamp() as i32)),
            updated_at: Set(Some(chrono::Utc::now().timestamp() as i32)),
        };

        // 插入新游戏记录
        let inserted_game = new_game.insert(new_db).await?;
        println!("已插入游戏 ID: {}", inserted_game.id);

        // 创建 other_data 记录
        let other_data = reina::other_data::ActiveModel {
            game_id: Set(inserted_game.id),
            image: Set(Some("/images/default.png".to_string())),
            name: Set(old_game.name.clone()),
            summary: Set(None),
            tags: Set(None),
            developer: Set(None),
        };

        // 插入 other_data 记录
        other_data.insert(new_db).await?;

        // 如果有历史记录，也迁移游戏会话
        if let Some(uuid) = &old_game.uuid {
            migrate_game_sessions(old_db, new_db, uuid, inserted_game.id).await?;
        }
    }

    Ok(())
}

async fn migrate_game_sessions(
    old_db: &DatabaseConnection,
    new_db: &DatabaseConnection,
    uuid: &str,
    new_game_id: i32,
) -> Result<()> {
    // 获取该游戏的所有 PlayEvent，这些包含实际的游戏时长信息
    let play_events = whitecloud::event::Entity::find()
        .filter(whitecloud::event::Column::Game.eq(uuid))
        .filter(whitecloud::event::Column::EventType.eq("PlayEvent"))
        .all(old_db)
        .await?;

    let mut total_time = 0i64;
    let mut session_count = 0i32;
    let mut last_played: Option<i64> = None;
    let mut daily_stats: std::collections::HashMap<String, i64> = std::collections::HashMap::new();

    for event in &play_events {
        if let Some(time_ms) = event.time {
            let time_s = (time_ms / 1000.0) as i64;

            // 尝试解析 context 中的 playtime
            let duration = if let Some(context) = &event.context {
                match parse_playtime_from_context(context) {
                    Ok(playtime_ms) => playtime_ms / 1000 / 60, // 毫秒转换为分钟
                    Err(_) => {
                        // 如果无法解析 context，尝试从 history 表获取时长
                        get_duration_from_history(old_db, uuid, time_s)
                            .await
                            .unwrap_or(0)
                    }
                }
            } else {
                get_duration_from_history(old_db, uuid, time_s)
                    .await
                    .unwrap_or(0)
            };

            if duration > 0 {
                // 计算会话结束时间 (以分钟为单位)
                let end_time = time_s;
                let start_time = end_time - (duration * 60); // duration是分钟，转换为秒来计算开始时间

                // 转换为日期字符串
                let date = if let Some(datetime) = DateTime::from_timestamp(start_time, 0) {
                    datetime.format("%Y-%m-%d").to_string()
                } else {
                    chrono::Utc::now().format("%Y-%m-%d").to_string()
                };

                let session = reina::game_sessions::ActiveModel {
                    session_id: Default::default(),
                    game_id: Set(new_game_id),
                    start_time: Set(start_time as i32),
                    end_time: Set(end_time as i32),
                    duration: Set(duration as i32), // duration 已经是分钟单位
                    date: Set(date.clone()),
                    created_at: Set(Some(chrono::Utc::now().timestamp() as i32)),
                };

                session.insert(new_db).await?;

                total_time += duration;
                session_count += 1;
                last_played = Some(end_time.max(last_played.unwrap_or(0)));

                // 更新每日统计
                *daily_stats.entry(date).or_insert(0) += duration;
            }
        }
    }

    // 创建游戏统计记录
    if session_count > 0 {
        // 构建 daily_stats JSON
        let daily_stats_json = if daily_stats.is_empty() {
            "{}".to_string()
        } else {
            // 转换为包含 playtime 字段的格式，类似：[{"date":"2025-05-31","playtime":108}]
            let daily_array: Vec<serde_json::Value> = daily_stats
                .into_iter()
                .map(|(date, playtime)| {
                    serde_json::json!({
                        "date": date,
                        "playtime": playtime
                    })
                })
                .collect();
            serde_json::to_string(&daily_array)?
        };

        let stats = reina::game_statistics::ActiveModel {
            game_id: Set(new_game_id),
            total_time: Set(Some(total_time as i32)),
            session_count: Set(Some(session_count)),
            last_played: Set(last_played.map(|t| t as i32)),
            daily_stats: Set(Some(daily_stats_json)),
        };

        stats.insert(new_db).await?;
    }

    Ok(())
}

// 解析 PlayEvent context 中的 playtime
fn parse_playtime_from_context(context: &[u8]) -> Result<i64> {
    let context_str = String::from_utf8(context.to_vec())?;
    let context_json: serde_json::Value = serde_json::from_str(&context_str)?;

    if let Some(playtime) = context_json.get("playtime") {
        if let Some(playtime_num) = playtime.as_i64() {
            return Ok(playtime_num);
        }
    }

    Err(anyhow::anyhow!("No playtime found in context"))
}

// 从 history 表获取时长作为备选方案
async fn get_duration_from_history(
    old_db: &DatabaseConnection,
    uuid: &str,
    target_time: i64,
) -> Result<i64> {
    let histories = whitecloud::history::Entity::find()
        .filter(whitecloud::history::Column::Game.eq(uuid))
        .all(old_db)
        .await?;

    // 找到时间最接近的 history 记录
    for history in histories {
        if let (Some(start_ms), Some(end_ms)) = (history.start, history.end) {
            let start_s = (start_ms / 1000.0) as i64;
            let end_s = (end_ms / 1000.0) as i64;

            // 如果目标时间在这个会话的时间范围内或接近
            if (target_time - end_s).abs() < 300 {
                // 5分钟容差
                return Ok((end_s - start_s) / 60); // 转换为分钟
            }
        }
    }

    Ok(0)
}
