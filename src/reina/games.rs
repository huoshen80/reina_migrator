use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "games")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,

    // === 迁移写入字段 ===
    #[sea_orm(column_type = "Text")]
    pub id_type: String,
    #[sea_orm(column_type = "Text", nullable)]
    pub localpath: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub executable: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub savepath: Option<String>,
    pub clear: Option<i32>,
    #[sea_orm(column_type = "Text", nullable)]
    pub custom_data: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::game_sessions::Entity")]
    GameSessions,
    #[sea_orm(has_one = "super::game_statistics::Entity")]
    GameStatistics,
}

impl Related<super::game_sessions::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::GameSessions.def()
    }
}

impl Related<super::game_statistics::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::GameStatistics.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::ActiveModel;
    use sea_orm::ActiveValue::{NotSet, Set};
    use sea_orm::{ActiveModelTrait, ConnectionTrait, Database, DatabaseBackend, Statement};

    #[tokio::test]
    async fn inserts_into_the_reina_manager_v025_games_schema() {
        let database = Database::connect("sqlite::memory:").await.unwrap();
        database
            .execute_unprepared(
                r#"
                CREATE TABLE games (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    id_type TEXT NOT NULL,
                    date TEXT,
                    localpath TEXT,
                    savepath TEXT,
                    autosave INTEGER DEFAULT 0,
                    clear INTEGER DEFAULT 0,
                    created_at INTEGER DEFAULT (strftime('%s', 'now')),
                    updated_at INTEGER DEFAULT (strftime('%s', 'now')),
                    custom_data TEXT,
                    maxbackups INTEGER DEFAULT 20,
                    le_launch INTEGER DEFAULT 0,
                    magpie INTEGER DEFAULT 0,
                    user_rating REAL GENERATED ALWAYS AS (
                        CAST(json_extract(custom_data, '$.user_rating') AS REAL)
                    ) VIRTUAL,
                    executable TEXT
                )
                "#,
            )
            .await
            .unwrap();

        ActiveModel {
            id: NotSet,
            id_type: Set("Whitecloud".to_string()),
            localpath: Set(Some(r"D:\Games\Foo".to_string())),
            executable: Set(Some("Foo.exe".to_string())),
            savepath: Set(Some(r"D:\Games\Foo\savedata".to_string())),
            clear: Set(Some(1)),
            custom_data: Set(Some(r#"{"name":"Foo"}"#.to_string())),
        }
        .insert(&database)
        .await
        .unwrap();

        let row = database
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                r#"
                SELECT
                    localpath,
                    executable,
                    json_valid(custom_data) AS custom_data_is_valid,
                    user_rating,
                    autosave,
                    maxbackups,
                    clear,
                    COUNT(*) FILTER (
                        WHERE localpath IS NULL AND executable IS NOT NULL
                    ) OVER () AS orphan_count
                FROM games
                "#
                .to_string(),
            ))
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            row.try_get::<String>("", "localpath").unwrap(),
            r"D:\Games\Foo"
        );
        assert_eq!(row.try_get::<String>("", "executable").unwrap(), "Foo.exe");
        assert_eq!(row.try_get::<i32>("", "custom_data_is_valid").unwrap(), 1);
        assert_eq!(row.try_get::<Option<f64>>("", "user_rating").unwrap(), None);
        assert_eq!(row.try_get::<i32>("", "autosave").unwrap(), 0);
        assert_eq!(row.try_get::<i32>("", "maxbackups").unwrap(), 20);
        assert_eq!(row.try_get::<i32>("", "clear").unwrap(), 1);
        assert_eq!(row.try_get::<i64>("", "orphan_count").unwrap(), 0);
    }
}
