//! S12 验收：分享守卫（FR-I04–FR-I05，红线"发布零数据"）。
//!
//! 覆盖：
//! 1. 真实项目数据 → 拒绝，并且不允许打成分享包
//! 2. 工具仓库（文档 + 源码）→ 放行，能打出分享包与逐文件清单
//! 3. 合成示例（有 SYNTHETIC.md 标记）→ 待人工确认但可打包
//! 4. 自定义敏感关键词 + 邮箱/手机号 → 进隐私清单

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-guard-flow-{name}"));
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

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn real_project_is_refused_and_cannot_be_packed() {
    let dir = tmp("project");
    let project = dir.join("真实项目");
    let (code, _, err) = run(&[
        "init",
        project.to_str().unwrap(),
        "--name",
        "真实项目",
    ]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = run(&[
        "append",
        project.to_str().unwrap(),
        "--table",
        "records",
        "--set",
        "record_id=R-1",
        "--set",
        "value_num=12.5",
    ]);
    assert_eq!(code, 0, "{err}");

    let rep = ok_json(&["guard", project.to_str().unwrap(), "--json"]);
    assert_eq!(rep["verdict"], "refuse", "{rep}");
    assert!(rep["refused"].as_u64().unwrap() >= 2);
    assert!(
        rep["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["class"] == "refuse" && f["path"] == "tables/records.csv"),
        "真实数据表必须被点名：{rep}"
    );

    // 打成分享包 → 直接不干
    let out = dir.join("包");
    let (code, stdout, stderr) = run(&[
        "guard",
        project.to_str().unwrap(),
        "--pack",
        out.to_str().unwrap(),
    ]);
    assert_ne!(code, 0, "真实数据不该能打成分享包");
    assert!(format!("{stdout}{stderr}").contains("不能作为分享包"));
    assert!(!out.join("share-manifest.json").exists());
}

#[test]
fn tool_repo_packs_with_manifest_and_skips_git() {
    let dir = tmp("tool");
    write(&dir.join("README.md"), "# 工具说明\n");
    write(&dir.join("LICENSE"), "Apache-2.0\n");
    write(&dir.join("src/lib.rs"), "pub fn hi() {}\n");
    write(&dir.join("docs/用法.md"), "怎么用\n");
    write(&dir.join(".git/HEAD"), "ref: refs/heads/main\n");
    write(&dir.join("target/debug/junk.bin"), "x\n");

    let rep = ok_json(&["guard", dir.to_str().unwrap(), "--json"]);
    assert_eq!(rep["verdict"], "ok", "{rep}");
    assert_eq!(rep["refused"], 0);
    assert!(
        !rep["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"].as_str().unwrap_or("").starts_with(".git/")),
        ".git 里的东西不该进清单"
    );

    let out = dir.join("发布包");
    let packed = ok_json(&[
        "guard",
        dir.to_str().unwrap(),
        "--pack",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(packed["packed_to"].as_str().unwrap().contains("发布包"));
    assert!(out.join("README.md").exists());
    assert!(out.join("src/lib.rs").exists());
    assert!(!out.join(".git/HEAD").exists());
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(out.join("share-manifest.json")).unwrap())
            .unwrap();
    assert!(manifest["files"].as_array().unwrap().len() >= 4);
    assert!(out.join("SHARE-README.md").exists());
}

#[test]
fn synthetic_example_needs_review_but_can_be_packed() {
    let dir = tmp("synthetic");
    let project = dir.join("合成示例");
    let (code, _, err) = run(&[
        "init",
        project.to_str().unwrap(),
        "--name",
        "合成示例",
    ]);
    assert_eq!(code, 0, "{err}");
    write(&project.join("SYNTHETIC.md"), "本项目全部为虚构数据，可用于演示与验收。\n");

    let rep = ok_json(&["guard", project.to_str().unwrap(), "--json"]);
    assert_eq!(rep["verdict"], "review", "{rep}");
    assert_eq!(rep["refused"], 0);
    assert!(rep["synthetic"].as_bool().unwrap());

    let out = dir.join("示例包");
    ok_json(&[
        "guard",
        project.to_str().unwrap(),
        "--pack",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(out.join("project.yaml").exists());
    assert!(out.join("SHARE-README.md").exists());
}

#[test]
fn privacy_and_keywords_are_reported() {
    let dir = tmp("privacy");
    write(
        &dir.join("notes.md"),
        "联系人：张三 someone@example.com；电话 13800138000；这里是未公开配方部分。\n",
    );
    let rep = ok_json(&[
        "guard",
        dir.to_str().unwrap(),
        "--keyword",
        "未公开配方",
        "--json",
    ]);
    assert_eq!(rep["verdict"], "review", "{rep}");
    let kinds: Vec<String> = rep["privacy"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["kind"].as_str().unwrap_or("").to_string())
        .collect();
    assert!(kinds.contains(&"email".to_string()), "{rep}");
    assert!(kinds.contains(&"phone".to_string()), "{rep}");
    assert!(kinds.contains(&"keyword".to_string()), "{rep}");
}
