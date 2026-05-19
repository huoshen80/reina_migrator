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
