//! 两种迁移来源共用的游戏判重和空统计补全规则。

use anyhow::Result;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, QueryFilter, Set,
};
use std::collections::{HashMap, HashSet};

use crate::reina;

#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub enum GameIdentity {
    Steam(String),
    Local(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum MatchResult {
    Missing,
    Unique(i32),
    Ambiguous,
}

#[derive(Default)]
pub struct TargetIndex {
    games_by_identity: HashMap<GameIdentity, HashSet<i32>>,
}

impl TargetIndex {
    pub async fn load(db: &impl ConnectionTrait) -> Result<Self> {
        let mut index = Self::default();
        for game in reina::games::Entity::find().all(db).await? {
            index.insert(
                game.id,
                &identities(
                    game.steam_launch_id.as_deref(),
                    game.localpath.as_deref(),
                    game.executable.as_deref(),
                ),
            );
        }
        Ok(index)
    }

    pub fn insert(&mut self, game_id: i32, keys: &[GameIdentity]) {
        for key in keys {
            self.games_by_identity
                .entry(key.clone())
                .or_default()
                .insert(game_id);
        }
    }

    pub fn find(&self, keys: &[GameIdentity]) -> MatchResult {
        let ids: HashSet<i32> = keys
            .iter()
            .filter_map(|key| self.games_by_identity.get(key))
            .flat_map(|ids| ids.iter().copied())
            .collect();
        match ids.len() {
            0 => MatchResult::Missing,
            1 => MatchResult::Unique(*ids.iter().next().expect("恰好存在一个目标 ID")),
            _ => MatchResult::Ambiguous,
        }
    }
}

pub fn identities(
    steam_id: Option<&str>,
    localpath: Option<&str>,
    executable: Option<&str>,
) -> Vec<GameIdentity> {
    let mut keys = Vec::new();
    if let Some(steam_id) = steam_id.and_then(normalize_steam_id) {
        keys.push(GameIdentity::Steam(steam_id));
    }
    if let (Some(localpath), Some(executable)) = (
        localpath.and_then(non_empty),
        executable.and_then(non_empty),
    ) {
        let directory = normalize_path(localpath);
        if directory != "." && !directory.is_empty() {
            let executable = normalize_path(executable);
            let absolute = executable.starts_with("\\\\")
                || (executable.as_bytes().get(1) == Some(&b':')
                    && executable.as_bytes().get(2) == Some(&b'\\'));
            let path = if absolute {
                executable
            } else {
                format!("{directory}\\{executable}")
            };
            keys.push(GameIdentity::Local(path));
        }
    }
    keys
}

pub fn normalize_steam_id(value: &str) -> Option<String> {
    value
        .trim()
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
}

pub fn normalize_path(value: &str) -> String {
    value
        .trim()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

fn non_empty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

pub struct ImportedStatistics {
    pub total_time: i32,
    pub session_count: i32,
    pub last_played: Option<i32>,
    pub daily_stats: String,
}

pub enum StatisticsSlot {
    Vacant,
    Zero(reina::game_statistics::Model),
    Occupied,
}

impl StatisticsSlot {
    pub async fn load(db: &impl ConnectionTrait, game_id: i32) -> Result<Self> {
        let existing = reina::game_statistics::Entity::find_by_id(game_id)
            .one(db)
            .await?;
        if existing.as_ref().is_some_and(|stats| {
            stats.total_time.unwrap_or(0) != 0 || stats.session_count.unwrap_or(0) != 0
        }) {
            return Ok(Self::Occupied);
        }
        if reina::game_sessions::Entity::find()
            .filter(reina::game_sessions::Column::GameId.eq(game_id))
            .one(db)
            .await?
            .is_some()
        {
            return Ok(Self::Occupied);
        }
        Ok(match existing {
            Some(stats) => Self::Zero(stats),
            None => Self::Vacant,
        })
    }

    pub fn is_available(&self) -> bool {
        !matches!(self, Self::Occupied)
    }

    pub async fn write(
        self,
        db: &impl ConnectionTrait,
        game_id: i32,
        imported: ImportedStatistics,
    ) -> Result<bool> {
        let mut active = reina::game_statistics::ActiveModel {
            game_id: Set(game_id),
            total_time: Set(Some(imported.total_time)),
            session_count: Set(Some(imported.session_count)),
            last_played: Set(imported.last_played),
            daily_stats: Set(Some(imported.daily_stats)),
        };
        match self {
            Self::Vacant => {
                active.insert(db).await?;
                Ok(true)
            }
            Self::Zero(existing) => {
                if existing.last_played.is_some() {
                    active.last_played = Set(existing.last_played);
                }
                let mut previous = existing.into_active_model();
                previous.total_time = active.total_time;
                previous.session_count = active.session_count;
                previous.last_played = active.last_played;
                previous.daily_stats = active.daily_stats;
                previous.update(db).await?;
                Ok(true)
            }
            Self::Occupied => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{identities, MatchResult, TargetIndex};

    #[test]
    fn matches_both_steam_and_local_keys_but_not_names_or_incomplete_paths() {
        let keys = identities(Some("000730"), Some(r"D:/Games/Foo/"), Some("Foo.exe"));
        let mut index = TargetIndex::default();
        index.insert(10, &keys);

        assert_eq!(
            index.find(&identities(Some("730"), None, None)),
            MatchResult::Unique(10)
        );
        assert_eq!(
            index.find(&identities(None, Some(r"d:\games\foo"), Some("foo.EXE"))),
            MatchResult::Unique(10)
        );
        assert_eq!(
            index.find(&identities(None, Some(r"D:\Games\Foo"), None)),
            MatchResult::Missing
        );
        assert!(identities(None, Some("."), Some("Foo.exe")).is_empty());
    }

    #[test]
    fn reports_ambiguous_matches() {
        let key = identities(None, Some(r"D:\Games\Foo"), Some("Foo.exe"));
        let mut index = TargetIndex::default();
        index.insert(10, &key);
        index.insert(11, &key);
        assert_eq!(index.find(&key), MatchResult::Ambiguous);

        let mut index = TargetIndex::default();
        index.insert(10, &identities(Some("730"), None, None));
        index.insert(11, &key);
        assert_eq!(
            index.find(&identities(
                Some("730"),
                Some(r"D:\Games\Foo"),
                Some("Foo.exe")
            )),
            MatchResult::Ambiguous
        );
    }

    #[test]
    fn matches_the_same_executable_with_different_directory_splits() {
        let nested = identities(None, Some(r"D:\Games"), Some(r"Bin\Foo.exe"));
        let split = identities(None, Some(r"d:\games\bin"), Some("foo.exe"));
        let absolute = identities(None, Some(r"D:\Games"), Some(r"D:\Games\Bin\Foo.exe"));
        assert_eq!(nested, split);
        assert_eq!(nested, absolute);
    }
}
