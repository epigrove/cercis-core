//! S14 验收：NAS / 网络位置与工作副本（FR-N01–FR-N07，跨进程跑真的 `cercis`）。
//!
//! 覆盖：
//! 1. 登记 / 列出 / 体检 / 移除网络位置（把本机临时目录当"NAS"）
//! 2. 工作副本：建副本 → 两边各自离线记 → 状态能看出各有多少新变更 → 双向同步 → 并集无损
//! 3. 断网（主副本不可达）：状态报 `offline`，本地仍可记录；同步明确报错不半途乱写
//! 4. 凭据进系统钥匙串：项目文件里找不到口令（用假钥匙串命令验证）
//! 5. 写租约：取/查/释放，过期的不算数

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-nas-flow-{name}"));
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

fn add_row(project: &Path, id: &str) {
    ok(
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
        &[],
    );
}

fn row_ids(project: &Path) -> Vec<String> {
    let v = ok_json(
        &[
            "query",
            project.to_str().unwrap(),
            "--table",
            "records",
            "--json",
        ],
        &[],
    );
    v["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|r| r["record_id"].as_str().unwrap_or("").to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn remote_location_add_test_list_remove() {
    let dir = tmp("remote");
    let main = dir.join("nas项目");
    ok(&["init", main.to_str().unwrap(), "--name", "NAS 试验"], &[]);
    add_row(&main, "R-1");

    // 登记（把本机临时目录当 NAS）
    ok(
        &[
            "remote",
            "add",
            main.to_str().unwrap(),
            "--name",
            "nas",
            "--path",
            main.to_str().unwrap(),
            "--kind",
            "local",
            "--note",
            "本机自测",
        ],
        &[],
    );
    let list = ok_json(&["remote", "list", main.to_str().unwrap(), "--json"], &[]);
    assert_eq!(list["remotes"].as_array().unwrap().len(), 1);
    assert_eq!(list["remotes"][0]["name"], "nas");

    // 体检：可达、可写、支持原子改名、认得出这是个项目
    let t = ok_json(
        &["remote", "test", main.to_str().unwrap(), "--name", "nas", "--json"],
        &[],
    );
    let test = &t["test"];
    assert_eq!(test["reachable"], true);
    assert_eq!(test["writable"], true);
    assert_eq!(test["atomic_rename"], true);
    assert_eq!(test["is_project"], true);

    // 清单里没有口令
    let text = std::fs::read_to_string(main.join("access/remotes.yaml")).unwrap();
    assert!(!text.contains("password"), "{text}");

    ok(
        &["remote", "remove", main.to_str().unwrap(), "--name", "nas"],
        &[],
    );
    let list = ok_json(&["remote", "list", main.to_str().unwrap(), "--json"], &[]);
    assert_eq!(list["remotes"].as_array().unwrap().len(), 0);
}

#[test]
fn workcopy_offline_edits_then_two_way_sync() {
    let dir = tmp("workcopy");
    let main = dir.join("nas-主副本");
    let copy = dir.join("本地工作副本");
    ok(&["init", main.to_str().unwrap(), "--name", "试验"], &[]);
    add_row(&main, "R-1");

    ok(
        &[
            "workcopy",
            "create",
            main.to_str().unwrap(),
            "--to",
            copy.to_str().unwrap(),
            "--name",
            "nas",
        ],
        &[],
    );
    assert!(copy.join("_labnote/workcopy.yaml").exists());

    // 本地离线记两条；NAS 主副本上（别人）记一条
    add_row(&copy, "R-2");
    add_row(&copy, "R-3");
    add_row(&main, "R-4");

    let st = ok_json(&["workcopy", "status", copy.to_str().unwrap(), "--json"], &[]);
    assert_eq!(st["status"]["verdict"], "needs-sync", "{st}");
    assert_eq!(st["status"]["local_changes"], 2);
    assert_eq!(st["status"]["remote_changes"], 1);

    let rep = ok_json(
        &["workcopy", "sync", copy.to_str().unwrap(), "--json"],
        &[],
    );
    assert_eq!(rep["ok"], true, "{rep}");
    assert_eq!(rep["conflicts_total"], 0);

    // 两边都齐，且没有重复
    for root in [&main, &copy] {
        let ids = row_ids(root);
        assert_eq!(ids.len(), 4, "{root:?} 应有 4 条：{ids:?}");
        for want in ["R-1", "R-2", "R-3", "R-4"] {
            assert!(ids.contains(&want.to_string()), "{root:?} 缺 {want}");
        }
    }

    // 再查状态：已一致
    let st2 = ok_json(&["workcopy", "status", copy.to_str().unwrap(), "--json"], &[]);
    assert_eq!(st2["status"]["verdict"], "ok", "{st2}");
}

#[test]
fn offline_is_reported_and_local_writes_keep_working() {
    let dir = tmp("offline");
    let main = dir.join("主副本");
    let copy = dir.join("副本");
    ok(&["init", main.to_str().unwrap(), "--name", "试验"], &[]);
    add_row(&main, "R-1");
    ok(
        &[
            "workcopy",
            "create",
            main.to_str().unwrap(),
            "--to",
            copy.to_str().unwrap(),
        ],
        &[],
    );

    // 主副本"掉线"
    std::fs::rename(&main, dir.join("主副本-挪走了")).unwrap();

    let st = ok_json(&["workcopy", "status", copy.to_str().unwrap(), "--json"], &[]);
    assert_eq!(st["status"]["verdict"], "offline", "{st}");
    assert_eq!(st["offline"], true);

    // 本地照记不误
    add_row(&copy, "R-2");
    assert_eq!(row_ids(&copy).len(), 2);

    // 同步会明确报离线（不是默默成功）
    let (code, stdout, stderr) = run(&["workcopy", "sync", copy.to_str().unwrap()], &[]);
    assert_ne!(code, 0);
    assert!(format!("{stdout}{stderr}").contains("离线"));
}

#[test]
fn credentials_live_in_keychain_not_in_project() {
    let dir = tmp("cred");
    let project = dir.join("proj");
    ok(&["init", project.to_str().unwrap(), "--name", "凭据试验"], &[]);
    ok(
        &[
            "remote",
            "add",
            project.to_str().unwrap(),
            "--name",
            "nas",
            "--path",
            project.to_str().unwrap(),
        ],
        &[],
    );

    // 用一个"假钥匙串"（把口令写到盒子外的文件里）验证行为
    let box_file = dir.join("keychain.txt");
    let store_cmd = format!("printf '%s' {{secret}} > {}", box_file.display());
    let load_cmd = format!("cat {}", box_file.display());
    let delete_cmd = format!("rm -f {}", box_file.display());
    let envs: Vec<(&str, &str)> = vec![
        ("CERCIS_KEYCHAIN_STORE_CMD", store_cmd.as_str()),
        ("CERCIS_KEYCHAIN_LOAD_CMD", load_cmd.as_str()),
        ("CERCIS_KEYCHAIN_DELETE_CMD", delete_cmd.as_str()),
        ("CERCIS_TEST_PW", "super-secret-pw"),
    ];

    ok(
        &[
            "remote",
            "credential",
            "set",
            project.to_str().unwrap(),
            "--name",
            "nas",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    // 钥匙串（我们的假盒子）里有
    assert_eq!(
        std::fs::read_to_string(&box_file).unwrap(),
        "super-secret-pw"
    );
    // 项目里没有
    let mut leaked = Vec::new();
    let mut stack = vec![project.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(text) = std::fs::read_to_string(&p) {
                if text.contains("super-secret-pw") {
                    leaked.push(p.display().to_string());
                }
            }
        }
    }
    assert!(leaked.is_empty(), "口令泄漏到项目里了：{leaked:?}");

    let has = ok_json(
        &[
            "remote",
            "credential",
            "has",
            project.to_str().unwrap(),
            "--name",
            "nas",
            "--json",
        ],
        &envs,
    );
    assert_eq!(has["stored"], true);

    ok(
        &[
            "remote",
            "credential",
            "delete",
            project.to_str().unwrap(),
            "--name",
            "nas",
        ],
        &envs,
    );
    assert!(!box_file.exists(), "删掉后钥匙串里不该还留着");
}

#[test]
fn write_lease_is_visible_and_releasable() {
    let dir = tmp("lease");
    let main = dir.join("主副本");
    ok(&["init", main.to_str().unwrap(), "--name", "试验"], &[]);

    let claimed = ok_json(
        &[
            "remote",
            "claim",
            main.to_str().unwrap(),
            "--who",
            "wenhuan@laptop",
            "--minutes",
            "30",
            "--json",
        ],
        &[],
    );
    assert_eq!(claimed["ok"], true);
    assert!(main.join("access/leases/wenhuan_laptop.lease").exists());

    let released = ok_json(
        &[
            "remote",
            "release",
            main.to_str().unwrap(),
            "--who",
            "wenhuan@laptop",
            "--json",
        ],
        &[],
    );
    assert!(released["cleared"].as_u64().unwrap() >= 1);
    assert!(!main.join("access/leases/wenhuan_laptop.lease").exists());
}
