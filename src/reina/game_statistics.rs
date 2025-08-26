use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "game_statistics")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub game_id: i32,
    pub total_time: Option<i64>,
    pub session_count: Option<i32>,
    pub last_played: Option<i64>,
    pub daily_stats: Option<String>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::games::Entity",
        from = "Column::GameId",
        to = "super::games::Column::Id"
    )]
    Games,
}

impl Related<super::games::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Games.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
