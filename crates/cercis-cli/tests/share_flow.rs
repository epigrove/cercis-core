//! S11 验收：加密共享与分权（跨进程跑真的 `cercis` 命令）。
//!
//! 覆盖需求 FR-I07–FR-I13：
//! 1. 加密区域：命中的文件在磁盘上变成密文（明文消失）
//! 2. 分权解锁：没解锁读不了；解锁后透明读写（`show` / `append` 照常用）
//! 3. 未授权成员：看不到区域、也读不出内容
//! 4. 权限级别：只读成员能看能导出、不能写
//! 5. 移除成员：换密钥 + 重加密，旧钥匙作废
//! 6. 解密导出：明明白白写进 exports/，并留一条流水
//! 7. 加密与开放并存：明文残留能被 `apply` 收干净，并有明确提示
//! 8. 口令安全：改口令不动数据；项目里永远找不到明文口令

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-share-flow-{name}"));
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

/// 项目夹具：临时目录 + 一个装着一条记录的项目 + 独立的会话目录。
struct Fixture {
    dir: PathBuf,
    project: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = tmp(name);
        let project = dir.join("proj");
        let home = dir.join("share-home");
        std::fs::create_dir_all(&home).unwrap();
        let f = Self {
            dir,
            project,
            home,
        };
        let p = f.project.to_string_lossy().to_string();
        ok(&["init", &p, "--name", "加密试验"], &f.env_refs());
        ok(
            &[
                "append",
                &p,
                "--table",
                "records",
                "--set",
                "record_id=R-1",
                "--set",
                "secret_note=机密X9",
                "--set",
                "value_num=12.5",
                "--allow-new-columns",
            ],
            &f.env_refs(),
        );
        f
    }

    fn env_refs(&self) -> Vec<(&str, &str)> {
        vec![
            ("CERCIS_SHARE_HOME", self.home.to_str().unwrap()),
            ("CERCIS_SHARE_KDF", "light"),
            ("CERCIS_KEYCHAIN", "off"),
        ]
    }

    fn root(&self) -> &str {
        self.project.to_str().unwrap()
    }

    fn add_member(&self, id: &str, password: &str) {
        let mut envs = self.env_refs();
        envs.push(("CERCIS_TEST_PW", password));
        ok(
            &[
                "share",
                "member-add",
                self.root(),
                "--id",
                id,
                "--name",
                id,
                "--password-env",
                "CERCIS_TEST_PW",
            ],
            &envs,
        );
    }

    fn unlock(&self, id: &str, password: &str) {
        let mut envs = self.env_refs();
        envs.push(("CERCIS_TEST_PW", password));
        ok(
            &[
                "share",
                "unlock",
                self.root(),
                "--member",
                id,
                "--password-env",
                "CERCIS_TEST_PW",
                "--ttl",
                "8",
            ],
            &envs,
        );
    }

    /// 建区域：默认把命中的文件立刻加密。
    fn zone_create(&self, extra: &[&str]) {
        let mut args = vec![
            "share",
            "zone-create",
            self.root(),
            "--name",
            "secret-records",
            "--label",
            "机密的",
            "--path",
            "tables/records.csv",
        ];
        args.extend_from_slice(extra);
        ok(&args, &self.env_refs());
    }

    /// 把"当前生效成员"指到某人（模拟他在这台机器上解锁过）。
    fn switch_current_member(&self, id: &str) {
        let projects = self.home.join("projects");
        let dir = std::fs::read_dir(&projects)
            .unwrap()
            .next()
            .expect("有项目会话目录")
            .unwrap()
            .path();
        std::fs::write(dir.join("current"), format!("{id}\n")).unwrap();
    }
}

/// 在项目里找字符串（找得到 = 明文泄漏了）。
fn find_marker(root: &Path, marker: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().map(|n| n == ".git").unwrap_or(false) {
                    continue;
                }
                stack.push(p);
            } else if let Ok(bytes) = std::fs::read(&p) {
                if bytes.windows(marker.len()).any(|w| w == marker.as_bytes()) {
                    hits.push(p.display().to_string());
                }
            }
        }
    }
    hits.sort();
    hits
}

