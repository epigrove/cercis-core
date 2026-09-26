//! S12 验收：打包与合并（跨进程跑真的 `cercis` 命令）。
//!
//! 覆盖需求 FR-I01–FR-I03：
//! 1. 打包：全量 / 增量 / 分卷；包里有清单、每个文件带指纹
//! 2. 合并：**并集**（两台设备各自新增的记录都保留，同一条不装两遍）
//! 3. 校验：包里文件被改坏 → 整包拒绝，目标项目一个文件都不动
//! 4. 冲突：两边都改过的可变文件 → 本地不动，对方那份留 `.conflict-…` 副本
//! 5. 加密区域：包里的密文原样搬运（不含明文）；要并集加密表必须先解锁

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cercis"))
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-pack-flow-{name}"));
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

fn append(project: &Path, id: &str, envs: &[(&str, &str)]) {
    ok(
        &[
            "append",
            project.to_str().unwrap(),
            "--table",
            "records",
            "--set",
            &format!("record_id={id}"),
            "--set",
            &format!("notes={id} 的备注"),
            "--set",
            "value_num=1",
            "--allow-new-columns",
        ],
        envs,
    );
}

fn read_rows(project: &Path, envs: &[(&str, &str)]) -> Vec<String> {
    let v = ok_json(
        &[
            "query",
            project.to_str().unwrap(),
            "--table",
            "records",
            "--json",
        ],
        envs,
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

/// 递归复制一个项目（模拟"拷到第二台设备"），跳过 .git。
fn copy_project(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        let name = e.file_name();
        if name.to_string_lossy() == ".git" {
            continue;
        }
        let dest = to.join(&name);
        if p.is_dir() {
            copy_project(&p, &dest);
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

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

#[test]
fn pack_then_merge_brings_new_files_over() {
    let dir = tmp("basic");
    let a = dir.join("a");
    let b = dir.join("b");
    ok(
        &["init", a.to_str().unwrap(), "--name", "搬运试验"],
        &[],
    );
    append(&a, "R-1", &[]);
    copy_project(&a, &b);
    // 乙机新增一条
    append(&b, "R-2", &[]);

    let pack_dir = dir.join("包");
    let rep = ok_json(
        &[
            "pack",
            b.to_str().unwrap(),
            "--out",
            pack_dir.to_str().unwrap(),
            "--json",
        ],
        &[],
    );
    assert_eq!(rep["ok"], true);
    assert!(rep["files"].as_u64().unwrap() >= 3, "至少有 project.yaml 与两张表");
    assert!(pack_dir.join("pack.json").exists());
    assert!(pack_dir.join("README.md").exists());

    let merged = ok_json(
        &[
            "merge",
            pack_dir.to_str().unwrap(),
            "--into",
            a.to_str().unwrap(),
            "--json",
        ],
        &[],
    );
    assert_eq!(merged["ok"], true);
    let ids = read_rows(&a, &[]);
    assert!(ids.contains(&"R-1".to_string()));
    assert!(ids.contains(&"R-2".to_string()));
    assert_eq!(ids.len(), 2, "同一行不该装两遍");

    // 报告与流水
    let report = PathBuf::from(merged["report_path"].as_str().unwrap());
    assert!(report.exists(), "应写出合并报告");
    let text = std::fs::read_to_string(&report).unwrap();
    assert!(text.contains("并集合并"), "{text}");
    let log = ok_json(&["log", a.to_str().unwrap(), "--limit", "5", "--json"], &[]);
    assert!(
        log["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["action"] == "merge"),
        "合并要留一条流水"
    );
}

#[test]
fn incremental_pack_skips_unchanged_and_unions() {
    let dir = tmp("incremental");
    let a = dir.join("a");
    let b = dir.join("b");
    ok(&["init", a.to_str().unwrap(), "--name", "增量试验"], &[]);
    append(&a, "R-1", &[]);

    let pack1 = dir.join("pack-1");
    ok(
        &[
            "pack",
            a.to_str().unwrap(),
            "--out",
            pack1.to_str().unwrap(),
        ],
        &[],
    );

    copy_project(&a, &b);
    append(&b, "R-2", &[]);

    let pack2 = dir.join("pack-2");
    let rep = ok_json(
        &[
            "pack",
            b.to_str().unwrap(),
            "--out",
            pack2.to_str().unwrap(),
            "--since",
            pack1.join("pack.json").to_str().unwrap(),
            "--json",
        ],
        &[],
    );
    assert!(
        rep["skipped_unchanged"].as_u64().unwrap() > 0,
        "没变的文件应被跳过：{rep}"
    );
    assert!(
        rep["files"].as_u64().unwrap() < 4,
        "增量包只该装变化的文件：{rep}"
    );

    ok(
        &[
            "merge",
            pack2.to_str().unwrap(),
            "--into",
            a.to_str().unwrap(),
        ],
        &[],
    );
    let ids = read_rows(&a, &[]);
    assert_eq!(ids.len(), 2);
}

#[test]
fn corrupted_pack_is_refused_and_target_untouched() {
    let dir = tmp("corrupt");
    let a = dir.join("a");
    let b = dir.join("b");
    ok(&["init", a.to_str().unwrap(), "--name", "校验试验"], &[]);
    append(&a, "R-1", &[]);

    let pack_dir = dir.join("pack");
    ok(
        &[
            "pack",
            a.to_str().unwrap(),
            "--out",
            pack_dir.to_str().unwrap(),
        ],
        &[],
    );
    // 把包里的一行改掉
    let target = pack_dir.join("part-001/tables/records.csv");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.extend_from_slice(b"\n");
    std::fs::write(&target, bytes).unwrap();

    copy_project(&a, &b);
    std::fs::remove_file(b.join("tables/records.csv")).unwrap();
    let err = fail(
        &[
            "merge",
            pack_dir.to_str().unwrap(),
            "--into",
            b.to_str().unwrap(),
        ],
        &[],
    );
    assert!(err.contains("校验"), "{err}");
    assert!(
        !b.join("tables/records.csv").exists(),
        "校验没过时不该动目标项目"
    );
}

#[test]
fn conflict_copies_keep_both_sides_of_docs() {
    let dir = tmp("conflict");
    let a = dir.join("a");
    let b = dir.join("b");
    ok(&["init", a.to_str().unwrap(), "--name", "冲突试验"], &[]);
    append(&a, "R-1", &[]);
    copy_project(&a, &b);

    std::fs::create_dir_all(a.join("methods/记录")).unwrap();
    std::fs::create_dir_all(b.join("methods/记录")).unwrap();
    std::fs::write(a.join("methods/记录/方法.md"), "甲机版本\n").unwrap();
    std::fs::write(b.join("methods/记录/方法.md"), "乙机版本\n").unwrap();

    let pack_dir = dir.join("pack");
    ok(
        &[
            "pack",
            b.to_str().unwrap(),
            "--out",
            pack_dir.to_str().unwrap(),
        ],
        &[],
    );
    let rep = ok_json(
        &[
            "merge",
            pack_dir.to_str().unwrap(),
            "--into",
            a.to_str().unwrap(),
            "--json",
        ],
        &[],
    );
    assert_eq!(rep["conflicts"].as_array().unwrap().len(), 1, "{rep}");
    assert_eq!(
        std::fs::read_to_string(a.join("methods/记录/方法.md")).unwrap(),
        "甲机版本\n",
        "本地那份永不被覆盖"
    );
    let wrote = rep["conflicts"][0]["wrote"].as_str().unwrap();
    assert!(a.join(wrote).exists(), "对方那份要留成副本");
    assert!(wrote.contains(".conflict-"));
}

#[test]
fn volume_split_and_manifest_match() {
    let dir = tmp("parts");
    let a = dir.join("a");
    ok(&["init", a.to_str().unwrap(), "--name", "分卷试验"], &[]);
    append(&a, "R-1", &[]);

    // 整包一卷
    let single = dir.join("single");
    let rep = ok_json(
        &[
            "pack",
            a.to_str().unwrap(),
            "--out",
            single.to_str().unwrap(),
            "--part-size-mb",
            "0",
            "--json",
        ],
        &[],
    );
    assert_eq!(rep["parts"].as_array().unwrap().len(), 1);

    // 放一个 1.5 MB 的"照片"，每卷 1 MB → 必然多卷
    std::fs::create_dir_all(a.join("media")).unwrap();
    std::fs::write(a.join("media/big.bin"), vec![7u8; 1_500_000]).unwrap();
    let many = dir.join("many");
    let rep = ok_json(
        &[
            "pack",
            a.to_str().unwrap(),
            "--out",
            many.to_str().unwrap(),
            "--part-size-mb",
            "1",
            "--json",
        ],
        &[],
    );
    assert!(
        rep["parts"].as_array().unwrap().len() > 1,
        "每卷 1 MB 时应分成多卷：{rep}"
    );
    // 大文件独占一卷，且完整（没有被切开）
    let manifest = serde_json::from_str::<Value>(
        &std::fs::read_to_string(many.join("pack.json")).unwrap(),
    )
    .unwrap();
    let big = manifest["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == "media/big.bin")
        .expect("大文件也在清单里");
    let part = big["part"].as_str().unwrap();
    assert_eq!(
        std::fs::metadata(many.join(part).join("media/big.bin"))
            .unwrap()
            .len(),
        1_500_000,
        "文件不能被切碎"
    );
}

#[test]
fn encrypted_tables_stay_ciphertext_and_need_unlock_to_merge() {
    let dir = tmp("encrypted");
    let a = dir.join("a");
    let b = dir.join("b");
    let home = dir.join("share-home");
    std::fs::create_dir_all(&home).unwrap();
    let envs: Vec<(&str, &str)> = vec![
        ("CERCIS_SHARE_HOME", home.to_str().unwrap()),
        ("CERCIS_SHARE_KDF", "light"),
        ("CERCIS_KEYCHAIN", "off"),
    ];
    ok(&["init", a.to_str().unwrap(), "--name", "加密搬运"], &envs);
    append(&a, "R-1", &envs);
    // 建成员 + 区域（把 records.csv 变成密文）
    let mut env_pw = envs.clone();
    env_pw.push(("CERCIS_TEST_PW", "alice-pw"));
    ok(
        &[
            "share",
            "member-add",
            a.to_str().unwrap(),
            "--id",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &env_pw,
    );
    ok(
        &[
            "share",
            "zone-create",
            a.to_str().unwrap(),
            "--name",
            "secret",
            "--path",
            "tables/records.csv",
            "--member",
            "alice:admin",
        ],
        &envs,
    );

    // 打包：包里只有密文，没有明文
    let pack_dir = dir.join("pack");
    ok(
        &[
            "pack",
            a.to_str().unwrap(),
            "--out",
            pack_dir.to_str().unwrap(),
        ],
        &envs,
    );
    assert!(pack_dir.join("part-001/tables/records.csv.enc").exists());
    assert!(
        !find_marker(&pack_dir, "R-1").contains(&pack_dir.join("part-001/tables/records.csv").display().to_string()),
        "包里不该有明文表文件"
    );

    // 第二台设备：同一份项目（共享加密声明），各加一条
    copy_project(&a, &b);
    let mut env_pw_b = envs.clone();
    env_pw_b.push(("CERCIS_TEST_PW", "alice-pw"));
    ok(
        &[
            "share",
            "unlock",
            b.to_str().unwrap(),
            "--member",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &env_pw_b,
    );
    append(&b, "R-2", &envs);
    ok(
        &[
            "share",
            "lock",
            b.to_str().unwrap(),
        ],
        &envs,
    );
    let pack_b = dir.join("pack-b");
    ok(
        &[
            "pack",
            b.to_str().unwrap(),
            "--out",
            pack_b.to_str().unwrap(),
        ],
        &envs,
    );

    // 没解锁就合并：加密表会被拒绝（不能瞎合并密文），但要如实报告
    let rep = ok_json(
        &[
            "merge",
            pack_b.to_str().unwrap(),
            "--into",
            a.to_str().unwrap(),
            "--dry-run",
            "--json",
        ],
        &envs,
    );
    assert!(
        rep["refused"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap_or("").contains("records")),
        "没解锁时应拒绝合并加密表：{rep}"
    );

    // 解锁后合并：两份密文各解各的，并集写回密文
    ok(
        &[
            "share",
            "unlock",
            a.to_str().unwrap(),
            "--member",
            "alice",
            "--password-env",
            "CERCIS_TEST_PW",
        ],
        &env_pw,
    );
    let rep = ok_json(
        &[
            "merge",
            pack_b.to_str().unwrap(),
            "--into",
            a.to_str().unwrap(),
            "--json",
        ],
        &envs,
    );
    assert!(
        rep["merged"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["table"] == "records" && m["encrypted"] == true),
        "加密表应被并集合并：{rep}"
    );
    let ids = read_rows(&a, &envs);
    assert_eq!(ids.len(), 2, "两边记录都要在");
    assert!(
        !a.join("tables/records.csv").exists(),
        "合并后仍然只有密文，不能留下明文"
    );
    assert_eq!(find_marker(&a, "R-2"), Vec::<String>::new(), "项目里不该有明文残留");
}
