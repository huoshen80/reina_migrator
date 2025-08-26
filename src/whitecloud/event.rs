use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "events")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub game: Option<String>,
    pub state: Option<String>,  // 修改为 String 类型
    pub context: Option<Vec<u8>>,
    pub time: Option<f64>,
    pub host: Option<String>,
    #[sea_orm(column_name = "type")]
    pub event_type: Option<String>,
    pub server_id: Option<f64>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
