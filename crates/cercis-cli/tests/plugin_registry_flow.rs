//! D37–D40 的端到端验收（跨进程跑真的 `cercis` 命令）。
//!
//! 覆盖：
//! 1. 插件声明式前置：装了但不适用时，内核说清"缺什么"（D38）
//! 2. 装模板 / 切术语包时**内核代插件登记**，并备份上一份（D37）
//! 3. 停用插件 = 倒序回滚它的登记；模板还原、空表撤掉（D37）
//! 4. 表里已有数据时不替用户删，台账记下"被拦下"与原因（数据只增不改优先）
//! 5. 扩展点键必须带命名空间；第三方不得用 `cercis` 前缀（D40）

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-registry-flow-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin())
        .args(args)
        .output()
        .expect("跑得起来 cercis");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

fn ok(args: &[&str]) -> Value {
    let (code, stdout, stderr) = run(args);
    assert!(code == 0 || code == 2, "命令失败 {args:?}\n{stderr}");
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{stdout}"))
}

const TEMPLATE: &str = r#"
name: demo-trial
label_cn: 演示模板
label_en: Demo trial
terms_profile: generic
tables:
  - name: plots
    label_cn: 小区
    label_en: Plot
    primary_key: plot_id
    fields:
      - { name: plot_id, label_cn: 小区编号, label_en: Plot ID, type: text, required: true }
      - { name: area_m2, label_cn: 面积, label_en: Area, type: number, precision: 1 }
  - name: readings
    label_cn: 读数
    label_en: Reading
    primary_key: ""
    fields:
      - { name: plot_id, label_cn: 小区编号, label_en: Plot ID, type: text, required: true, reference: plots.plot_id }
      - { name: value_num, label_cn: 数值, label_en: Value, type: number, precision: 1 }
"#;

const MANIFEST: &str = r#"
id: demo_io
namespace: demo_io
name: Demo IO
label_cn: 演示插件
version: 0.1.0
tier: third-party
requires_plugin_api: 1
requires:
  tables: [readings]
permissions:
  read: true
  write: true
ui:
  - kind: command
    id: demo_io.ping
    label_cn: 打招呼
entrypoint: demo-io
"#;

fn setup(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = tmp(name);
    let project = dir.join("proj");
    let tpl = dir.join("template.yaml");
    std::fs::write(&tpl, TEMPLATE).unwrap();
    ok(&[
        "init",
        &project.display().to_string(),
        "--name",
        "登记台账验收",
        "--json",
    ]);
    // 放一个"第三方插件"进项目
    let plugin_dir = project.join("plugins").join("demo_io");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("plugin.yaml"), MANIFEST).unwrap();
    (dir, project, tpl)
}

