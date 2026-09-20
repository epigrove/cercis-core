//! 校验：引用完整性 + 字段类型/范围（由模板驱动）。
//!
//! 分两个层级（文档 §8.1）：
//! - **结构完整性**（内核，不可关闭）：列名规范、主键唯一、引用目标存在
//! - **业务规则**（插件可配置）：取值范围、允许值、必填等
//!
//! 本模块输出**可读报告**（错误阻止写入；警告放行但记录），不直接中断流程。

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::format;
use crate::store::Store;
use crate::table::Table;
use crate::template::{FieldDef, Template};
use crate::{CercisError, Result};

/// 单条问题。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    /// 级别：error / warning
    pub level: String,
    /// 表名
    pub table: String,
    /// 行号（1 起，含表头时为表内物理行号）
    pub row: usize,
    /// 列名（可为空）
    pub field: String,
    /// 可读说明
    pub message: String,
}

/// 校验报告。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Report {
    pub errors: Vec<Issue>,
    pub warnings: Vec<Issue>,
}

impl Report {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }

    pub fn push_error(&mut self, table: &str, row: usize, field: &str, message: String) {
        self.errors.push(Issue {
            level: "error".into(),
            table: table.into(),
            row,
            field: field.into(),
            message,
        });
    }

    pub fn push_warning(&mut self, table: &str, row: usize, field: &str, message: String) {
        self.warnings.push(Issue {
            level: "warning".into(),
            table: table.into(),
            row,
            field: field.into(),
            message,
        });
    }
}

/// 对单个项目（给定模板）做整体校验。
///
/// 检查内容：
/// 1. 每张表是否存在、列是否与模板一致
/// 2. 主键唯一、必填项非空
/// 3. 字段类型与范围
/// 4. 引用完整性（引用的记录必须存在）
pub fn validate_project(store: &Store, template: &Template) -> Result<Report> {
    template.validate()?;
    let mut report = Report::default();

    // 先载入所有存在的表（缺失的表记为警告：模板声明了但尚未使用）
    let mut tables: BTreeMap<String, Table> = BTreeMap::new();
    for t in &template.tables {
        let path = store.root().join("tables").join(format!("{}.csv", t.name));
        if path.exists() {
            let table = Table::load(&path)?;
            report.check_template_columns(t, &table);
            tables.insert(t.name.clone(), table);
        } else {
            report.push_warning(&t.name, 0, "", "模板声明的表尚不存在（未使用）".into());
        }
    }

    for t in &template.tables {
        let Some(table) = tables.get(&t.name) else {
            continue;
        };
        validate_table(t, table, template, &tables, &mut report)?;
    }

    Ok(report)
}

impl Report {
    /// 表列与模板列的一致性检查（结构层）。
    fn check_template_columns(&mut self, t: &crate::template::TableDef, table: &Table) {
        for col in &table.columns {
            if t.fields.iter().all(|f| f.name != *col) {
                self.push_warning(
                    &t.name,
                    0,
                    col,
                    format!("列 {col} 不在模板定义中（可能由插件扩展加入）"),
                );
            }
        }
        for f in &t.fields {
            if !table.columns.contains(&f.name) {
                self.push_error(
                    &t.name,
                    0,
                    &f.name,
                    format!("模板要求的列 {} 在表中缺失", f.name),
                );
            }
        }
    }
}

fn validate_table(
    t: &crate::template::TableDef,
    table: &Table,
    template: &Template,
    all_tables: &BTreeMap<String, Table>,
    report: &mut Report,
) -> Result<()> {
    // 主键唯一性
    if !t.primary_key.is_empty() && table.columns.contains(&t.primary_key) {
        let mut seen: BTreeMap<String, usize> = BTreeMap::new();
        for (i, row) in table.rows.iter().enumerate() {
            let key = row.get(&t.primary_key).cloned().unwrap_or_default();
            if key.trim().is_empty() {
                report.push_error(&t.name, i + 2, &t.primary_key, "主键不能为空".into());
                continue;
            }
            if let Some(prev) = seen.insert(key.clone(), i + 2) {
                report.push_error(
                    &t.name,
                    i + 2,
                    &t.primary_key,
                    format!("主键 {key} 重复（首次出现在第 {prev} 行）"),
                );
            }
        }
    }

    for (i, row) in table.rows.iter().enumerate() {
        let lineno = i + 2;
        for f in &t.fields {
            let Some(cell) = row.get(&f.name) else {
                continue;
            };
            validate_cell(t, f, cell, lineno, report);
        }
        // 引用完整性
        for f in &t.fields {
            let Some(reference) = &f.reference else {
                continue;
            };
            let Some(cell) = row.get(&f.name) else {
                continue;
            };
            if cell.trim().is_empty() {
                continue; // 引用列允许为空（除非 required）
            }
            let (rt, rf) = reference.split_once('.').expect("模板校验已保证格式");
            if let Some(target) = all_tables.get(rt) {
                let found = target
                    .rows
                    .iter()
                    .any(|r| r.get(rf).map(|v| v.as_str()) == Some(cell.as_str()));
                if !found {
                    report.push_error(
                        &t.name,
                        lineno,
                        &f.name,
                        format!("引用不存在：{reference} 中没有 {cell}"),
                    );
                }
            }
        }
    }
    let _ = template;
    Ok(())
}

