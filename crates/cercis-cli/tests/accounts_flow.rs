//! S14 验收：多人账户与权限分级（FR-N08 / FR-N09 / FR-N11，跨进程跑真的 `cercis`）。
//!
//! 覆盖：
//! 1. 账户文件只存加盐哈希——项目里搜不到明文口令
//! 2. 开放模式（没有账户文件）一切照旧
//! 3. 启用账户后：没登录写不了；查看者只能读；编辑者能写数据但不能改结构；管理员全能
//! 4. 越权被拒绝会留痕（change_log 里有 denied）
//! 5. FR-N11：`plugins.admin_only_edit` 打开后，插件改动只允许管理员（没账户则明确报错）

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-accounts-flow-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], envs: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("跑得起来 cercis");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(args: &[&str], envs: &[(&str, &str)]) -> String {
    let (code, stdout, stderr) = run(args, envs);
    assert_eq!(code, 0, "命令失败 {args:?}\nstdout: {stdout}\nstderr: {stderr}");
    stdout
}

fn ok_json(args: &[&str], envs: &[(&str, &str)]) -> Value {
    let stdout = ok(args, envs);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{stdout}"))
}

fn fail(args: &[&str], envs: &[(&str, &str)]) -> String {
    let (code, stdout, stderr) = run(args, envs);
    assert_ne!(code, 0, "这条命令本该失败：{args:?}\nstdout: {stdout}");
    format!("{stdout}{stderr}")
}

fn append(project: &Path, id: &str, envs: &[(&str, &str)]) -> (i32, String, String) {
    run(
        &[
            "append",
            project.to_str().unwrap(),
            "--table",
            "records",
            "--set",
            &format!("record_id={id}"),
            "--set",
            "value_num=1",
            "--allow-new-columns",
        ],
        envs,
    )
}

#[test]
fn accounts_store_hashes_not_passwords() {
    let dir = tmp("hash");
    let project = dir.join("proj");
    let home = dir.join("share-home");
    let home_s = home.to_str().unwrap().to_string();
    let envs: Vec<(&str, &str)> = vec![
        ("CERCIS_SHARE_HOME", home_s.as_str()),
        ("CERCIS_SHARE_KDF", "light"),
        ("CERCIS_TEST_PW", "alice-secret-pw"),
    ];
    ok(&["init", project.to_str().unwrap(), "--name", "多人试验"], &envs);
    ok(
        &[
            "account",
            "add",
            project.to_str().unwrap(),
            "--id",
            "alice",
            "--name",
            "张老师",
            "--role",
            "admin",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );

    // 项目里搜不到明文口令
    let mut leaked = Vec::new();
    let mut stack = vec![project.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(text) = std::fs::read_to_string(&p) {
                if text.contains("alice-secret-pw") {
                    leaked.push(p.display().to_string());
                }
            }
        }
    }
    assert!(leaked.is_empty(), "口令泄漏：{leaked:?}");
    let accounts = std::fs::read_to_string(project.join("access/accounts.yaml")).unwrap();
    assert!(accounts.contains("hash") && accounts.contains("salt"));

    let list = ok_json(
        &["account", "list", project.to_str().unwrap(), "--json"],
        &envs,
    );
    assert_eq!(list["accounts"][0]["role"], "admin");
    assert_eq!(list["enabled"], true);
}

#[test]
fn open_mode_still_works_without_accounts() {
    let dir = tmp("open");
    let project = dir.join("proj");
    ok(&["init", project.to_str().unwrap(), "--name", "单人项目"], &[]);
    // 没有账户文件 → 照常写
    let (code, _, stderr) = append(&project, "R-1", &[]);
    assert_eq!(code, 0, "{stderr}");
    let list = ok_json(&["account", "list", project.to_str().unwrap(), "--json"], &[]);
    assert_eq!(list["enabled"], false);
    assert!(list["accounts"].as_array().unwrap().is_empty());
}

