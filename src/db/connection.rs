use sea_orm::{Database, DatabaseConnection, DbErr};

pub async fn connect_old_db(database_url: &str) -> Result<DatabaseConnection, DbErr> {
    Database::connect(database_url).await
}

pub async fn connect_new_db(database_url: &str) -> Result<DatabaseConnection, DbErr> {
    Database::connect(database_url).await
}