fn validate_cell(
    t: &crate::template::TableDef,
    f: &FieldDef,
    cell: &str,
    lineno: usize,
    report: &mut Report,
) {
    let empty = cell.trim().is_empty();

    if empty {
        if f.required {
            report.push_error(&t.name, lineno, &f.name, "必填项为空".into());
        }
        return;
    }

    if format::is_missing_marker(cell) {
        report.push_warning(
            &t.name,
            lineno,
            &f.name,
            format!("疑似把缺失写成 {:?}；建议留空单元格", cell),
        );
    }

    match f.field_type.as_str() {
        "number" => {
            if !format::is_canonical_number(cell) {
                report.push_error(
                    &t.name,
                    lineno,
                    &f.name,
                    format!("不是规范数字写法: {cell:?}"),
                );
                return;
            }
            if let Some(range) = &f.range {
                if let Ok(v) = cell.trim().parse::<f64>() {
                    if let Some(Some(min)) = range.first() {
                        if v < *min {
                            report.push_warning(
                                &t.name,
                                lineno,
                                &f.name,
                                format!("小于建议下限 {min}（实际 {v}）"),
                            );
                        }
                    }
                    if let Some(Some(max)) = range.get(1) {
                        if v > *max {
                            report.push_warning(
                                &t.name,
                                lineno,
                                &f.name,
                                format!("大于建议上限 {max}（实际 {v}）"),
                            );
                        }
                    }
                }
            }
        }
        "integer" => {
            if cell.trim().parse::<i64>().is_err() {
                report.push_error(&t.name, lineno, &f.name, format!("不是整数: {cell:?}"));
            }
        }
        "date" => {
            if !format::is_iso_date(cell.trim()) {
                report.push_error(
                    &t.name,
                    lineno,
                    &f.name,
                    format!("不是 ISO 日期（YYYY-MM-DD）: {cell:?}"),
                );
            }
        }
        "datetime" => {
            if crate::ids::parse_iso8601(cell.trim()).is_err() {
                report.push_warning(
                    &t.name,
                    lineno,
                    &f.name,
                    format!("不是带时区的 ISO 时间: {cell:?}"),
                );
            }
        }
        "bool" if !matches!(cell.trim(), "是" | "否" | "true" | "false") => {
            report.push_warning(
                &t.name,
                lineno,
                &f.name,
                format!("布尔值建议使用 是/否: {cell:?}"),
            );
        }
        _ => {}
    }

    // 允许值
    if !f.allowed_values.is_empty() {
        let allowed: Vec<&str> = f
            .allowed_values
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !allowed.is_empty() && !allowed.contains(&cell.trim()) {
            report.push_warning(
                &t.name,
                lineno,
                &f.name,
                format!("取值 {cell:?} 不在允许值列表（{}）中", allowed.join("; ")),
            );
        }
    }

    if !format::is_nfc(cell) {
        report.push_warning(&t.name, lineno, &f.name, "未做 Unicode NFC 归一化".into());
    }
}

/// 把报告转成可读文本（CLI 输出用）。
pub fn report_to_text(r: &Report) -> String {
    let mut out = String::new();
    if r.errors.is_empty() && r.warnings.is_empty() {
        out.push_str("校验通过：未发现问题。\n");
        return out;
    }
    if !r.errors.is_empty() {
        out.push_str(&format!("错误 {} 项（会阻止写入）:\n", r.errors.len()));
        for i in &r.errors {
            out.push_str(&format!(
                "  [{}:{}] {}.{}: {}\n",
                i.table, i.row, i.table, i.field, i.message
            ));
        }
    }
    if !r.warnings.is_empty() {
        out.push_str(&format!("警告 {} 项（可放行）:\n", r.warnings.len()));
        for i in &r.warnings {
            out.push_str(&format!(
                "  [{}:{}] {}.{}: {}\n",
                i.table, i.row, i.table, i.field, i.message
            ));
        }
    }
    out
}

