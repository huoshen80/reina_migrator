use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "games")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub name: Option<String>,
    #[sea_orm(column_name = "gameDir")]
    pub game_dir: Option<String>,
    #[sea_orm(column_name = "saveDir")]
    pub save_dir: Option<String>,
    #[sea_orm(column_name = "exePath")]
    pub exe_path: Option<String>,
    pub state: Option<u32>,
    pub uuid: Option<String>,
    #[sea_orm(column_name = "updateTime")]
    pub update_time: Option<f64>,
    #[sea_orm(column_name = "order")]
    pub order: Option<f64>,
    #[sea_orm(column_name = "nativeSaveNumber")]
    pub native_save_number: Option<f64>,
    #[sea_orm(column_name = "startWithStrategy")]
    pub start_with_strategy: Option<u32>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
