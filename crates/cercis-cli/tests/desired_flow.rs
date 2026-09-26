//! D44 验收：期望状态对账 + 更新检查（跨进程跑真的 `cercis` 命令）。
//!
//! 覆盖：
//! 1. `plugins desired`：想装没装的、多余可删的、要停用/屏蔽的，一次说清
//! 2. `plugins reconcile --dry-run`：只算账不动文件
//! 3. `plugins reconcile --from <已下载目录>`：把缺的装上、把状态改对，都留痕
//! 4. `market check`：版本更新提示；**权限变化必须重新确认**；接口太新不能升

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-desired-flow-{name}"));
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

/// 造一份"已经下载好的市场仓库"：两个插件 + 清单。
fn fake_repo(dir: &Path, io_version: &str, io_permissions: &str) -> PathBuf {
    let repo = dir.join("downloaded");
    for (id, version) in [("research_io", io_version), ("research_scribe", "0.1.0")] {
        let p = repo.join("plugins").join(id);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("plugin.yaml"),
            format!(
                "id: {id}\nnamespace: research\nname: {id}\nversion: {version}\ntier: research\n\
                 requires_plugin_api: 1\npermissions:\n  read: true\n  write: true\n\
                 ui:\n  - kind: command\n    id: research.{id}_run\n    label_cn: 跑一下\nentrypoint: {id}\n"
            ),
        )
        .unwrap();
    }
    std::fs::write(
        repo.join("marketplace.json"),
        format!(
            r#"{{"schema": "cercis.marketplace/1", "name": "科研工具包",
                 "repository": "https://github.com/epigrove/cercis-kits-research",
                 "plugins": [
                   {{"id": "research_io", "path": "plugins/research_io", "version": "{io_version}",
                    "permissions": "{io_permissions}", "requires_plugin_api": 1}},
                   {{"id": "research_scribe", "path": "plugins/research_scribe", "version": "0.1.0",
                    "permissions": "读取数据、写入数据", "requires_plugin_api": 1}}
                 ]}}"#
        ),
    )
    .unwrap();
    repo
}

fn write_desired(project: &Path) {
    std::fs::create_dir_all(project.join("plugins")).unwrap();
    std::fs::write(
        project.join("plugins/desired.yaml"),
        "schema: cercis.desired/1\ninstall:\n  - {market: research, id: research_io}\n  - {market: research, id: research_scribe}\ndisable: [research_scribe]\nblock: []\n",
    )
    .unwrap();
}

#[test]
fn desired_diff_then_reconcile_aligns_everything() {
    let dir = tmp("align");
    let repo = fake_repo(&dir, "0.1.0", "读取数据、写入数据");
    let global = dir.join("global");
    let project = dir.join("proj");
    let g = global.display().to_string();
    let p = project.display().to_string();
    let envs: [(&str, &str); 1] = [("CERCIS_GLOBAL_PLUGINS", &g)];

    ok(&["init", &p, "--name", "期望状态验收", "--json"], &[]);
    write_desired(&project);

    // 对账：两个都要装，其中一个还要停用
    let d = ok(&["plugins", &p, "desired", "--json"], &envs);
    assert_eq!(d["aligned"], false);
    assert_eq!(d["diff"]["missing"].as_array().unwrap().len(), 2, "{d}");
    assert_eq!(d["diff"]["to_disable"][0], "research_scribe", "{d}");

    // dry-run：不动文件
    let dry = ok(&["plugins", &p, "reconcile", "--dry-run", "--json"], &envs);
    assert_eq!(dry["dry_run"], true);
    assert!(
        dry["applied"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x.as_str().unwrap().contains("停用 research_scribe")),
        "{dry}"
    );
    assert!(!project.join("plugins/state.yaml").exists(), "dry-run 不该落盘");
    assert!(!global.join("research_io").exists(), "dry-run 不该装东西");

    // 真对齐（带本地目录）
    let r = ok(
        &[
            "plugins", &p, "reconcile", "--from", &repo.display().to_string(), "--json",
        ],
        &envs,
    );
    assert_eq!(r["skipped"].as_array().unwrap().len(), 0, "不该有跳过的：{r}");
    assert!(global.join("research_io/plugin.yaml").is_file());
    assert!(global.join("research_scribe/plugin.yaml").is_file());
    let state = std::fs::read_to_string(project.join("plugins/state.yaml")).unwrap();
    assert!(state.contains("research_scribe"), "{state}");

    // 再对账 → 一致
    let d2 = ok(&["plugins", &p, "desired", "--json"], &envs);
    assert_eq!(d2["aligned"], true, "{d2}");

    // 对齐动作进了流水
    let log = ok(&["log", &p, "--limit", "20", "--json"], &[]);
    assert!(log.to_string().contains("reconcile"), "{log}");
}

#[test]
fn extra_plugins_are_reported_as_removable() {
    let dir = tmp("extra");
    let repo = fake_repo(&dir, "0.1.0", "读取数据、写入数据");
    let global = dir.join("global");
    let project = dir.join("proj");
    let g = global.display().to_string();
    let p = project.display().to_string();
    let envs: [(&str, &str); 1] = [("CERCIS_GLOBAL_PLUGINS", &g)];
    ok(&["init", &p, "--name", "多余插件验收", "--json"], &[]);

    // 先装一个，但期望状态里不想要它
    ok(
        &[
            "market", "install", "--from", &repo.display().to_string(), "--plugin", "research_io",
            "--market", "research", "--json",
        ],
        &envs,
    );
    create_empty_desired(&project);
    let d = ok(&["plugins", &p, "desired", "--json"], &envs);
    assert_eq!(d["diff"]["extra"][0], "research_io", "{d}");
}

fn create_empty_desired(project: &Path) {
    std::fs::create_dir_all(project.join("plugins")).unwrap();
    std::fs::write(
        project.join("plugins/desired.yaml"),
        "schema: cercis.desired/1\ninstall: []\ndisable: []\nblock: []\n",
    )
    .unwrap();
}

#[test]
fn market_check_only_hints_and_flags_permission_changes() {
    let dir = tmp("check");
    let v1 = fake_repo(&dir, "0.1.0", "读取数据、写入数据");
    let global = dir.join("global");
    let g = global.display().to_string();
    let envs: [(&str, &str); 1] = [("CERCIS_GLOBAL_PLUGINS", &g)];
    let project = dir.join("proj");
    ok(
        &["init", &project.display().to_string(), "--name", "更新检查", "--json"],
        &[],
    );
    ok(
        &[
            "market", "install", "--from", &v1.display().to_string(), "--plugin", "research_io",
            "--market", "research", "--json",
        ],
        &envs,
    );

    // 同版本 → 已是最新
    let same = ok(&["market", "check", "--from", &v1.display().to_string(), "--json"], &envs);
    assert_eq!(same["updates"][0]["status"], "up_to_date", "{same}");

    // 新版 + 多一项权限 → 可更新，且要重新确认
    let dir2 = tmp("check-v2");
    let v2 = fake_repo(&dir2, "0.2.0", "读取数据、写入数据、访问网络");
    let newer = ok(&["market", "check", "--from", &v2.display().to_string(), "--json"], &envs);
    let item = &newer["updates"][0];
    assert_eq!(item["status"], "update_available", "{newer}");
    assert_eq!(item["needs_reconfirmation"], true, "权限变了必须重新确认：{newer}");
    assert_eq!(item["available_version"], "0.2.0");

    // 顺手确认：检查本身不动任何东西（还是旧版本）
    let still = ok(&["market", "list", "--json"], &envs);
    assert_eq!(still["installed"][0]["version"], "0.1.0", "检查更新不该自动升级");
}