impl Store {
    /// 载入模板（若项目内没有则返回 None）。
    pub fn load_template(&self) -> Result<Option<Template>> {
        let p = self.root().join("template.yaml");
        if !p.exists() {
            return Ok(None);
        }
        Ok(Some(Template::load(p)?))
    }

    /// 保存模板到项目内。
    pub fn save_template(&self, t: &Template) -> Result<()> {
        t.save(self.root().join("template.yaml"))
    }

    /// 依据模板建表（缺列则创建空表；不改动已有数据的列）。
    pub fn ensure_tables_from_template(&self, t: &Template) -> Result<Vec<String>> {
        let mut created = Vec::new();
        for td in &t.tables {
            let path = self.root().join("tables").join(format!("{}.csv", td.name));
            if !path.exists() {
                let cols: Vec<&str> = td.fields.iter().map(|f| f.name.as_str()).collect();
                let table = crate::table::Table::load_or_create(path, &cols)?;
                table.save()?;
                created.push(td.name.clone());
            }
        }
        Ok(created)
    }

    /// 载入某张表并按模板检查列是否齐全。
    pub fn load_table_with_template(&self, name: &str, t: &Template) -> Result<Table> {
        let td = t
            .table(name)
            .ok_or_else(|| CercisError::NotFound(format!("模板中没有表: {name}")))?;
        let cols: Vec<&str> = td.fields.iter().map(|f| f.name.as_str()).collect();
        self.load_table(name, &cols)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Identity;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cercis-val-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn detects_missing_reference() {
        let dir = tmp("ref");
        let store = Store::new(&dir, Identity::default());
        let t = Template::example_study();
        store.ensure_tables_from_template(&t).unwrap();

        // 写一个引用不存在的 study_id 的实验单元
        let mut row = crate::table::Row::new();
        row.insert("unit_id".into(), "U-001".into());
        row.insert("study_id".into(), "ST-DOES-NOT-EXIST".into());
        store
            .append_row("units", &["unit_id", "study_id"], row, "U-001", "测试")
            .unwrap();

        let report = validate_project(&store, &t).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.message.contains("引用不存在")),
            "应报告引用失败: {:?}",
            report.errors
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepts_valid_reference() {
        let dir = tmp("ref-ok");
        let store = Store::new(&dir, Identity::default());
        let t = Template::example_study();
        store.ensure_tables_from_template(&t).unwrap();

        let mut s = crate::table::Row::new();
        s.insert("study_id".into(), "ST1".into());
        s.insert("name".into(), "示例研究".into());
        store
            .append_row("studies", &["study_id", "name"], s, "ST1", "测试")
            .unwrap();

        let mut u = crate::table::Row::new();
        u.insert("unit_id".into(), "U-001".into());
        u.insert("study_id".into(), "ST1".into());
        store
            .append_row("units", &["unit_id", "study_id"], u, "U-001", "测试")
            .unwrap();

        let report = validate_project(&store, &t).unwrap();
        assert!(report.errors.is_empty(), "不应有错误: {:?}", report.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_duplicate_primary_key_and_bad_type() {
        let dir = tmp("dup");
        let store = Store::new(&dir, Identity::default());
        let t = Template::example_study();
        store.ensure_tables_from_template(&t).unwrap();

        for _ in 0..2 {
            let mut r = crate::table::Row::new();
            r.insert("study_id".into(), "ST1".into());
            r.insert("name".into(), "示例".into());
            store
                .append_row("studies", &["study_id", "name"], r, "ST1", "测试")
                .unwrap();
        }

        // 写入一个非数字的 number 列
        let mut obs = crate::table::Row::new();
        obs.insert("obs_id".into(), "O-1".into());
        obs.insert("sample_id".into(), "S-1".into());
        obs.insert("trait_id".into(), "length".into());
        obs.insert("value_num".into(), "abc".into());
        store
            .append_row(
                "observations",
                &["obs_id", "sample_id", "trait_id", "value_num"],
                obs,
                "O-1",
                "测试",
            )
            .unwrap();

        let report = validate_project(&store, &t).unwrap();
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.message.contains("主键 ST1 重复")),
            "应报告主键重复"
        );
        assert!(
            report
                .errors
                .iter()
                .any(|e| e.field == "value_num" && e.message.contains("不是规范数字")),
            "应报告数字格式错误"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
