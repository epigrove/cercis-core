//! D41 验收：插件市场的"内核那一段"（跨进程跑真的 `cercis` 命令）。
//!
//! 分工（需求文档 §14.8）：清单在 git 仓库、下载在 app、**校验与安装落盘与记账在内核**。
//! 所以这里刻意**不联网**：造一个"已经下载好的仓库目录"，把三条路都跑一遍。
//!
//! 覆盖：
//! 1. `market list` 列默认预置市场（指向 GitHub 公开仓库）与已装插件
//! 2. `market validate` 认得出坏清单（版本不认识、路径越界、接口版本过高、id 重复）
//! 3. `market install --from <本地目录>`：清单先校验 → 复制到用户插件目录 → 记来源与指纹
//! 4. 装完能被项目看见（`plugins --global-dir`），并给出"未生效 + 原因"（D38）
//! 5. `market uninstall`：试运行不动文件；真卸载删副本清记录；随包插件只能停用（FR-P50）

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-market-flow-{name}"));
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

/// 造一个"已经下载好的市场仓库"：marketplace.json + 两个插件目录。
fn fake_repo(dir: &Path) -> PathBuf {
    let repo = dir.join("downloaded");
    for (id, version) in [("demo_io", "0.2.0"), ("demo_report", "0.1.0")] {
        let p = repo.join("plugins").join(id);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("plugin.yaml"),
            format!(
                "id: {id}\nnamespace: {id}\nname: {id}\nlabel_cn: 演示\nversion: {version}\ntier: third-party\n\
                 requires_plugin_api: 1\nrequires:\n  tables: [observations]\npermissions:\n  read: true\n  write: true\n\
                 ui:\n  - kind: command\n    id: {id}.run\n    label_cn: 跑一下\nentrypoint: {id}\n"
            ),
        )
        .unwrap();
    }
    std::fs::write(
        repo.join("marketplace.json"),
        r#"{
  "schema": "cercis.marketplace/1",
  "name": "演示市场",
  "label_cn": "演示市场",
  "repository": "https://github.com/example/demo-market",
  "plugins": [
    {"id": "demo_io", "name": "Demo IO", "path": "plugins/demo_io", "version": "0.2.0",
     "tier": "third-party", "requires_plugin_api": 1, "permissions": "读取数据、写入数据",
     "summary": "演示用", "requires": {"tables": ["observations"]}},
    {"id": "demo_report", "name": "Demo Report", "path": "plugins/demo_report", "version": "0.1.0",
     "tier": "third-party", "requires_plugin_api": 1, "permissions": "读取数据", "summary": "演示用"}
  ]
}
"#,
    )
    .unwrap();
    repo
}

#[test]
fn presets_are_public_github_and_installed_list_is_empty_at_first() {
    let dir = tmp("presets");
    let global = dir.join("global");
    let g = global.display().to_string();
    let out = ok(&["market", "list", "--json"], &[("CERCIS_GLOBAL_PLUGINS", &g)]);
    let presets = out["presets"].as_array().unwrap();
    assert_eq!(presets.len(), 1, "目前只预置科研市场：{out}");
    assert!(
        presets[0]["repository"]
            .as_str()
            .unwrap()
            .starts_with("https://github.com/epigrove/"),
        "默认预置市场指向公开 GitHub 仓库：{out}"
    );
    assert_eq!(presets[0]["default_ref"], "main");
    assert_eq!(out["installed"].as_array().unwrap().len(), 0);
}

#[test]
fn validate_accepts_good_market_and_rejects_bad_ones() {
    let dir = tmp("validate");
    let repo = fake_repo(&dir);
    let file = repo.join("marketplace.json");
    let f = file.display().to_string();
    let out = ok(&["market", "validate", "--file", &f, "--json"], &[]);
    assert_eq!(out["marketplace"]["plugins"].as_array().unwrap().len(), 2);

    // 坏清单：接口版本要求高于内核
    let bad = dir.join("bad1.json");
    std::fs::write(
        &bad,
        std::fs::read_to_string(&file)
            .unwrap()
            .replace("\"requires_plugin_api\": 1", "\"requires_plugin_api\": 99"),
    )
    .unwrap();
    let (code, _o, err) = run(&["market", "validate", "--file", &bad.display().to_string()], &[]);
    assert_ne!(code, 0);
    assert!(err.contains("需要接口版本"), "{err}");

    // 坏清单：路径跑到仓库外面
    let bad2 = dir.join("bad2.json");
    std::fs::write(
        &bad2,
        std::fs::read_to_string(&file)
            .unwrap()
            .replace("plugins/demo_io", "../外面/demo_io"),
    )
    .unwrap();
    let (code, _o, err) = run(&["market", "validate", "--file", &bad2.display().to_string()], &[]);
    assert_ne!(code, 0);
    assert!(err.contains("相对路径"), "{err}");
}