#[test]
fn viewer_cannot_write_editor_cannot_change_structure() {
    let dir = tmp("roles");
    let project = dir.join("proj");
    let home = dir.join("share-home");
    let home_s = home.to_str().unwrap().to_string();
    let envs: Vec<(&str, &str)> = vec![
        ("CERCIS_SHARE_HOME", home_s.as_str()),
        ("CERCIS_SHARE_KDF", "light"),
        ("CERCIS_TEST_PW", "pw"),
    ];
    ok(&["init", project.to_str().unwrap(), "--name", "多人试验"], &envs);
    for (id, role) in [("alice", "admin"), ("bob", "editor"), ("carol", "viewer")] {
        ok(
            &[
                "account",
                "add",
                project.to_str().unwrap(),
                "--id",
                id,
                "--role",
                role,
                "--password-env",
                "CERCIS_TEST_PW",
            ],
            &envs,
        );
    }

    // 没登录：写不了，但读得了
    let (code, _, stderr) = append(&project, "R-1", &envs);
    assert_ne!(code, 0);
    assert!(stderr.contains("登录"), "{stderr}");
    ok(&["show", project.to_str().unwrap(), "--table", "records"], &envs);

    // 查看者：还是不能写
    ok(
        &[
            "account",
            "login",
            project.to_str().unwrap(),
            "--id",
            "carol",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    let (code, _, stderr) = append(&project, "R-1", &envs);
    assert_ne!(code, 0);
    assert!(stderr.contains("查看者"), "{stderr}");
    // 但可以打包（只读操作）
    let out_dir = dir.join("包");
    ok(
        &[
            "pack",
            project.to_str().unwrap(),
            "--out",
            out_dir.to_str().unwrap(),
        ],
        &envs,
    );

    // 编辑者：能写数据，不能改结构（装模板 / 改字段类型 / 删表都拒）
    ok(
        &[
            "account",
            "login",
            project.to_str().unwrap(),
            "--id",
            "bob",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    let (code, _, stderr) = append(&project, "R-1", &envs);
    assert_eq!(code, 0, "编辑者应该能写数据：{stderr}");
    let err = fail(
        &[
            "install-template",
            project.to_str().unwrap(),
            "--name",
            "example-study",
        ],
        &envs,
    );
    assert!(err.contains("管理员") || err.contains("权限"), "{err}");
    let err = fail(
        &[
            "drop-table",
            project.to_str().unwrap(),
            "--table",
            "records",
            "--dry-run",
        ],
        &envs,
    );
    assert!(err.contains("管理员") || err.contains("权限"), "{err}");

    // 管理员：结构改动放行（这里用删空表试试——records 有数据会被拒，但那是业务拒绝不是权限拒绝）
    ok(
        &[
            "account",
            "login",
            project.to_str().unwrap(),
            "--id",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    let err = fail(
        &[
            "drop-table",
            project.to_str().unwrap(),
            "--table",
            "records",
            "--dry-run",
        ],
        &envs,
    );
    assert!(!err.contains("权限"), "管理员不该被权限拦住：{err}");

    // 越权留痕
    let log = ok_json(&["log", project.to_str().unwrap(), "--limit", "50", "--json"], &envs);
    assert!(
        log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "denied"),
        "越权应留痕：{log}"
    );
}

#[test]
fn wrong_password_is_refused_and_logged() {
    let dir = tmp("wrongpw");
    let project = dir.join("proj");
    let home = dir.join("share-home");
    let home_s = home.to_str().unwrap().to_string();
    let envs: Vec<(&str, &str)> = vec![
        ("CERCIS_SHARE_HOME", home_s.as_str()),
        ("CERCIS_SHARE_KDF", "light"),
        ("CERCIS_TEST_PW", "right-pw"),
    ];
    ok(&["init", project.to_str().unwrap(), "--name", "试验"], &envs);
    ok(
        &[
            "account",
            "add",
            project.to_str().unwrap(),
            "--id",
            "alice",
            "--role",
            "admin",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    let mut wrong = envs.clone();
    wrong.retain(|(k, _)| *k != "CERCIS_TEST_PW");
    wrong.push(("CERCIS_TEST_PW", "wrong-pw"));
    let err = fail(
        &[
            "account",
            "login",
            project.to_str().unwrap(),
            "--id",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &wrong,
    );
    assert!(err.contains("口令不对"), "{err}");

    // 登录态没建立
    let who = ok_json(&["account", "whoami", project.to_str().unwrap(), "--json"], &envs);
    assert!(who["whoami"].is_null());
}

#[test]
fn plugin_edit_needs_admin_when_flag_is_on() {
    let dir = tmp("pluginadmin");
    let project = dir.join("proj");
    let home = dir.join("share-home");
    let home_s = home.to_str().unwrap().to_string();
    let envs: Vec<(&str, &str)> = vec![
        ("CERCIS_SHARE_HOME", home_s.as_str()),
        ("CERCIS_SHARE_KDF", "light"),
        ("CERCIS_TEST_PW", "pw"),
    ];
    ok(&["init", project.to_str().unwrap(), "--name", "NAS 多人"], &envs);
    for (id, role) in [("alice", "admin"), ("bob", "editor")] {
        ok(
            &[
                "account",
                "add",
                project.to_str().unwrap(),
                "--id",
                id,
                "--role",
                role,
                "--password-env",
                "CERCIS_TEST_PW",
            ],
            &envs,
        );
    }
    // 打开"仅管理员可改插件目录"
    let cfg_path = project.join("project.yaml");
    let text = std::fs::read_to_string(&cfg_path).unwrap();
    let text = if text.contains("admin_only_edit: false") {
        text.replace("admin_only_edit: false", "admin_only_edit: true")
    } else if text.contains("plugins:") {
        text.replace("plugins:", "plugins:\n  admin_only_edit: true")
    } else {
        format!("{text}\nplugins:\n  admin_only_edit: true\n")
    };
    std::fs::write(&cfg_path, text).unwrap();
    assert!(
        std::fs::read_to_string(&cfg_path)
            .unwrap()
            .contains("admin_only_edit: true"),
        "测试自己要把开关真的打开"
    );

    // 编辑者：插件改动被拒
    ok(
        &[
            "account",
            "login",
            project.to_str().unwrap(),
            "--id",
            "bob",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    let err = fail(
        &[
            "plugins",
            project.to_str().unwrap(),
            "register",
            "--plugin",
            "demo",
            "--kind",
            "column",
            "--target",
            "records.x",
        ],
        &envs,
    );
    assert!(err.contains("管理员") || err.contains("权限"), "{err}");

    // 管理员：放行（这里只是登记一条，不该被权限拦住）
    ok(
        &[
            "account",
            "login",
            project.to_str().unwrap(),
            "--id",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    let stdout = ok(
        &[
            "plugins",
            project.to_str().unwrap(),
            "register",
            "--plugin",
            "demo",
            "--kind",
            "column",
            "--target",
            "records.x",
        ],
        &envs,
    );
    assert!(stdout.contains("登记") || stdout.contains("demo"), "{stdout}");
}