fn read_text(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

#[test]
fn zone_create_encrypts_file_and_leaves_no_plaintext() {
    let f = Fixture::new("zone-create");
    f.add_member("alice", "alice-pw");
    f.zone_create(&["--member", "alice:admin"]);

    assert!(
        !f.project.join("tables/records.csv").exists(),
        "明文 CSV 应该已经被换成密文"
    );
    assert!(
        f.project.join("tables/records.csv.enc").exists(),
        "密文文件应该在"
    );
    let blob = std::fs::read(f.project.join("tables/records.csv.enc")).unwrap();
    assert!(!blob.windows(3).any(|w| w == "R-1".as_bytes()));
    assert_eq!(
        find_marker(&f.project, "机密X9"),
        Vec::<String>::new(),
        "项目目录里不该有任何明文残留（含历史对象库）"
    );

    // 口令不落项目：members.yaml 里只有公钥
    let members = read_text(&f.project.join("access/members.yaml"));
    assert!(members.contains("public_key"));
    assert!(!members.contains("alice-pw"));

    let st = ok_json(&["share", "status", f.root(), "--json"], &f.env_refs());
    let zone = &st["zones"][0];
    assert_eq!(zone["zone"], "secret-records");
    assert_eq!(zone["files"], 1);
    assert_eq!(zone["plaintext_left"].as_array().unwrap().len(), 0);
}

#[test]
fn locked_read_fails_and_unlock_restores_transparent_io() {
    let f = Fixture::new("transparent");
    f.add_member("alice", "alice-pw");
    f.zone_create(&["--member", "alice:admin"]);

    let err = fail(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(err.contains("未解锁"), "错误提示应说明未解锁：{err}");
    assert!(err.contains("share unlock"), "错误提示应给出解锁命令：{err}");

    let mut envs = f.env_refs();
    envs.push(("CERCIS_TEST_PW", "wrong-pw"));
    let err = fail(
        &[
            "share",
            "unlock",
            f.root(),
            "--member",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    assert!(err.contains("口令不对"), "应提示口令不对：{err}");

    f.unlock("alice", "alice-pw");
    let out = ok(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(out.contains("机密X9"), "解锁后应看到明文：{out}");

    ok(
        &[
            "append",
            f.root(),
            "--table",
            "records",
            "--set",
            "record_id=R-2",
            "--set",
            "secret_note=机密X9-B",
            "--allow-new-columns",
        ],
        &f.env_refs(),
    );
    assert!(!f.project.join("tables/records.csv").exists());
    assert_eq!(find_marker(&f.project, "机密X9"), Vec::<String>::new());
    assert_eq!(find_marker(&f.project, "R-2"), Vec::<String>::new());

    let out = ok(
        &["query", f.root(), "--table", "records", "--json"],
        &f.env_refs(),
    );
    assert!(out.contains("机密X9-B"));

    ok(&["share", "lock", f.root()], &f.env_refs());
    let err = fail(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(err.contains("未解锁"), "{err}");
}

#[test]
fn unauthorized_member_sees_nothing() {
    let f = Fixture::new("unauthorized");
    f.add_member("alice", "alice-pw");
    f.add_member("bob", "bob-pw");
    f.zone_create(&["--member", "alice:admin"]);

    let list = ok_json(
        &["share", "list", f.root(), "--member", "bob", "--json"],
        &f.env_refs(),
    );
    assert_eq!(
        list["zones"].as_array().unwrap().len(),
        0,
        "未授权成员不该看到区域"
    );
    assert_eq!(list["as_member"], "bob");

    f.unlock("bob", "bob-pw");
    let st = ok_json(&["share", "status", f.root(), "--json"], &f.env_refs());
    assert_eq!(st["zones"].as_array().unwrap().len(), 0);

    let err = fail(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(err.contains("没有区域") || err.contains("不在区域"), "{err}");
    let err = fail(
        &["share", "cat", f.root(), "--file", "tables/records.csv"],
        &f.env_refs(),
    );
    assert!(err.contains("没有区域") || err.contains("不在区域"), "{err}");
}

#[test]
fn read_only_member_can_read_and_export_but_not_write() {
    let f = Fixture::new("readonly");
    f.add_member("alice", "alice-pw");
    f.add_member("bob", "bob-pw");
    f.zone_create(&["--member", "alice:admin", "--member", "bob:read"]);
    f.unlock("bob", "bob-pw");

    let out = ok(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(out.contains("机密X9"));

    let err = fail(
        &[
            "append",
            f.root(),
            "--table",
            "records",
            "--set",
            "record_id=R-9",
            "--allow-new-columns",
        ],
        &f.env_refs(),
    );
    assert!(err.contains("只读"), "应提示只读权限：{err}");

    let rep = ok_json(
        &[
            "share",
            "export",
            f.root(),
            "--zone",
            "secret-records",
            "--json",
        ],
        &f.env_refs(),
    );
    assert_eq!(rep["member"], "bob");
    assert_eq!(rep["files"].as_array().unwrap().len(), 1);
    assert!(f
        .project
        .join("exports/secret-records/tables/records.csv")
        .exists());
}

#[test]
fn revoke_rotates_key_and_kills_old_session() {
    let f = Fixture::new("revoke");
    f.add_member("alice", "alice-pw");
    f.add_member("bob", "bob-pw");
    f.zone_create(&["--member", "alice:admin", "--member", "bob:read"]);
    f.unlock("bob", "bob-pw");
    let before = ok(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(before.contains("机密X9"));

    f.unlock("alice", "alice-pw");
    let rep = ok_json(
        &[
            "share",
            "revoke",
            f.root(),
            "--zone",
            "secret-records",
            "--member",
            "bob",
            "--as",
            "alice",
            "--json",
        ],
        &f.env_refs(),
    );
    assert_eq!(rep["generation"], 2);
    assert_eq!(rep["reencrypted"], 1);
    assert_eq!(rep["remaining"].as_array().unwrap().len(), 1);
    assert!(f
        .project
        .join("access/keyrings/secret-records.yaml")
        .exists());

    let after = ok(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(after.contains("机密X9"), "重加密不该弄丢数据");

    // bob 的旧会话（旧钥匙）解不开新密文
    f.switch_current_member("bob");
    let err = fail(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(
        err.contains("旧钥匙") || err.contains("解不开"),
        "被移除者的旧钥匙应当作废：{err}"
    );

    let list = ok_json(
        &["share", "list", f.root(), "--member", "bob", "--json"],
        &f.env_refs(),
    );
    assert_eq!(list["zones"].as_array().unwrap().len(), 0);
}

#[test]
fn export_and_log_are_recorded() {
    let f = Fixture::new("export");
    f.add_member("alice", "alice-pw");
    f.zone_create(&["--member", "alice:admin"]);
    f.unlock("alice", "alice-pw");

    let dry = ok_json(
        &[
            "share",
            "export",
            f.root(),
            "--zone",
            "secret-records",
            "--dry-run",
            "--json",
        ],
        &f.env_refs(),
    );
    assert_eq!(dry["dry_run"], true);
    assert!(!f.project.join("exports/secret-records").exists());

    let rep = ok_json(
        &[
            "share",
            "export",
            f.root(),
            "--zone",
            "secret-records",
            "--to",
            "exports/give-to-r",
            "--json",
        ],
        &f.env_refs(),
    );
    assert_eq!(rep["dest"], "exports/give-to-r");
    let exported = read_text(&f.project.join("exports/give-to-r/tables/records.csv"));
    assert!(exported.contains("机密X9"), "导出的明文应可读");

    let log = ok_json(&["log", f.root(), "--limit", "20", "--json"], &f.env_refs());
    let entries = log["entries"].as_array().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e["action"] == "export" && e["target_table"] == "zone"),
        "流水里应有解密导出记录：{log}"
    );
}

#[test]
fn apply_cleans_up_plaintext_left_behind() {
    let f = Fixture::new("apply");
    f.add_member("alice", "alice-pw");
    f.zone_create(&["--member", "alice:admin", "--no-apply"]);
    assert!(f.project.join("tables/records.csv").exists());

    let issues = ok(&["doctor", f.root()], &f.env_refs());
    assert!(issues.contains("明文"), "体检应提示明文残留：{issues}");

    let err = fail(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(err.contains("明文"), "{err}");

    f.unlock("alice", "alice-pw");
    ok(&["share", "apply", f.root(), "--dry-run"], &f.env_refs());
    assert!(f.project.join("tables/records.csv").exists());

    let rep = ok_json(&["share", "apply", f.root(), "--json"], &f.env_refs());
    assert_eq!(rep["zones"][0]["encrypted"].as_array().unwrap().len(), 1);
    assert!(!f.project.join("tables/records.csv").exists());
    assert!(f.project.join("tables/records.csv.enc").exists());
    assert_eq!(find_marker(&f.project, "机密X9"), Vec::<String>::new());
}

#[test]
fn password_change_keeps_data_readable() {
    let f = Fixture::new("passwd");
    f.add_member("alice", "old-pw");
    f.zone_create(&["--member", "alice:admin"]);
    f.unlock("alice", "old-pw");

    std::fs::write(f.dir.join("old.txt"), "old-pw\n").unwrap();
    std::fs::write(f.dir.join("new.txt"), "new-pw\n").unwrap();
    ok(
        &[
            "share",
            "passwd",
            f.root(),
            "--id",
            "alice",
            "--old-password-file",
            f.dir.join("old.txt").to_str().unwrap(),
            "--new-password-file",
            f.dir.join("new.txt").to_str().unwrap(),
        ],
        &f.env_refs(),
    );

    ok(&["share", "lock", f.root(), "--forget"], &f.env_refs());
    let mut envs = f.env_refs();
    envs.push(("CERCIS_TEST_PW", "old-pw"));
    let err = fail(
        &[
            "share",
            "unlock",
            f.root(),
            "--member",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &envs,
    );
    assert!(err.contains("口令不对"), "{err}");

    f.unlock("alice", "new-pw");
    let out = ok(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(out.contains("机密X9"));
}

#[test]
fn history_snapshots_are_encrypted_but_still_restorable() {
    let f = Fixture::new("history");
    // 加密前再写一条：这样对象库里就有一份"明文快照"
    ok(
        &[
            "append",
            f.root(),
            "--table",
            "records",
            "--set",
            "record_id=R-0",
            "--set",
            "secret_note=机密X9-OLD",
            "--allow-new-columns",
        ],
        &f.env_refs(),
    );
    let log = ok_json(&["log", f.root(), "--limit", "5", "--json"], &f.env_refs());
    let old_hash = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| !e["hash_after"].as_str().unwrap_or("").is_empty())
        .expect("有历史指纹")["hash_after"]
        .as_str()
        .unwrap()
        .to_string();

    // 划进加密区域：老快照也要跟着变密文，不能留明文备份
    f.add_member("alice", "alice-pw");
    f.zone_create(&["--member", "alice:admin"]);
    assert_eq!(
        find_marker(&f.project, "机密X9-OLD"),
        Vec::<String>::new(),
        "历史对象库里不该留明文快照"
    );
    assert!(f.project.join("access/zone-objects.yaml").exists());

    // 老指纹仍然还原得出来（走台账换指纹 + 解密）
    let dest = f.dir.join("restored.csv");
    f.unlock("alice", "alice-pw");
    ok(
        &[
            "restore",
            f.root(),
            "--hash",
            &old_hash,
            "--to",
            dest.to_str().unwrap(),
            "--summary",
            "回滚到加密前的版本",
        ],
        &f.env_refs(),
    );
    let text = read_text(&dest);
    assert!(text.contains("R-1"), "还原出来的内容应完整：{text}");

    // 没解锁时还原不了（密文对象解不开），且提示清楚
    ok(&["share", "lock", f.root()], &f.env_refs());
    let err = fail(
        &[
            "restore",
            f.root(),
            "--hash",
            &old_hash,
            "--to",
            f.dir.join("restored2.csv").to_str().unwrap(),
        ],
        &f.env_refs(),
    );
    assert!(
        err.contains("密文") || err.contains("解锁"),
        "没解锁时还原应给出可操作提示：{err}"
    );
}

#[test]
fn export_then_clean_removes_plaintext_copies() {
    let f = Fixture::new("clean");
    f.add_member("alice", "alice-pw");
    f.zone_create(&["--member", "alice:admin"]);
    f.unlock("alice", "alice-pw");
    ok(
        &[
            "share",
            "export",
            f.root(),
            "--zone",
            "secret-records",
        ],
        &f.env_refs(),
    );
    let exported = f.project.join("exports/secret-records/tables/records.csv");
    assert!(exported.exists(), "导出后应该有明文副本");
    assert_eq!(
        find_marker(&f.project.join("exports"), "机密X9").len(),
        1,
        "exports/ 里确实有明文"
    );

    // 试运行不动文件
    let dry = ok_json(
        &["share", "clean", f.root(), "--dry-run", "--json"],
        &f.env_refs(),
    );
    assert_eq!(dry["ok"], true);
    assert_eq!(dry["files"].as_array().unwrap().len(), 1);
    assert!(exported.exists(), "试运行不该删东西");

    // 真清：明文没了，项目里的密文还在、还能读
    let rep = ok_json(&["share", "clean", f.root(), "--json"], &f.env_refs());
    assert_eq!(rep["bytes"].as_u64().unwrap() > 0, true);
    assert!(!exported.exists(), "明文副本应被删掉");
    assert_eq!(find_marker(&f.project, "机密X9"), Vec::<String>::new());
    assert!(f.project.join("tables/records.csv.enc").exists());
    let out = ok(&["show", f.root(), "--table", "records"], &f.env_refs());
    assert!(out.contains("机密X9"), "密文还在、还能读：{out}");

    // 流水里留了一条清理记录
    let log = ok_json(&["log", f.root(), "--limit", "10", "--json"], &f.env_refs());
    assert!(
        log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["summary"]
                .as_str()
                .unwrap_or("")
                .contains("清理解密导出")),
        "清理应留痕：{log}"
    );
}

#[test]
fn manifest_reports_encrypted_tables() {
    let f = Fixture::new("manifest");
    f.add_member("alice", "alice-pw");
    f.zone_create(&["--member", "alice:admin"]);

    let m = ok_json(&["manifest-cmd", f.root(), "--json"], &f.env_refs());
    let tables = m["manifest"]["tables"].as_array().unwrap();
    let records = tables
        .iter()
        .find(|t| t["name"] == "records")
        .expect("加密的表也要出现在清单里");
    assert_eq!(records["encrypted"], true);
    assert_eq!(records["locked"], true);
    assert_eq!(m["manifest"]["access"]["zones_total"], 1);
    assert_eq!(m["manifest"]["access"]["zones_locked"], 1);
    assert_eq!(
        m["manifest"]["access"]["zones"].as_array().unwrap().len(),
        0,
        "未解锁时不该暴露区域名"
    );

    f.unlock("alice", "alice-pw");
    let m = ok_json(&["manifest-cmd", f.root(), "--json"], &f.env_refs());
    let tables = m["manifest"]["tables"].as_array().unwrap();
    let records = tables.iter().find(|t| t["name"] == "records").unwrap();
    assert_eq!(records["locked"], false);
    assert!(records["rows"].as_u64().unwrap() >= 1);
    assert!(records["columns"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c == "secret_note"));
    assert_eq!(m["manifest"]["access"]["unlocked_as"], "alice");
    assert_eq!(m["manifest"]["access"]["zones"][0]["permission"], "admin");
}