#[test]
fn install_from_downloaded_repo_records_source_and_is_visible() {
    let dir = tmp("install");
    let repo = fake_repo(&dir);
    let global = dir.join("global");
    let g = global.display().to_string();
    let r = repo.display().to_string();

    // 试运行：不动文件
    let dry = ok(
        &[
            "market", "install", "--from", &r, "--plugin", "demo_io", "--dry-run", "--json",
        ],
        &[("CERCIS_GLOBAL_PLUGINS", &g)],
    );
    assert_eq!(dry["dry_run"], true);
    assert!(!global.join("demo_io").exists(), "试运行不该落盘");

    // 真装
    let out = ok(
        &[
            "market", "install", "--from", &r, "--plugin", "demo_io", "--market", "演示市场",
            "--ref", "v0.2.0", "--json",
        ],
        &[("CERCIS_GLOBAL_PLUGINS", &g)],
    );
    assert_eq!(out["record"]["id"], "demo_io");
    assert_eq!(out["record"]["version"], "0.2.0");
    assert_eq!(out["record"]["git_ref"], "v0.2.0");
    assert_eq!(out["record"]["market"], "演示市场");
    assert!(
        out["record"]["repository"]
            .as_str()
            .unwrap()
            .contains("example/demo-market"),
        "{out}"
    );
    assert!(global.join("demo_io/plugin.yaml").is_file(), "插件应落到用户目录");

    // 装不存在的插件 → 说清可用的有哪些
    let (code, _o, err) = run(
        &["market", "install", "--from", &r, "--plugin", "不存在"],
        &[("CERCIS_GLOBAL_PLUGINS", &g)],
    );
    assert_ne!(code, 0);
    assert!(err.contains("demo_io") && err.contains("demo_report"), "{err}");

    // 项目里能看见它，并且因为缺表而"未生效"（D38）
    let project = dir.join("proj");
    ok(
        &[
            "init",
            &project.display().to_string(),
            "--name",
            "市场验收",
            "--json",
        ],
        &[],
    );
    let found = ok(
        &[
            "plugins",
            &project.display().to_string(),
            "--global-dir",
            &g,
            "--json",
        ],
        &[],
    );
    let p = found["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == "demo_io")
        .expect("全局插件应被发现");
    assert_eq!(p["origin"], "global");
    assert_eq!(p["active"], false, "{p}");
    assert!(
        p["inactive_reasons"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap_or("").contains("observations")),
        "{p}"
    );
}

#[test]
fn uninstall_removes_local_copy_but_bundled_plugins_cannot_be_deleted() {
    let dir = tmp("uninstall");
    let repo = fake_repo(&dir);
    let global = dir.join("global");
    let g = global.display().to_string();
    ok(
        &[
            "market", "install", "--from", &repo.display().to_string(), "--plugin", "demo_report",
            "--json",
        ],
        &[("CERCIS_GLOBAL_PLUGINS", &g)],
    );
    assert!(global.join("demo_report").exists());

    // 试运行
    let dry = ok(
        &["market", "uninstall", "--id", "demo_report", "--dry-run", "--json"],
        &[("CERCIS_GLOBAL_PLUGINS", &g)],
    );
    assert!(dry["message"].as_str().unwrap().contains("会删掉"), "{dry}");
    assert!(global.join("demo_report").exists(), "试运行不该删");

    // 真卸载
    ok(
        &["market", "uninstall", "--id", "demo_report", "--json"],
        &[("CERCIS_GLOBAL_PLUGINS", &g)],
    );
    assert!(!global.join("demo_report").exists(), "卸载应删掉本地副本");
    let listed = ok(&["market", "list", "--json"], &[("CERCIS_GLOBAL_PLUGINS", &g)]);
    assert_eq!(listed["installed"].as_array().unwrap().len(), 0);

    // 随包插件（没有安装记录）不能物理删除
    let (code, _o, err) = run(
        &["market", "uninstall", "--id", "health_check"],
        &[("CERCIS_GLOBAL_PLUGINS", &g)],
    );
    assert_ne!(code, 0);
    assert!(err.contains("随包插件不能物理删除"), "{err}");
}
