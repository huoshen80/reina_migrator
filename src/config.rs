use anyhow::Result;

pub struct Config;

impl Config {
    pub fn old_database_path() -> Result<String> {
        let current_dir = std::env::current_dir()?;
        let db_path = current_dir.join("db.3.sqlite");
        Ok(format!("sqlite:{}", db_path.display()))
    }

    pub fn new_database_path() -> Result<String> {
        let home_dir = dirs::home_dir()
            .ok_or_else(|| anyhow::anyhow!("无法获取用户主目录"))?;
        
        let appdata_dir = home_dir
            .join("AppData")
            .join("Roaming")
            .join("com.reinamanager.dev")
            .join("data");
        
        // 确保目录存在
        std::fs::create_dir_all(&appdata_dir)?;
        
        let db_path = appdata_dir.join("reina_manager.db");
        Ok(format!("sqlite:{}", db_path.display()))
    }
}
