//! 交错测试（受 Cordis §3.3 的"可观察等价"启发）：**两组插件的操作换个顺序做，最终状态必须一样**。
//!
//! 为什么要这一条：插件是并行的、独立作者写的，谁先谁后不该影响数据。
//! 现实里最容易出的问题就是"我这边先装了模板、你那边先录了数据"，两边结果不一致。
//! 本测试把同一组操作按不同顺序跑两遍，再比较：
//! - `tables/*.csv` 的内容必须**逐字节相同**（数据是唯一事实来源）；
//! - 项目状态文件（模板 / 术语包 / 停用名单 / 注册台账）必须相同；
//! - `change_log` 只允许"顺序与时间戳"不同（流水本来就是按发生顺序记的）。
//!
//! 允许的差异写在这里，读的人一眼知道"哪些不同是可以接受的"。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-interleave-{name}"));
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

fn ok(args: &[&str], envs: &[(&str, &str)]) -> Value {
    let (code, stdout, stderr) = run(args, envs);
    assert_eq!(code, 0, "命令失败 {args:?}\n{stderr}");
    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{stdout}"))
}

const TEMPLATE: &str = r#"
name: demo-trial
label_cn: 交错验收模板
terms_profile: generic
tables:
  - name: plots
    label_cn: 小区
    primary_key: plot_id
    fields:
      - { name: plot_id, label_cn: 小区编号, type: text, required: true }
      - { name: block, label_cn: 区组, type: text }
  - name: readings
    label_cn: 读数
    primary_key: reading_id
    fields:
      - { name: reading_id, label_cn: 读数编号, type: text, required: true }
      - { name: plot_id, label_cn: 小区编号, type: text, required: true, reference: plots.plot_id }
      - { name: value_num, label_cn: 数值, type: number }
"#;

/// 写一份"插件 A 的登记"：给 readings 加一列（空列，可撤回）。
fn plugin_a(project: &Path) -> Vec<Vec<String>> {
    let p = project.display().to_string();
    vec![vec![
        "plugins".into(),
        p,
        "register".into(),
        "--plugin".into(),
        "plugin_a".into(),
        "--kind".into(),
        "column".into(),
        "--target".into(),
        "readings.spad_note".into(),
        "--json".into(),
    ]]
}

/// 插件 B 的动作：装模板（会建表）。
fn plugin_b(project: &Path, template: &Path) -> Vec<String> {
    vec![
        "install-template".into(),
        project.display().to_string(),
        "--file".into(),
        template.display().to_string(),
        "--plugin".into(),
        "plugin_b".into(),
        "--json".into(),
    ]
}

/// 插件 A 的另一半：往 readings 里写一行。
fn plugin_a_write(project: &Path) -> Vec<String> {
    vec![
        "append".into(),
        project.display().to_string(),
        "--table".into(),
        "readings".into(),
        "--target".into(),
        "R-001".into(),
        "--set".into(),
        "reading_id=R-001".into(),
        "--set".into(),
        "plot_id=P-1".into(),
        "--set".into(),
        "value_num=12.5".into(),
        "--json".into(),
    ]
}

/// 跑一遍"先 A 后 B"与"先 B 后 A"，比较最终状态。
fn run_order(name: &str, a_first: bool) -> PathBuf {
    let dir = tmp(name);
    let project = dir.join("proj");
    let template = dir.join("template.yaml");
    std::fs::write(&template, TEMPLATE).unwrap();
    let global = dir.join("global");
    let g = global.display().to_string();
    let envs: [(&str, &str); 1] = [("CERCIS_GLOBAL_PLUGINS", &g)];
    ok(
        &[
            "init",
            &project.display().to_string(),
            "--name",
            "交错验收",
            "--json",
        ],
        &[],
    );

    let do_a = |project: &Path| {
        for args in plugin_a(project) {
            let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            ok(&refs, &envs);
        }
    };
    let do_b = |project: &Path| {
        let args = plugin_b(project, &template);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        ok(&refs, &envs);
    };
    let do_write = |project: &Path| {
        let args = plugin_a_write(project);
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        ok(&refs, &envs);
    };

    if a_first {
        do_a(&project); // A 先登记一列
        do_b(&project); // B 后装模板
        do_write(&project);
    } else {
        do_b(&project); // B 先装模板
        do_a(&project); // A 后登记一列
        do_write(&project);
    }
    project
}

