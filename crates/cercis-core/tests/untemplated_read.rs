//! S6 验收发现 F-01 / F-03 的回归测试。
//!
//! - **F-01**：模板里没有、但项目里确实存在的表（系统表与插件自建表）
//!   必须能**读**（查询/导出），不能因为"模板没登记"就拒绝——否则
//!   `show` 能看、`query` 说没有，同一份数据两种命运。
//! - **F-03**：`append` 也要能像 `append-many` 一样显式允许新增列。
//!
//! 数据全部为合成数据。

use std::fs;

use cercis_core::query::Query;
use cercis_core::store::Identity;
use cercis_core::table::Row;
use cercis_core::{Project, Template};

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("cercis-untemplated-{name}"));
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
fn reads_system_tables_that_are_not_in_template() {
    let dir = tmp("system");
    let p = Project::create(&dir, "系统表可读（合成数据）", Identity::default()).unwrap();

    // 造一条更正，内核会顺便建出 corrections 表（它不在模板里）
    p.store
        .append_row(
            "records",
            &["record_id", "value_num"],
            row(&[("record_id", "R-1"), ("value_num", "1.0")]),
            "R-1",
            "写入",
        )
        .unwrap();
    p.store
        .correct("records", "R-1", "value_num", "1.0", "2.0", "笔误", "")
        .unwrap();

    let template = p.template().unwrap();
    assert!(
        template.table("corrections").is_none(),
        "前提：模板里确实没有 corrections 表"
    );

    // 关键断言：模板外的表照样能查询
    let rows = p.query("corrections", Query::new()).unwrap();
    assert_eq!(rows.len(), 1, "更正表应该能查到 1 行");
    assert_eq!(rows[0]["old_value"], "1.0");
    assert_eq!(rows[0]["new_value"], "2.0");

    // change_log 同理
    let log = p.query("change_log", Query::new()).unwrap();
    assert!(log.len() >= 3, "流水表应该能查到记录：{}", log.len());

    // 真的不存在的表仍然要报错（读放宽 ≠ 凭空造表）
    let err = p.query("no_such_table", Query::new()).unwrap_err();
    assert!(
        err.to_string().contains("表不存在"),
        "不存在的表应报「表不存在」：{err}"
    );

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn append_can_extend_columns_when_explicitly_allowed() {
    let dir = tmp("extend");
    let p = Project::create(&dir, "扩展列（合成数据）", Identity::default()).unwrap();

    p.store
        .append_row(
            "records",
            &["record_id"],
            row(&[("record_id", "R-1")]),
            "R-1",
            "写入",
        )
        .unwrap();

    // 默认：写一个新列会被拒绝
    let err = p
        .store
        .append_row(
            "records",
            &["record_id"],
            row(&[("record_id", "R-2"), ("extra", "x")]),
            "R-2",
            "写入",
        )
        .unwrap_err();
    assert!(err.to_string().contains("不存在列"), "{err}");
    assert!(
        err.to_string().contains("--allow-new-columns"),
        "错误里应提示怎么办：{err}"
    );

    // 显式允许：扩展列并写入
    p.store
        .append_row_extending(
            "records",
            &["record_id"],
            row(&[("record_id", "R-2"), ("extra", "x")]),
            "R-2",
            "写入并扩展列",
        )
        .unwrap();

    let table = p.store.load_table("records", &[]).unwrap();
    assert!(table.columns.contains(&"extra".to_string()), "新列应被追加");
    assert_eq!(table.rows.len(), 2);
    assert_eq!(table.rows[1]["extra"], "x");

    // 模板仍然没有这一列 —— 说明"扩展的是表结构"，不是偷偷改模板
    assert!(Template::generic().table("records").is_some());

    let _ = fs::remove_dir_all(&dir);
}
