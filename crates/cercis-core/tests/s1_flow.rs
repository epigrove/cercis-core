//! S1 验收流程测试：建项目 → 写数据 → 更正 → 查流水 → 检查点 → 还原。
//!
//! 说明：这是**集成测试**，验证“数据只增不改 + 逐次存档 + 原子写入”这条主线。
//! 测试数据全部为**合成数据**，与任何真实研究无关。

use std::fs;

use cercis_core::history::ObjectStore;
use cercis_core::store::Identity;
use cercis_core::table::Row;
use cercis_core::Project;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-it-{name}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

#[test]
fn s1_full_flow() {
    let dir = tmp("flow");
    let identity = Identity {
        user_id: "tester".into(),
        device_id: "DEV-TEST-01".into(),
        record_device: true,
    };

    // 1. 建项目
    let p = Project::create(&dir, "示例项目（合成数据）", identity.clone()).unwrap();
    assert!(dir.join("project.yaml").exists());
    assert!(dir.join("tables").exists());

    // 2. 写一行（合成数据；列来自项目模板 records 表）
    let cols = ["record_id", "date", "label", "value_num"];
    let mut row = Row::new();
    row.insert("record_id".into(), "R-0001".into());
    row.insert("date".into(), "2026-03-15".into());
    row.insert("label".into(), "示例".into());
    row.insert("value_num".into(), "12.5".into());
    p.store
        .append_row("records", &cols, row, "R-0001", "录入合成数据")
        .unwrap();

    // 3. 更正（不覆盖原值）
    let cid = p
        .store
        .correct(
            "records",
            "R-0001",
            "value_num",
            "12.5",
            "12.7",
            "合成数据：模拟誊写笔误",
            "import/scans/example.pdf",
        )
        .unwrap();
    assert!(cid.starts_with("C-"));
    let corrections = p.store.load_table("corrections", &[]).unwrap();
    assert_eq!(corrections.rows.len(), 1);
    assert_eq!(corrections.rows[0]["old_value"], "12.5");
    assert_eq!(corrections.rows[0]["new_value"], "12.7");

    // 4. 原始数据文件仍在（未被更正覆盖）
    let recs = p.store.load_table("records", &[]).unwrap();
    assert_eq!(recs.rows[0]["value_num"], "12.5");

    // 5. 变更流水有记录，且带用户与设备
    let log = p.store.read_change_log().unwrap();
    assert!(log.len() >= 3, "至少有：建项目、写入、更正");
    let last = log.last().unwrap();
    assert_eq!(last.user_id, "tester");
    assert_eq!(last.device_id, "DEV-TEST-01");

    // 6. 检查点与历史对象：能把旧状态还原出来
    let cp = p.checkpoint("测试检查点").unwrap();
    assert!(cp.exists());

    let objects = ObjectStore::new(dir.join("history"));
    let old_text = "record_id,value_num\nR-0001,12.5\n";
    let hash = objects.put(old_text.as_bytes()).unwrap();
    let target = dir.join("tables/restored_check.csv");
    p.store
        .restore_object_to_file(&hash, &target, "还原测试")
        .unwrap();
    assert!(fs::read_to_string(&target).unwrap().contains("12.5"));

    // 7. 体检不报缺目录
    let issues = p.doctor().unwrap();
    assert!(issues.is_empty(), "体检不应有问题: {issues:?}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn atomic_write_leaves_no_tmp() {
    let dir = tmp("atomic");
    fs::create_dir_all(&dir).unwrap();
    let f = dir.join("a.csv");
    cercis_core::tx::write_atomic_str(&f, "x,y\n1,2\n").unwrap();
    assert!(f.exists());
    let leftovers = cercis_core::tx::clean_stale_tmp(&dir).unwrap();
    assert!(leftovers.is_empty());
    let _ = fs::remove_dir_all(&dir);
}
