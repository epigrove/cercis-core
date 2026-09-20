//! 批量写入（导入原语）的集成测试。
//!
//! 导入最怕两件事：**写了一半**、**日志被冲爆**。这两条就是本测试的验收点。
//! 数据全部为合成数据。

use std::fs;

use cercis_core::store::Identity;
use cercis_core::table::Row;
use cercis_core::Project;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-batch-{name}"));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn row(pairs: &[(&str, &str)]) -> Row {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn batch_append_writes_once_with_single_change_entry() {
    let dir = tmp("once");
    let p = Project::create(&dir, "批量导入（合成数据）", Identity::default()).unwrap();
    let cols = ["record_id", "date", "label", "value_num"];

    let rows = vec![
        row(&[
            ("record_id", "R-1001"),
            ("date", "2026-03-15"),
            ("label", "合成甲"),
            ("value_num", "12.5"),
        ]),
        row(&[
            ("record_id", "R-1002"),
            ("date", "2026-03-15"),
            ("label", "合成乙"),
            ("value_num", "13.0"),
        ]),
        row(&[
            ("record_id", "R-1003"),
            ("date", "2026-03-16"),
            ("label", "合成丙"),
            ("value_num", "9.75"),
        ]),
    ];

    let before = p.store.read_change_log().unwrap().len();
    let outcome = p
        .store
        .append_rows(
            "records",
            &cols,
            rows,
            "import",
            "导入 3 行（合成数据）",
            "imports/example.csv",
        )
        .unwrap();

    // 行数与内容
    assert_eq!(outcome.rows_written, 3);
    assert!(!outcome.batch_id.is_empty(), "批量写入必须有批次号");
    let table = p.store.load_table("records", &[]).unwrap();
    assert_eq!(table.rows.len(), 3);
    assert_eq!(table.rows[2]["value_num"], "9.75");

    // 流水：只多一条，且带上批次号、来源与动作
    let log = p.store.read_change_log().unwrap();
    assert_eq!(log.len(), before + 1, "整批导入只应产生一条流水");
    let entry = log.last().unwrap();
    assert_eq!(entry.action, "import");
    assert_eq!(entry.batch_id, outcome.batch_id);
    assert_eq!(entry.source_ref, "imports/example.csv");
    assert_eq!(entry.hash_after, outcome.hash_after);

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn batch_append_rejects_unknown_column_without_partial_write() {
    let dir = tmp("atomic-batch");
    let p = Project::create(&dir, "批量导入回滚（合成数据）", Identity::default()).unwrap();
    let cols = ["record_id", "date", "value_num"];

    let rows = vec![
        row(&[
            ("record_id", "R-2001"),
            ("date", "2026-04-01"),
            ("value_num", "1.0"),
        ]),
        // 第二行多出一个表中没有的列：整批必须被拒绝
        row(&[
            ("record_id", "R-2002"),
            ("date", "2026-04-01"),
            ("value_num", "2.0"),
            ("typo_column", "x"),
        ]),
    ];

    let before = p.store.read_change_log().unwrap().len();
    let err = p
        .store
        .append_rows("records", &cols, rows, "import", "导入", "imports/bad.csv")
        .unwrap_err();
    assert!(
        err.to_string().contains("typo_column"),
        "错误里应点名问题列: {err}"
    );

    // 表不存在（表未创建）或为空，流水没有新增——没有"写了一半"
    let log = p.store.read_change_log().unwrap();
    assert_eq!(log.len(), before, "失败的导入不应产生流水");
    let empty = p.store.load_table("records", &[]).unwrap();
    assert!(empty.rows.is_empty());

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn batch_append_rejects_empty_input() {
    let dir = tmp("empty");
    let p = Project::create(&dir, "空导入（合成数据）", Identity::default()).unwrap();
    let err = p
        .store
        .append_rows("records", &["record_id"], Vec::new(), "import", "", "")
        .unwrap_err();
    assert!(err.to_string().contains('0'), "空导入应报错: {err}");
    let _ = fs::remove_dir_all(&dir);
}
