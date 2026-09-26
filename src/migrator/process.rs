//! 进程管理
//!
//! 等待用户自行关闭 ReinaManager，确保迁移期间数据库不被占用。

use anyhow::{bail, Result};
use std::io::{self, BufRead, Write};
use std::process::Command;

#[derive(Debug, PartialEq, Eq)]
pub enum ProcessStatus {
    AlreadyStopped,
    StoppedAfterPrompt,
    Cancelled,
}

/// 检查 ReinaManager 是否在运行，必要时等待用户手动关闭。
pub fn wait_for_reina_manager_exit() -> Result<ProcessStatus> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    wait_for_exit(&mut stdin.lock(), &mut stdout, is_reina_manager_running)
}

fn wait_for_exit<R: BufRead, W: Write>(
    input: &mut R,
    output: &mut W,
    mut is_running: impl FnMut() -> Result<bool>,
) -> Result<ProcessStatus> {
    if !is_running()? {
        return Ok(ProcessStatus::AlreadyStopped);
    }

    writeln!(output, "检测到 ReinaManager 正在运行。")?;
    writeln!(output, "请先保存数据并手动退出 ReinaManager。")?;

    loop {
        write!(output, "按 Enter 重新检测，输入 0 取消迁移: ")?;
        output.flush()?;

        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            bail!("标准输入已关闭，无法确认 ReinaManager 是否退出");
        }

        match answer.trim() {
            "" => {
                if !is_running()? {
                    return Ok(ProcessStatus::StoppedAfterPrompt);
                }
                writeln!(output, "ReinaManager 仍在运行，请关闭后重试。")?;
            }
            "0" => return Ok(ProcessStatus::Cancelled),
            _ => writeln!(output, "无效选项，请按 Enter 重新检测或输入 0 取消。")?,
        }
    }
}

/// 检查 ReinaManager.exe 是否正在运行。
fn is_reina_manager_running() -> Result<bool> {
    let output = Command::new("tasklist")
        .args(["/FI", "IMAGENAME eq ReinaManager.exe"])
        .output()?;
    if !output.status.success() {
        bail!("无法检查 ReinaManager 是否正在运行");
    }

    let output_str = String::from_utf8_lossy(&output.stdout);
    Ok(output_str.contains("ReinaManager.exe"))
}

#[cfg(test)]
mod tests {
    use super::{wait_for_exit, ProcessStatus};
    use std::io::Cursor;

    #[test]
    fn retries_until_the_user_closes_reina_manager() {
        let mut input = Cursor::new("\n\n");
        let mut output = Vec::new();
        let mut checks = [true, true, false].into_iter();

        let status = wait_for_exit(&mut input, &mut output, || Ok(checks.next().unwrap())).unwrap();

        assert_eq!(status, ProcessStatus::StoppedAfterPrompt);
        assert!(String::from_utf8(output).unwrap().contains("仍在运行"));
    }

    #[test]
    fn cancellation_does_not_check_or_close_the_process_again() {
        let mut input = Cursor::new("0\n");
        let mut output = Vec::new();
        let mut checks = [true].into_iter();

        let status = wait_for_exit(&mut input, &mut output, || Ok(checks.next().unwrap())).unwrap();

        assert_eq!(status, ProcessStatus::Cancelled);
    }

    #[test]
    fn closed_input_does_not_retry_forever() {
        let mut input = Cursor::new("");
        let mut output = Vec::new();

        let error = wait_for_exit(&mut input, &mut output, || Ok(true)).unwrap_err();

        assert!(error.to_string().contains("标准输入已关闭"));
    }
}
