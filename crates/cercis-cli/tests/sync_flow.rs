//! S13 验收：云盘 / Git 共存体检（FR-I06）。
//!
//! 覆盖：
//! 1. 干净项目 → `ok`
//! 2. 出现"云盘冲突副本"与"残留临时文件" → `needs-merge`，并给出收拢建议
//! 3. 项目在 Git 仓库里 → `attention`，提示 Git 不适合当数据同步通道

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-sync-flow-{name}"));
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

fn ok_json(args: &[&str]) -> Value {
    let (code, stdout, stderr) = run(args);
    assert_eq!(code, 0, "命令失败 {args:?}\n{stderr}");
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{stdout}"))
}

#[test]
fn clean_project_reports_ok() {
    let dir = tmp("clean");
    let project = dir.join("proj");
    let (code, _, err) = run(&["init", project.to_str().unwrap(), "--name", "干净项目"]);
    assert_eq!(code, 0, "{err}");
    let rep = ok_json(&["sync", project.to_str().unwrap(), "--json"]);
    assert_eq!(rep["verdict"], "ok", "{rep}");
    assert_eq!(rep["ok"], true);
}

#[test]
fn cloud_conflict_copies_trigger_needs_merge() {
    let dir = tmp("conflict");
    let project = dir.join("proj");
    let (code, _, err) = run(&["init", project.to_str().unwrap(), "--name", "分叉项目"]);
    assert_eq!(code, 0, "{err}");
    // 云盘留下的"副本"，以及一次被打断的写入
    std::fs::write(
        project.join("tables/observations (1).csv"),
        "record_id\nR-1\n",
    )
    .unwrap();
    std::fs::write(
        project.join("tables/observations.csv.cercis-tmp"),
        "写了一半",
    )
    .unwrap();

    let rep = ok_json(&["sync", project.to_str().unwrap(), "--json"]);
    assert_eq!(rep["verdict"], "needs-merge", "{rep}");
    assert_eq!(rep["ok"], false, "体检结果要能被脚本判成败");
    let kinds: Vec<String> = rep["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["kind"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(kinds.contains(&"cloud_copy".to_string()), "{rep}");
    assert!(kinds.contains(&"stale_tmp".to_string()), "{rep}");
    assert!(rep["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["advice"].as_str().unwrap_or("").contains("merge")));

    // 给人看的输出里要有收拢办法
    let (code, stdout, _) = run(&["sync", project.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(stdout.contains("cercis merge"), "{stdout}");
}

#[test]
fn git_repo_is_flagged_as_attention() {
    let dir = tmp("git");
    let project = dir.join("proj");
    let (code, _, err) = run(&["init", project.to_str().unwrap(), "--name", "Git 里的项目"]);
    assert_eq!(code, 0, "{err}");
    // 造一个最小 git 仓库（不依赖 git 命令是否可用：.git/HEAD 手写）
    std::fs::create_dir_all(project.join(".git")).unwrap();
    std::fs::write(project.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

    let rep = ok_json(&["sync", project.to_str().unwrap(), "--json"]);
    assert_eq!(rep["verdict"], "attention", "{rep}");
    assert_eq!(rep["git"]["branch"], "main");
    assert!(rep["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap_or("").contains("Git")));
}
