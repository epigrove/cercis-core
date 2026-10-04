//! 权限降级（Cordis §6.3）验收：插件清单说了能碰什么，内核就只让它碰什么。
//!
//! 覆盖：
//! 1. 表范围：清单只允许 `records` → 读别的表被拒，并在 `_labnote/denied.log` 留痕
//! 2. 只读：`permissions.write: false` → 写入一律被拒
//! 3. 允许写 + 允许的表 → 正常写入（不影响正常插件干活）
//! 4. 人/脚本（没有 `CERCIS_PLUGIN_ID`）不受影响：开放
//! 5. 联网批准：`trust --allow-network` 记进 `plugins/trust.yaml`

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-plugin-guard-flow-{name}"));
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

#[test]
fn plugin_context_limits_tables_and_writes() {
    let dir = tmp("scope");
    let project = dir.join("proj");
    let p = project.to_str().unwrap();
    let (code, _, err) = run(&["init", p, "--name", "权限降级试验"], &[]);
    assert_eq!(code, 0, "{err}");
    let (code, _, err) = run(
        &[
            "append",
            p,
            "--table",
            "records",
            "--set",
            "record_id=R-1",
            "--allow-new-columns",
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    // 再建一张"插件清单里没有"的表
    let (code, _, err) = run(
        &[
            "append",
            p,
            "--table",
            "private_notes",
            "--set",
            "note_id=N-1",
            "--allow-new-columns",
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");

    // 插件 A：只允许读 records、只读
    let plugin_env: Vec<(&str, &str)> = vec![
        ("CERCIS_PLUGIN_ID", "demo_reader"),
        ("CERCIS_PLUGIN_TABLES", "records"),
        ("CERCIS_PLUGIN_WRITE", "0"),
        ("CERCIS_PLUGIN_NETWORK", "off"),
    ];
    let (code, out, _) = run(&["show", p, "--table", "records"], &plugin_env);
    assert_eq!(code, 0, "允许的表应该读得到：{out}");

    let (code, _, err) = run(&["show", p, "--table", "private_notes"], &plugin_env);
    assert_ne!(code, 0, "清单外的表不该读得到");
    assert!(err.contains("没有读表"), "{err}");
    assert!(err.contains("demo_reader"), "{err}");
    // 系统表（流水/更正）是内核自己的账本：为了让"写入必须留痕"做得到，永远放行
    let (code, _, err) = run(&["show", p, "--table", "change_log"], &plugin_env);
    assert_eq!(code, 0, "系统表应放行：{err}");

    let (code, _, err) = run(
        &[
            "append",
            p,
            "--table",
            "records",
            "--set",
            "record_id=R-2",
            "--allow-new-columns",
        ],
        &plugin_env,
    );
    assert_ne!(code, 0, "只读插件不该写得动");
    assert!(err.contains("只读"), "{err}");

    // 越权留痕（故意不走内核写入通道）
    let denied = std::fs::read_to_string(project.join("_labnote/denied.log")).unwrap();
    assert!(denied.contains("demo_reader"), "{denied}");
    assert!(denied.contains("private_notes"), "{denied}");
    assert!(denied.contains("permissions.write"), "{denied}");

    // 插件 B：允许写、只碰 records
    let writer_env: Vec<(&str, &str)> = vec![
        ("CERCIS_PLUGIN_ID", "demo_writer"),
        ("CERCIS_PLUGIN_TABLES", "records"),
        ("CERCIS_PLUGIN_WRITE", "1"),
    ];
    let (code, _, err) = run(
        &[
            "append",
            p,
            "--table",
            "records",
            "--set",
            "record_id=R-2",
            "--allow-new-columns",
        ],
        &writer_env,
    );
    assert_eq!(code, 0, "声明了写就写得动：{err}");
    // 但清单外的表仍然写不进去
    let (code, _, err) = run(
        &[
            "append",
            p,
            "--table",
            "private_notes",
            "--set",
            "note_id=N-2",
            "--allow-new-columns",
        ],
        &writer_env,
    );
    assert_ne!(code, 0);
    assert!(err.contains("没有读表") || err.contains("没有"), "{err}");

    // 人（没有插件上下文）不受限制
    let (code, _, err) = run(&["show", p, "--table", "change_log"], &[]);
    assert_eq!(code, 0, "人用命令行不该被插件权限拦住：{err}");
}

#[test]
fn trust_can_approve_network_explicitly() {
    let dir = tmp("network");
    let project = dir.join("proj");
    let p = project.to_str().unwrap();
    let (code, _, err) = run(&["init", p, "--name", "联网批准试验"], &[]);
    assert_eq!(code, 0, "{err}");

    // 造一个"项目内插件"：清单声明联网 + 允许的表
    let plugin_dir = project.join("plugins/demo_net");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.yaml"),
        "id: demo_net\nnamespace: demo\nname: Demo Net\nlabel_cn: 演示联网\nversion: 0.1.0\n\
         tier: third-party\nrequires_plugin_api: 1\nentrypoint: demo_net\n\
         permissions:\n  read: true\n  write: false\n  network: [github.com]\n  tables: [records]\n\
         ui: []\n",
    )
    .unwrap();
    std::fs::write(plugin_dir.join("demo_net"), "#!/bin/sh\necho '{}'\n").unwrap();

    // 没批准联网时：信任记录里没有它
    let (code, _, err) = run(&["trust", p, "--id", "demo_net"], &[]);
    assert_eq!(code, 0, "{err}");
    let trust = std::fs::read_to_string(project.join("plugins/trust.yaml")).unwrap();
    assert!(
        !trust.contains("demo_net\n") || !trust.contains("- demo_net"),
        "还没批准过，network_allowed 里不该有它：{trust}"
    );

    // 明确点头：记进 network_allowed
    let (code, out, err) = run(
        &["trust", p, "--id", "demo_net", "--allow-network"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("已批准") || out.contains("github.com"), "{out}");
    let trust = std::fs::read_to_string(project.join("plugins/trust.yaml")).unwrap();
    assert!(trust.contains("demo_net"), "{trust}");
    assert!(trust.contains("network_allowed"), "{trust}");

    // 反悔：--deny-network 撤掉
    let (code, _, err) = run(
        &["trust", p, "--id", "demo_net", "--deny-network"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let trust = std::fs::read_to_string(project.join("plugins/trust.yaml")).unwrap();
    assert!(
        !trust.contains("- demo_net"),
        "撤销后 network_allowed 里不该还有它：{trust}"
    );
}

#[test]
fn plugins_list_shows_scope_and_network() {
    let dir = tmp("list");
    let project = dir.join("proj");
    let p = project.to_str().unwrap();
    run(&["init", p, "--name", "清单展示"], &[]);
    let plugin_dir = project.join("plugins/demo_scope");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.yaml"),
        "id: demo_scope\nnamespace: demo\nname: Demo\nlabel_cn: 演示\nversion: 0.1.0\n\
         tier: third-party\nrequires_plugin_api: 1\nentrypoint: demo_scope\n\
         permissions:\n  read: true\n  write: false\n  network: false\n  tables: [records, samples]\n\
         ui: []\n",
    )
    .unwrap();
    std::fs::write(plugin_dir.join("demo_scope"), "#!/bin/sh\necho '{}'\n").unwrap();
    let (code, out, err) = run(&["plugins", p, "--json"], &[]);
    assert_eq!(code, 0, "{err}");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    let plugins = v["plugins"].as_array().unwrap();
    let demo = plugins
        .iter()
        .find(|x| x["id"] == "demo_scope")
        .expect("插件应在列表里");
    assert_eq!(demo["permission_detail"]["tables"][0], "records");
    assert_eq!(demo["permission_detail"]["write"], false);
    assert_eq!(demo["permission_detail"]["network"], false);
    assert_eq!(demo["permission_detail"]["network_allowed"], false);
}
