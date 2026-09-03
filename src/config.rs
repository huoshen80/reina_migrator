use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct Config;

impl Config {
    pub fn old_database_path() -> Result<String> {
        let current_dir = std::env::current_dir()?;
        let db_path = current_dir.join("db.3.sqlite");
        Ok(format!("sqlite:{}", db_path.display()))
    }

    pub fn new_database_path() -> Result<String> {
        let home_dir = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("无法获取用户主目录"))?;
        let db_path = Self::installed_database_path_from_home(&home_dir)?;
        Ok(format!("sqlite:{}", db_path.display()))
    }

    fn installed_database_path_from_home(home_dir: &Path) -> Result<PathBuf> {
        let db_path = home_dir
            .join("AppData")
            .join("Roaming")
            .join("com.reinamanager.dev")
            .join("data")
            .join("reina_manager.db");

        Self::validate_database_file(db_path)
    }

    fn validate_database_file(db_path: PathBuf) -> Result<PathBuf> {
        if !db_path.is_file() {
            return Err(anyhow::anyhow!(
                "ReinaManager 数据库不存在或不是文件: {}",
                db_path.display()
            ));
        }

        Ok(db_path)
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use std::fs;

    fn temporary_home() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("reina-migrator-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn rejects_a_missing_installed_database_without_creating_directories() {
        let home_dir = temporary_home();

        let error = Config::installed_database_path_from_home(&home_dir).unwrap_err();

        assert!(error.to_string().contains("reina_manager.db"));
        assert!(!home_dir.exists());
    }

    #[test]
    fn rejects_an_installed_database_path_that_is_a_directory() {
        let home_dir = temporary_home();
        let db_path = home_dir
            .join("AppData")
            .join("Roaming")
            .join("com.reinamanager.dev")
            .join("data")
            .join("reina_manager.db");
        fs::create_dir_all(&db_path).unwrap();

        let error = Config::installed_database_path_from_home(&home_dir).unwrap_err();

        assert!(error.to_string().contains("不存在或不是文件"));
        fs::remove_dir_all(home_dir).unwrap();
    }

    #[test]
    fn accepts_an_existing_installed_database_file() {
        let home_dir = temporary_home();
        let db_path = home_dir
            .join("AppData")
            .join("Roaming")
            .join("com.reinamanager.dev")
            .join("data")
            .join("reina_manager.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        fs::File::create(&db_path).unwrap();

        let actual = Config::installed_database_path_from_home(&home_dir).unwrap();

        assert_eq!(actual, db_path);
        fs::remove_dir_all(home_dir).unwrap();
    }
}
