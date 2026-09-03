use anyhow::Result;
use reina_migrator::{config::Config, migrator};
use std::io::{self, Write};

#[derive(Debug, PartialEq, Eq)]
enum TargetChoice {
    Installed,
    Portable,
}

fn parse_target_choice(input: &str) -> Option<TargetChoice> {
    match input.trim() {
        "" | "1" => Some(TargetChoice::Installed),
        "2" => Some(TargetChoice::Portable),
        _ => None,
    }
}

fn select_portable_database() -> Result<Option<String>> {
    loop {
        let mut dialog = rfd::FileDialog::new().set_title("选择 ReinaManager 便携版目录");
        if let Ok(current_dir) = std::env::current_dir() {
            dialog = dialog.set_directory(current_dir);
        }

        let Some(reina_manager_dir) = dialog.pick_folder() else {
            return Ok(None);
        };

        match Config::portable_database_path(&reina_manager_dir) {
            Ok(database_path) => return Ok(Some(database_path)),
            Err(error) => {
                eprintln!("所选目录无效：{error}");
                eprintln!("请选择包含 resources\\data\\reina_manager.db 的 ReinaManager 根目录。");
            }
        }
    }
}

fn select_target_database() -> Result<String> {
    loop {
        println!("请选择 ReinaManager 版本：");
        println!("1. 安装版（默认）");
        println!("2. 便携版");
        print!("请输入选项 [1]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Err(anyhow::anyhow!("标准输入已关闭"));
        }

        match parse_target_choice(&input) {
            Some(TargetChoice::Installed) => match Config::new_database_path() {
                Ok(database_path) => return Ok(database_path),
                Err(error) => eprintln!("无法使用安装版数据库：{error}"),
            },
            Some(TargetChoice::Portable) => {
                if let Some(database_path) = select_portable_database()? {
                    return Ok(database_path);
                }
                println!("已取消目录选择，返回版本菜单。");
            }
            None => eprintln!("无效选项，请输入 1、2，或直接按 Enter。"),
        }

        println!();
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let new_database_path = select_target_database()?;
    migrator::run_migration_to(&new_database_path).await
}

#[cfg(test)]
mod tests {
    use super::{parse_target_choice, TargetChoice};

    #[test]
    fn defaults_to_the_installed_version_for_blank_input() {
        assert_eq!(parse_target_choice("  \n"), Some(TargetChoice::Installed));
    }

    #[test]
    fn parses_the_supported_target_choices() {
        assert_eq!(parse_target_choice("1"), Some(TargetChoice::Installed));
        assert_eq!(parse_target_choice("2"), Some(TargetChoice::Portable));
    }

    #[test]
    fn rejects_an_unknown_target_choice() {
        assert_eq!(parse_target_choice("3"), None);
    }
}
