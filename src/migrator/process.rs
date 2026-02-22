//! 进程管理
//!
//! 检测并关闭 ReinaManager 进程，确保迁移期间数据库不被占用。

use anyhow::Result;
use std::io::{self, Write};
use std::process::Command;

/// 检查 ReinaManager 是否在运行，若在运行则询问用户是否关闭
///
/// 返回 `true` 表示已成功关闭，`false` 表示未检测到进程。
pub fn check_and_close_reina_manager() -> Result<bool> {
    if !is_reina_manager_running()? {
        return Ok(false);
    }

    println!("检测到 ReinaManager 程序正在运行。");
    println!("⚠️  重要提醒：请先保存好您在 ReinaManager 中的数据！");
    println!();

    if !confirm_action("是否关闭 ReinaManager 程序继续迁移？(y/n): ")? {
        return Err(anyhow::anyhow!("用户取消操作"));
    }

    kill_reina_manager()?;
    Ok(true)
}

/// 检查 ReinaManager.exe 是否正在运行
fn is_reina_manager_running() -> Result<bool> {
    let output = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq ReinaManager.exe"])
        .output()?;
    let output_str = String::from_utf8_lossy(&output.stdout);
    Ok(output_str.contains("ReinaManager.exe"))
}

/// 显示提示并读取用户确认（y/yes 返回 true）
fn confirm_action(prompt: &str) -> Result<bool> {
    print!("{}", prompt);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let answer = input.trim().to_lowercase();
    Ok(answer == "y" || answer == "yes")
}

/// 强制终止 ReinaManager 并等待其完全退出
fn kill_reina_manager() -> Result<()> {
    println!("正在强制关闭 ReinaManager 程序...");

    Command::new("taskkill")
        .args(["/IM", "ReinaManager.exe", "/F", "/T"])
        .output()
        .map_err(|e| {
            println!("无法关闭程序: {}", e);
            anyhow::anyhow!("无法关闭 ReinaManager 程序")
        })?;

    println!("已发送强制关闭信号，等待程序完全关闭...");
    wait_for_process_exit()
}

/// 轮询等待进程退出，最多 10 秒
fn wait_for_process_exit() -> Result<()> {
    for i in 1..=10 {
        std::thread::sleep(std::time::Duration::from_secs(1));

        if !is_reina_manager_running()? {
            println!("✅ ReinaManager 程序已完全关闭");
            return Ok(());
        }

        if i <= 5 {
            print!("等待中... ({}/10)\r", i);
            io::stdout().flush()?;
        } else {
            println!("程序仍在运行，继续等待... ({}/10)", i);
        }
    }

    Err(anyhow::anyhow!(
        "无法完全关闭 ReinaManager 程序，请手动关闭后重试"
    ))
}
