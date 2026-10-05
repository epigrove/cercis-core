//! 系统钥匙串（口令托管）。
//!
//! 需求 FR-I12 要求"口令进系统钥匙串，项目内不存明文口令"。
//! 本模块把口令交给操作系统保管，**项目文件夹里永远不出现口令**：
//!
//! | 平台 | 后端 | 备注 |
//! |---|---|---|
//! | macOS | `security`（登录钥匙串） | 系统自带 |
//! | Linux | `secret-tool`（libsecret） | 桌面环境自带；服务器上可能没有 |
//! | Windows | 暂不支持 | 走"每次输入口令"或口令文件（`--password-file`） |
//!
//! - 后端不可用时**不报错**，只是退回"每次输入口令"（诚实告知，不静默降级安全性）。
//! - `CERCIS_KEYCHAIN=off` 可显式关闭（CI 与多人共用机器上建议关）。
//! - 测试与自建后端可用 `CERCIS_KEYCHAIN_STORE_CMD` / `_LOAD_CMD` / `_DELETE_CMD`
//!   三个模板覆盖（`{account}` / `{secret}` 占位符，用 `sh -c` 执行）。

use std::io::Write;
use std::process::{Command, Stdio};

use crate::{CercisError, Result};

/// 钥匙串服务名（在系统钥匙串里归到这一项下）。
pub const SERVICE: &str = "cercis";

/// 是否允许使用钥匙串（`CERCIS_KEYCHAIN=off` 关闭）。
pub fn enabled() -> bool {
    match std::env::var("CERCIS_KEYCHAIN") {
        Ok(v) => !(v.eq_ignore_ascii_case("off")
            || v.eq_ignore_ascii_case("none")
            || v.eq_ignore_ascii_case("false")
            || v == "0"),
        Err(_) => true,
    }
}

/// 钥匙串是否可用（当前平台 + 环境开关 + 后端存在）。
pub fn available() -> bool {
    if !enabled() {
        return false;
    }
    if custom_command("CERCIS_KEYCHAIN_LOAD_CMD").is_some() {
        return true;
    }
    platform_available()
}

/// 托管口令（同一个账号重复写会覆盖）。
pub fn store(account: &str, secret: &str) -> Result<()> {
    if !enabled() {
        return Err(CercisError::Invalid(
            "钥匙串已被 CERCIS_KEYCHAIN=off 关闭".into(),
        ));
    }
    if let Some(tpl) = custom_command("CERCIS_KEYCHAIN_STORE_CMD") {
        let cmd = tpl
            .replace("{account}", &shell_quote(account))
            .replace("{secret}", &shell_quote(secret));
        return run_shell_set(&cmd);
    }
    #[cfg(target_os = "macos")]
    {
        run(
            "security",
            &[
                "add-generic-password",
                "-a",
                account,
                "-s",
                SERVICE,
                "-w",
                secret,
                "-U",
            ],
            None,
        )
        .map(|_| ())
    }
    #[cfg(target_os = "linux")]
    {
        run(
            "secret-tool",
            &[
                "store",
                "--label=Cercis 项目口令",
                "service",
                SERVICE,
                "account",
                account,
            ],
            Some(secret),
        )
        .map(|_| ())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (account, secret);
        Err(CercisError::Invalid(
            "当前平台暂不支持系统钥匙串（请用 --password-file 或每次输入口令）".into(),
        ))
    }
}

/// 取回口令；没有就返回 `None`。
pub fn load(account: &str) -> Result<Option<String>> {
    if !enabled() {
        return Ok(None);
    }
    if let Some(tpl) = custom_command("CERCIS_KEYCHAIN_LOAD_CMD") {
        let cmd = tpl.replace("{account}", &shell_quote(account));
        let out = run_shell(&cmd)?;
        let s = out.trim().to_string();
        return Ok(if s.is_empty() { None } else { Some(s) });
    }
    #[cfg(target_os = "macos")]
    {
        match run(
            "security",
            &[
                "find-generic-password",
                "-a",
                account,
                "-s",
                SERVICE,
                "-w",
            ],
            None,
        ) {
            Ok(out) => {
                let s = out.trim().to_string();
                Ok(if s.is_empty() { None } else { Some(s) })
            }
            Err(_) => Ok(None), // 没存过，属于正常情况
        }
    }
    #[cfg(target_os = "linux")]
    {
        match run(
            "secret-tool",
            &["lookup", "service", SERVICE, "account", account],
            None,
        ) {
            Ok(out) => {
                let s = out.trim().to_string();
                Ok(if s.is_empty() { None } else { Some(s) })
            }
            Err(_) => Ok(None),
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = account;
        Ok(None)
    }
}

/// 删掉托管的口令。
pub fn delete(account: &str) -> Result<bool> {
    if !enabled() {
        return Ok(false);
    }
    if let Some(tpl) = custom_command("CERCIS_KEYCHAIN_DELETE_CMD") {
        let cmd = tpl.replace("{account}", &shell_quote(account));
        return Ok(run_shell(&cmd).is_ok());
    }
    #[cfg(target_os = "macos")]
    {
        Ok(run(
            "security",
            &["delete-generic-password", "-a", account, "-s", SERVICE],
            None,
        )
        .is_ok())
    }
    #[cfg(target_os = "linux")]
    {
        Ok(run(
            "secret-tool",
            &["clear", "service", SERVICE, "account", account],
            None,
        )
        .is_ok())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = account;
        Ok(false)
    }
}

/// 钥匙串里的账号名：把成员与项目绑在一起。
pub fn account_for(project: &str, member: &str) -> String {
    format!("{SERVICE}:{project}:{member}")
}

fn custom_command(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

fn platform_available() -> bool {
    #[cfg(target_os = "macos")]
    {
        which("security")
    }
    #[cfg(target_os = "linux")]
    {
        which("secret-tool")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        false
    }
}

/// 找可执行文件在不在 PATH 上（macOS/Linux 用；Windows 走"不可用"分支）。
#[cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
fn which(program: &str) -> bool {
    std::env::var("PATH")
        .map(|path| {
            path.split(':')
                .any(|dir| !dir.is_empty() && std::path::Path::new(dir).join(program).exists())
        })
        .unwrap_or(false)
}

fn run(program: &str, args: &[&str], stdin: Option<&str>) -> Result<String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| CercisError::Invalid(format!("无法调用 {program}：{e}")))?;
    if let Some(text) = stdin {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(text.as_bytes());
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| CercisError::Invalid(format!("{program} 执行失败：{e}")))?;
    if !out.status.success() {
        return Err(CercisError::Invalid(format!(
            "{program} 返回失败：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn run_shell(cmd: &str) -> Result<String> {
    run("sh", &["-c", cmd], None)
}

fn run_shell_set(cmd: &str) -> Result<()> {
    run_shell(cmd).map(|_| ())
}

/// 单引号安全包裹（自定义命令模板用）。
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_switch_disables_everything() {
        std::env::set_var("CERCIS_KEYCHAIN", "off");
        assert!(!enabled());
        assert!(!available());
        assert!(store("a", "b").is_err());
        assert_eq!(load("a").unwrap(), None);
        std::env::remove_var("CERCIS_KEYCHAIN");
    }

    #[test]
    fn account_names_are_project_scoped() {
        let a = account_for("abc123", "alice");
        let b = account_for("abc123", "bob");
        assert_ne!(a, b);
        assert!(a.starts_with("cercis:"));
    }
}
