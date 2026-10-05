//! FR-M05 验收：差异对比（记录级 + 快照级）。
//!
//! 覆盖：
//! 1. 记录级：一条记录被更正后，`diff --table --id` 给出"字段：旧 → 新"并列出更正理由
//! 2. 记录级：快照 + 生效值两套口径都对（表里那行没动、更正已记录 → 仍显示生效值）
//! 3. 快照级：两份历史对象逐行比，新增/删除行标出来

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-diff-flow-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin()).args(args).output().expect("跑得起来 cercis");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(args: &[&str]) -> String {
    let (code, stdout, stderr) = run(args);
    assert_eq!(code, 0, "命令失败 {args:?}\nstdout: {stdout}\nstderr: {stderr}");
    stdout
}

fn ok_json(args: &[&str]) -> Value {
    let stdout = ok(args);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{stdout}"))
}

#[test]
fn record_diff_uses_effective_value_from_corrections() {
    let dir = tmp("record");
    let p = dir.join("proj");
    let ps = p.to_str().unwrap();
    ok(&["init", ps, "--name", "差异试验"]);
    ok(&[
        "append",
        ps,
        "--table",
        "records",
        "--set",
        "record_id=R-1",
        "--set",
        "value_num=12.5",
        "--set",
        "notes=起初",
        "--allow-new-columns",
    ]);
    // 只记更正（表里那一行不动）——这是"更正即新增"的正常形态
    ok(&[
        "correct", ps, "--table", "records", "--target", "R-1", "--field", "value_num",
        "--old", "12.5", "--new", "12.7", "--reason", "笔误",
    ]);

    let d = ok_json(&["diff", ps, "--table", "records", "--id", "R-1", "--json"]);
    assert_eq!(d["ok"], true, "{d}");
    assert_eq!(d["kind"], "record");
    let changes = d["diff"]["changes"].as_array().unwrap();
    let value = changes
        .iter()
        .find(|c| c["field"] == "value_num")
        .expect("value_num 应有变化");
    assert_eq!(value["old"], "12.5", "老值来自快照");
    assert_eq!(value["new"], "12.7", "新值 = 快照 + 已记录的更正（生效值）");
    assert_eq!(d["diff"]["corrections"][0]["reason"], "笔误");

    // 人读输出里也要看得到
    let text = ok(&["diff", ps, "--table", "records", "--id", "R-1"]);
    assert!(text.contains("12.5 → 12.7"), "{text}");
    assert!(text.contains("笔误"), "{text}");
}

#[test]
fn snapshot_diff_marks_new_rows() {
    let dir = tmp("snapshot");
    let p = dir.join("proj");
    let ps = p.to_str().unwrap();
    ok(&["init", ps, "--name", "快照对比"]);
    ok(&[
        "append", ps, "--table", "records", "--set", "record_id=R-1", "--set", "value_num=1",
        "--allow-new-columns",
    ]);
    let log = ok_json(&["log", ps, "--limit", "5", "--json"]);
    let h1 = log["entries"][0]["hash_after"].as_str().unwrap().to_string();

    ok(&[
        "append", ps, "--table", "records", "--set", "record_id=R-2", "--set", "value_num=2",
        "--allow-new-columns",
    ]);
    let log = ok_json(&["log", ps, "--limit", "5", "--json"]);
    let h2 = log["entries"][0]["hash_after"].as_str().unwrap().to_string();

    let d = ok_json(&["diff", ps, "--from", &h1, "--to", &h2, "--json"]);
    assert_eq!(d["kind"], "snapshot");
    assert_eq!(d["diff"]["added"], 1, "{d}");
    assert_eq!(d["diff"]["removed"], 0);
    let text = ok(&["diff", ps, "--from", &h1, "--to", &h2]);
    assert!(text.contains("R-2"), "{text}");
}

#[test]
fn diff_needs_something_to_compare() {
    let dir = tmp("usage");
    let p = dir.join("proj");
    let ps = p.to_str().unwrap();
    ok(&["init", ps, "--name", "用法"]);
    let (code, _, err) = run(&["diff", ps]);
    assert_ne!(code, 0, "没给要比的东西应当报错");
    assert!(err.contains("cercis diff"), "错误提示要给出用法：{err}");
}