#[test]
fn interleaved_plugin_operations_reach_the_same_state() {
    let a = run_order("a-first", true);
    let b = run_order("b-first", false);

    // 1) 数据文件逐字节一致（唯一事实来源）
    for table in ["plots.csv", "readings.csv"] {
        let x = std::fs::read_to_string(a.join("tables").join(table)).unwrap_or_default();
        let y = std::fs::read_to_string(b.join("tables").join(table)).unwrap_or_default();
        assert_eq!(x, y, "表 {table} 在不同顺序下内容不一致：\n文件A：{x}\n文件B：{y}");
    }

    // 2) 项目状态一致（模板、停用名单、注册台账——SQL 之外的"结构状态"）
    for file in [
        "template.yaml",
        "plugins/state.yaml",
        "plugins/registrations.json",
    ] {
        let x = std::fs::read_to_string(a.join(file));
        let y = std::fs::read_to_string(b.join(file));
        let same = match (&x, &y) {
            (Ok(x), Ok(y)) => normalize(x.clone()) == normalize(y.clone()),
            (Err(_), Err(_)) => true, // 两边都没有这个文件，也算一致
            _ => false,
        };
        assert!(same, "状态文件 {file} 在不同顺序下不一致：\nA：{x:?}\nB：{y:?}");
    }

    // 3) 流水允许顺序/时间戳不同，但"做过哪些动作"必须一致
    let actions = |p: &Path| -> Vec<String> {
        let out = ok(
            &["log", &p.display().to_string(), "--limit", "50", "--json"],
            &[],
        );
        let mut list: Vec<String> = out["entries"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|e| {
                        format!(
                            "{}:{}",
                            e.get("action").and_then(|v| v.as_str()).unwrap_or(""),
                            e.get("target_table").and_then(|v| v.as_str()).unwrap_or("")
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        list.sort();
        list
    };
    assert_eq!(
        actions(&a),
        actions(&b),
        "两种顺序下做过的动作集合应相同"
    );

    // 4) 体检结论也要一致（体检按数据与状态算，顺序不该影响它）
    // validate 发现错误时退出码是 2——那是"结果"，不是失败
    let health = |p: &Path| -> Value {
        let args = ["validate".to_string(), p.display().to_string(), "--json".to_string()];
        let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let (code, stdout, stderr) = run(&refs, &[]);
        assert!(code == 0 || code == 2, "体检没跑起来：{stderr}");
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("不是 JSON：{e}\n{stdout}"))
    };
    let ha = health(&a);
    let hb = health(&b);
    assert_eq!(
        ha["errors"].as_array().map(|x| x.len()),
        hb["errors"].as_array().map(|x| x.len()),
        "两种顺序下体检的错误数应相同：\nA={ha}\nB={hb}"
    );
}

/// 归一化：注册台账里有 ULID 与时间戳，比较时只留"谁对什么做了什么"。
fn normalize(text: String) -> String {
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return text;
    };
    if let Some(entries) = v.get("entries").and_then(|e| e.as_array()) {
        let mut lines: Vec<String> = entries
            .iter()
            .map(|e| {
                format!(
                    "{}|{}|{}|{}|{}",
                    e.get("plugin").and_then(|v| v.as_str()).unwrap_or(""),
                    e.get("kind").and_then(|v| v.as_str()).unwrap_or(""),
                    e.get("target").and_then(|v| v.as_str()).unwrap_or(""),
                    e.get("state").and_then(|v| v.as_str()).unwrap_or(""),
                    e.get("blocked_reason").and_then(|v| v.as_str()).unwrap_or(""),
                )
            })
            .collect();
        lines.sort();
        return lines.join("\n");
    }
    text
}
