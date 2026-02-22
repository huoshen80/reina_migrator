//! 数据转换工具函数
//!
//! 纯函数，负责 Whitecloud 旧数据 → Reina 新数据的字段映射与格式转换。

use anyhow::Result;
use chrono::DateTime;
use std::collections::HashMap;

use crate::whitecloud;

// ─────────────────────────── 路径 & JSON 构建 ───────────────────────────

/// 拼接 Windows 本地路径（`game_dir\exe_path`）
pub fn build_localpath(game_dir: &Option<String>, exe_path: &Option<String>) -> Option<String> {
    match (game_dir, exe_path) {
        (Some(dir), Some(exe)) => Some(format!("{}\\{}", dir, exe)),
        (Some(dir), None) => Some(dir.clone()),
        (None, exe) => exe.clone(),
    }
}

/// 将游戏名称封装为 `custom_data` JSON 字符串
pub fn build_custom_data_json(name: &Option<String>) -> Result<Option<String>> {
    let mut data = serde_json::Map::new();
    if let Some(n) = name {
        data.insert("name".to_string(), serde_json::Value::String(n.clone()));
    }
    if data.is_empty() {
        Ok(None)
    } else {
        Ok(Some(serde_json::to_string(&data)?))
    }
}

/// 将每日统计 HashMap 序列化为 JSON 数组
///
/// 输出格式：`[{"date":"2025-05-31","playtime":108}, ...]`
pub fn build_daily_stats_json(daily_stats: HashMap<String, i64>) -> Result<String> {
    if daily_stats.is_empty() {
        return Ok("{}".to_string());
    }
    let daily_array: Vec<serde_json::Value> = daily_stats
        .into_iter()
        .map(|(date, playtime)| serde_json::json!({ "date": date, "playtime": playtime }))
        .collect();
    Ok(serde_json::to_string(&daily_array)?)
}

// ─────────────────────────── 时间 & 时长 ───────────────────────────

/// Unix 时间戳 → `YYYY-MM-DD` 日期字符串
pub fn timestamp_to_date(timestamp: i64) -> String {
    DateTime::from_timestamp(timestamp, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string())
}

/// 解析单条事件的游戏时长（分钟）
///
/// 优先从 event.context 中提取 playtime，失败则回退到 history 时间匹配。
pub fn resolve_event_duration(
    event: &whitecloud::event::Model,
    histories: Option<&[whitecloud::history::Model]>,
) -> i64 {
    let time_ms = match event.time {
        Some(t) => t,
        None => return 0,
    };
    let time_s = (time_ms / 1000.0) as i64;

    // 优先从 context JSON 解析
    if let Some(context) = &event.context {
        if let Ok(playtime_ms) = parse_playtime_from_context(context) {
            return playtime_ms / 1000 / 60; // 毫秒 → 分钟
        }
    }

    // 回退：在 history 记录中按时间窗口匹配
    find_duration_from_histories(histories, time_s)
}

// ─────────────────────────── 内部辅助 ───────────────────────────

/// 从 PlayEvent context 字节中提取 playtime 字段（毫秒）
fn parse_playtime_from_context(context: &[u8]) -> Result<i64> {
    let context_str = String::from_utf8(context.to_vec())?;
    let json: serde_json::Value = serde_json::from_str(&context_str)?;
    json.get("playtime")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow::anyhow!("No playtime found in context"))
}

/// 在 history 列表中找到与 target_time 最匹配的记录，返回时长（分钟）
fn find_duration_from_histories(
    histories: Option<&[whitecloud::history::Model]>,
    target_time: i64,
) -> i64 {
    let histories = match histories {
        Some(h) => h,
        None => return 0,
    };
    for history in histories {
        if let (Some(start_ms), Some(end_ms)) = (history.start, history.end) {
            let start_s = (start_ms / 1000.0) as i64;
            let end_s = (end_ms / 1000.0) as i64;
            // 5 分钟容差窗口
            if (target_time - end_s).abs() < 300 {
                return (end_s - start_s) / 60;
            }
        }
    }
    0
}
