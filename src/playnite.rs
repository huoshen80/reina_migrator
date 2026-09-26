//! Reina Exporter 生成的 Playnite 数据格式。

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs;
use std::path::Path;

const EXPORT_FORMAT: &str = "reina-playnite-export";
const EXPORT_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
pub struct ExportDocument {
    pub(crate) format: String,
    pub(crate) version: u32,
    pub exported_at: DateTime<Utc>,
    pub playnite_version: String,
    pub games: Vec<ExportGame>,
}

impl ExportDocument {
    pub fn read(path: &Path) -> Result<Self> {
        let bytes = fs::read(path)
            .with_context(|| format!("无法读取 Playnite 导出文件: {}", path.display()))?;
        let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes);
        let document: Self = serde_json::from_slice(bytes)
            .with_context(|| format!("无法解析 Playnite 导出文件: {}", path.display()))?;

        if document.format != EXPORT_FORMAT {
            return Err(anyhow::anyhow!(
                "不支持的 Playnite 导出格式: {}",
                document.format
            ));
        }
        if document.version != EXPORT_VERSION {
            return Err(anyhow::anyhow!(
                "不支持的 Playnite 导出版本: {}（当前支持 {}）",
                document.version,
                EXPORT_VERSION
            ));
        }

        Ok(document)
    }
}

#[derive(Debug, Deserialize)]
pub struct ExportGame {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub sorting_name: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub developers: Vec<String>,
    #[serde(default)]
    pub publishers: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub age_ratings: Vec<String>,
    #[serde(default)]
    pub user_score: Option<i32>,
    #[serde(default)]
    pub release_date: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub completion_status: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub install_directory: String,
    #[serde(default)]
    pub added: Option<DateTime<Utc>>,
    #[serde(default)]
    pub modified: Option<DateTime<Utc>>,
    #[serde(default)]
    pub playtime_seconds: u64,
    #[serde(default)]
    pub play_count: u64,
    #[serde(default)]
    pub last_activity: Option<DateTime<Utc>>,
    #[serde(default)]
    pub play_action: Option<ExportPlayAction>,
}

#[derive(Debug, Deserialize)]
pub struct ExportPlayAction {
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub working_directory: String,
    #[serde(default)]
    pub arguments: String,
    #[serde(default)]
    pub tracking_mode: i32,
    #[serde(default)]
    pub tracking_path: String,
}

#[cfg(test)]
mod tests {
    use super::ExportDocument;
    use std::fs;
    use uuid::Uuid;

    fn temporary_file() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("reina-playnite-export-{}.json", Uuid::new_v4()))
    }

    #[test]
    fn reads_the_versioned_export_with_a_utf8_bom() {
        let path = temporary_file();
        let json = r#"{
            "format":"reina-playnite-export",
            "version":1,
            "exported_at":"2026-09-03T00:00:00Z",
            "playnite_version":"10.40",
            "games":[{
                "id":"game-id",
                "name":"Game",
                "user_score":null,
                "added":null,
                "modified":null,
                "last_activity":null,
                "play_action":null
            }]
        }"#;
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(json.as_bytes());
        fs::write(&path, bytes).unwrap();

        let document = ExportDocument::read(&path).unwrap();

        assert_eq!(document.playnite_version, "10.40");
        assert_eq!(document.games.len(), 1);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_a_lunabox_style_root_array() {
        let path = temporary_file();
        fs::write(&path, r#"[{"id":"game-id","name":"Game"}]"#).unwrap();

        let error = ExportDocument::read(&path).unwrap_err();

        assert!(error.to_string().contains("无法解析"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_an_unknown_export_version() {
        let path = temporary_file();
        fs::write(
            &path,
            r#"{
                "format":"reina-playnite-export",
                "version":2,
                "exported_at":"2026-09-03T00:00:00Z",
                "playnite_version":"11.0",
                "games":[]
            }"#,
        )
        .unwrap();

        let error = ExportDocument::read(&path).unwrap_err();

        assert!(error.to_string().contains("当前支持 1"));
        fs::remove_file(path).unwrap();
    }
}
