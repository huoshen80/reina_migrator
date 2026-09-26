use anyhow::Result;
use reina_migrator::{
    config::Config,
    logging,
    migrator::{self, MigrationSource},
};
use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Debug, PartialEq, Eq)]
enum SourceChoice {
    Whitecloud,
    Playnite,
}

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

fn parse_source_choice(input: &str) -> Option<SourceChoice> {
    match input.trim() {
        "" | "1" => Some(SourceChoice::Whitecloud),
        "2" => Some(SourceChoice::Playnite),
        _ => None,
    }
}

fn select_migration_source() -> Result<MigrationSource> {
    loop {
        println!("请选择迁移来源：");
        println!("1. Whitecloud（默认）");
        println!("2. Playnite");
        print!("请输入选项 [1]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Err(anyhow::anyhow!("标准输入已关闭"));
        }

        match parse_source_choice(&input) {
            Some(SourceChoice::Whitecloud) => return Ok(MigrationSource::Whitecloud),
            Some(SourceChoice::Playnite) => {
                let dialog = rfd::FileDialog::new()
                    .set_title("选择 Reina Exporter 导出的 Playnite JSON")
                    .add_filter("Reina Playnite JSON", &["json"]);
                if let Some(export_path) = dialog.pick_file() {
                    return Ok(MigrationSource::Playnite { export_path });
                }
                println!("已取消文件选择，返回来源菜单。");
            }
            None => eprintln!("无效选项，请输入 1、2，或直接按 Enter。"),
        }

        println!();
    }
}

fn select_portable_database() -> Result<Option<String>> {
    println!("请找到便携版的 ReinaManager.exe，选择它所在的文件夹。");
    io::stdout().flush()?;
    let mut dialog_directory = std::env::current_dir().ok();

    loop {
        let mut dialog = rfd::FileDialog::new().set_title("选择 ReinaManager.exe 所在的文件夹");
        if let Some(directory) = &dialog_directory {
            dialog = dialog.set_directory(directory);
        }

        let Some(reina_manager_dir) = dialog.pick_folder() else {
            return Ok(None);
        };

        match Config::portable_database_path(&reina_manager_dir) {
            Ok(database_path) => {
                println!("已找到便携版 ReinaManager 数据。");
                return Ok(Some(database_path));
            }
            Err(_) => {
                tracing::warn!(folder = %reina_manager_dir.display(), "未找到便携版 ReinaManager 数据");
                dialog_directory = reina_manager_dir.parent().map(ToOwned::to_owned);
                eprintln!("所选文件夹：{}", reina_manager_dir.display());
                eprintln!("未找到便携版 ReinaManager 的数据。");
                eprintln!("请确认已启动过一次便携版，并选择 ReinaManager.exe 所在的文件夹。");
                loop {
                    print!("按 Enter 重新选择，输入 0 返回版本菜单: ");
                    io::stdout().flush()?;

                    let mut input = String::new();
                    if io::stdin().read_line(&mut input)? == 0 {
                        return Err(anyhow::anyhow!("标准输入已关闭"));
                    }
                    match input.trim() {
                        "" => break,
                        "0" => return Ok(None),
                        _ => eprintln!("无效选项，请按 Enter 重新选择或输入 0 返回。"),
                    }
                }
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
                Err(error) => {
                    tracing::warn!(details = %error, "无法使用安装版数据库");
                    eprintln!("无法使用安装版数据库：{error}");
                }
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
async fn main() -> ExitCode {
    let log_path = match logging::initialize() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("无法启动迁移日志：{error:#}");
            pause_after_failure();
            return ExitCode::FAILURE;
        }
    };
    println!("本次运行日志：{}", log_path.display());
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "迁移器启动");

    match run().await {
        Ok(()) => {
            tracing::info!("迁移器正常结束");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(details = %format!("{error:#}"), "迁移失败");
            eprintln!("迁移失败：{error:#}");
            eprintln!("本次运行日志：{}", log_path.display());
            pause_after_failure();
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    let source = select_migration_source()?;
    let new_database_path = select_target_database()?;
    migrator::run_migration_from_to(source, &new_database_path).await
}

fn pause_after_failure() {
    println!("按 Enter 退出...");
    let mut input = String::new();
    let _ = io::stdin().read_line(&mut input);
}

#[cfg(test)]
mod tests {
    use super::{parse_source_choice, parse_target_choice, SourceChoice, TargetChoice};

    #[test]
    fn defaults_to_whitecloud_for_blank_source_input() {
        assert_eq!(parse_source_choice("  \n"), Some(SourceChoice::Whitecloud));
    }

    #[test]
    fn parses_the_supported_source_choices() {
        assert_eq!(parse_source_choice("1"), Some(SourceChoice::Whitecloud));
        assert_eq!(parse_source_choice("2"), Some(SourceChoice::Playnite));
        assert_eq!(parse_source_choice("3"), None);
    }

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
