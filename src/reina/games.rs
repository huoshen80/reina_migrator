use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "games")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,

    // === 外部 ID ===
    #[sea_orm(column_type = "Text", nullable)]
    pub bgm_id: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub vndb_id: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub ymgal_id: Option<String>,
    #[sea_orm(column_type = "Text")]
    pub id_type: String,

    // === 核心状态 ===
    #[sea_orm(column_type = "Text", nullable)]
    pub date: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub localpath: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub savepath: Option<String>,
    pub autosave: Option<i32>,
    pub maxbackups: Option<i32>,
    pub clear: Option<i32>,
    pub le_launch: Option<i32>,
    pub magpie: Option<i32>,

    // === JSON 元数据列 ===
    #[sea_orm(column_type = "Text", nullable)]
    pub vndb_data: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub bgm_data: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub ymgal_data: Option<String>,
    #[sea_orm(column_type = "Text", nullable)]
    pub custom_data: Option<String>,

    // === 时间戳 ===
    pub created_at: Option<i32>,
    pub updated_at: Option<i32>,
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