fn plugin_status(project: &Path, id: &str) -> Value {
    let out = ok(&["plugins", &project.display().to_string(), "--json"]);
    out["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[test]
fn activation_says_what_is_missing_then_flips_after_template() {
    let (_dir, project, tpl) = setup("activation");
    let before = plugin_status(&project, "demo_io");
    assert_eq!(before["active"], false, "{before}");
    assert!(
        before["inactive_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap_or("").contains("readings")),
        "要说清缺什么：{before}"
    );

    ok(&[
        "install-template",
        &project.display().to_string(),
        "--file",
        &tpl.display().to_string(),
        "--plugin",
        "demo_io",
        "--json",
    ]);
    let after = plugin_status(&project, "demo_io");
    assert_eq!(after["active"], true, "装了模板就该生效：{after}");
}

#[test]
fn disable_reverts_what_the_plugin_registered() {
    let (_dir, project, tpl) = setup("revert");
    let p = project.display().to_string();
    ok(&[
        "install-template",
        &p,
        "--file",
        &tpl.display().to_string(),
        "--plugin",
        "demo_io",
        "--json",
    ]);
    // 模板带出来的表与模板本身都在台账里
    let ledger = ok(&["plugins", &p, "registered", "--json"]);
    let kinds: Vec<&str> = ledger["registrations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap_or(""))
        .collect();
    assert!(kinds.contains(&"template"), "{kinds:?}");
    assert_eq!(kinds.iter().filter(|k| **k == "table").count(), 2, "{kinds:?}");

    // 再登记一个"插件加的列"（表里没有数据行 → 列与表都能干净撤回）
    std::fs::write(
        project.join("tables/readings.csv"),
        "plot_id,value_num,spad_note\n",
    )
    .unwrap();
    ok(&[
        "plugins",
        &p,
        "register",
        "--plugin",
        "demo_io",
        "--kind",
        "column",
        "--target",
        "readings.spad_note",
        "--json",
    ]);

    // 停用：倒序回滚
    let (code, out) = {
        let (c, o, e) = run(&["plugins", &p, "disable", "--id", "demo_io"]);
        assert_eq!(c, 0, "{e}");
        (c, o)
    };
    assert!(out.contains("已停用 demo_io"), "{out}");

    // 空列被撤掉、模板表被撤掉、模板还原成 generic、插件转为停用
    assert!(
        !project.join("tables/readings.csv").exists(),
        "空表应被撤掉（含插件加的列）"
    );
    assert!(
        !project.join("tables/plots.csv").exists(),
        "模板带出来的另一张空表也应被撤掉"
    );
    let template = ok(&["template", &p, "--raw", "--json"]);
    assert_eq!(template["template"]["name"], "generic", "模板应还原成 generic");
    let status = plugin_status(&project, "demo_io");
    assert_eq!(status["disabled"], true, "{status}");
    assert_eq!(status["active"], false, "{status}");
    let _ = code;

    // 台账里只剩"被拦下"或"已撤回"，没有还在生效的
    let ledger = ok(&["plugins", &p, "registered", "--json"]);
    assert_eq!(
        ledger["registrations"].as_array().unwrap().len(),
        0,
        "停用后不该还有生效的登记：{ledger}"
    );

    // 重新启用
    ok(&["plugins", &p, "enable", "--id", "demo_io", "--json"]);
    let status = plugin_status(&project, "demo_io");
    assert_eq!(status["disabled"], false, "{status}");
}

#[test]
fn data_blocks_rollback_and_is_reported() {
    let (_dir, project, tpl) = setup("blocked");
    let p = project.display().to_string();
    ok(&[
        "install-template",
        &p,
        "--file",
        &tpl.display().to_string(),
        "--plugin",
        "demo_io",
        "--json",
    ]);
    // 往模板带出来的表里写数据
    let csv = project.parent().unwrap().join("plots.csv");
    std::fs::write(
        &csv,
        "plot_id,area_m2\nP-1,12.5\n",
    )
    .unwrap();
    ok(&[
        "append-many",
        &p,
        "--table",
        "plots",
        "--from",
        &csv.display().to_string(),
        "--json",
    ]);

    let (code, out, _err) = run(&["plugins", &p, "disable", "--id", "demo_io"]);
    assert_eq!(code, 0);
    assert!(
        out.contains("留在项目里了") && out.contains("已有 1 行数据"),
        "要说明为什么没撤：{out}"
    );
    assert!(
        project.join("tables/plots.csv").exists(),
        "有数据的表必须留着"
    );
    let ledger = ok(&["plugins", &p, "registered", "--all", "--json"]);
    let blocked: Vec<&Value> = ledger["registrations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["state"] == "blocked")
        .collect();
    assert_eq!(blocked.len(), 1, "{ledger}");
    assert!(
        blocked[0]["blocked_reason"]
            .as_str()
            .unwrap_or("")
            .contains("数据只增不改"),
        "{}",
        blocked[0]
    );
}

#[test]
fn manifests_must_be_namespaced() {
    let dir = tmp("namespace");
    let project = dir.join("proj");
    ok(&[
        "init",
        &project.display().to_string(),
        "--name",
        "命名空间验收",
        "--json",
    ]);
    // 没有命名空间的扩展点键 → 内核应当认为清单不合法
    let bad = project.join("plugins").join("bad");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(
        bad.join("plugin.yaml"),
        MANIFEST.replace("id: demo_io.ping", "id: ping"),
    )
    .unwrap();
    let (code, _out, err) = run(&["plugins", &project.display().to_string()]);
    assert_ne!(code, 0, "裸键应被拒绝");
    assert!(err.contains("命名空间"), "{err}");

    // 第三方冒用 cercis 前缀 → 也拒绝
    std::fs::write(
        bad.join("plugin.yaml"),
        MANIFEST.replace("id: demo_io", "id: cercis_fake"),
    )
    .unwrap();
    let (code, _out, err) = run(&["plugins", &project.display().to_string()]);
    assert_ne!(code, 0, "cercis 前缀留给官方");
    assert!(err.contains("cercis"), "{err}");
}
