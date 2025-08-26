use sea_orm::entity::prelude::*;

// 新数据库的 games 表模型
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "games")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub bgm_id: Option<String>,
    pub vndb_id: Option<String>,
    pub id_type: String,
    pub date: Option<String>,
    pub image: Option<String>,
    pub summary: Option<String>,
    pub name: Option<String>,
    pub name_cn: Option<String>,
    pub tags: Option<String>,
    pub rank: Option<i32>,
    pub score: Option<f64>,
    pub time: Option<String>,
    pub localpath: Option<String>,
    pub developer: Option<String>,
    pub all_titles: Option<String>,
    pub aveage_hours: Option<f64>,
    pub clear: Option<i32>,
    pub savepath: Option<String>,
    pub autosave: Option<i32>,
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
