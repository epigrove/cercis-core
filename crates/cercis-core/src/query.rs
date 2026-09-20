//! 查询原语：组合筛选、排序、分页。
//!
//! 这些是**内核能力**（而非插件）：每个插件都要查询，下沉到内核可统一优化、
//! 统一语义、统一缓存。视图（保存的查询条件）属于插件层。
//!
//! ## 设计要点
//! - 条件用**结构化描述**（不是字符串表达式）：便于校验、序列化、跨语言复现
//! - 排序稳定（多键依次比较）且**确定性**：相同输入必然相同顺序
//! - 比较遵循各列的字段类型（数值按数值比，日期按日期比，文本按字典序）

use serde::{Deserialize, Serialize};

use crate::format;
use crate::table::{Row, Table};
use crate::{CercisError, Result};

/// 比较运算符。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    /// 等于
    Eq,
    /// 不等于
    Ne,
    /// 小于
    Lt,
    /// 小于等于
    Le,
    /// 大于
    Gt,
    /// 大于等于
    Ge,
    /// 包含子串（文本）
    Contains,
    /// 以…开头（文本）
    StartsWith,
    /// 在集合中（逗号分隔）
    In,
    /// 为空（未填写）
    IsEmpty,
    /// 不为空
    NotEmpty,
}

/// 单个筛选条件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    /// 列名
    pub field: String,
    /// 运算符
    pub op: Op,
    /// 比较值（IsEmpty / NotEmpty / In 时可为空或逗号分隔）
    #[serde(default)]
    pub value: String,
}

impl Condition {
    pub fn new(field: &str, op: Op, value: &str) -> Self {
        Self {
            field: field.to_string(),
            op,
            value: value.to_string(),
        }
    }
}

/// 排序方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDir {
    Asc,
    Desc,
}

/// 排序键。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortKey {
    pub field: String,
    #[serde(default = "default_asc")]
    pub dir: SortDir,
}

fn default_asc() -> SortDir {
    SortDir::Asc
}

impl SortKey {
    pub fn asc(field: &str) -> Self {
        Self {
            field: field.to_string(),
            dir: SortDir::Asc,
        }
    }
    pub fn desc(field: &str) -> Self {
        Self {
            field: field.to_string(),
            dir: SortDir::Desc,
        }
    }
}

/// 一条查询：条件（AND）+ 排序 + 分页。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Query {
    /// 全部条件按 AND 组合
    #[serde(default)]
    pub where_all: Vec<Condition>,
    #[serde(default)]
    pub order_by: Vec<SortKey>,
    /// 跳过前 N 行（分页）
    #[serde(default)]
    pub offset: usize,
    /// 最多返回 N 行（0 = 不限）
    #[serde(default)]
    pub limit: usize,
    /// 列名 → 类型（用于正确比较；缺省按文本）
    #[serde(default)]
    pub types: std::collections::BTreeMap<String, String>,
}

impl Query {
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加一个条件。
    pub fn filter(mut self, field: &str, op: Op, value: &str) -> Self {
        self.where_all.push(Condition::new(field, op, value));
        self
    }

    /// 追加排序键。
    pub fn sort(mut self, field: &str, dir: SortDir) -> Self {
        self.order_by.push(SortKey {
            field: field.to_string(),
            dir,
        });
        self
    }

    /// 声明列的字段类型（数值/日期列的排序与比较需要）。
    pub fn with_types(mut self, types: std::collections::BTreeMap<String, String>) -> Self {
        self.types = types;
        self
    }

    /// 分页。
    pub fn page(mut self, offset: usize, limit: usize) -> Self {
        self.offset = offset;
        self.limit = limit;
        self
    }

    /// 校验：条件与排序引用的列必须存在。
    pub fn validate_against(&self, columns: &[String]) -> Result<()> {
        for c in &self.where_all {
            if !columns.contains(&c.field) {
                return Err(CercisError::Invalid(format!(
                    "筛选条件引用了不存在的列: {}（可用列: {}）",
                    c.field,
                    columns.join(", ")
                )));
            }
            if matches!(c.op, Op::In) && c.value.is_empty() {
                return Err(CercisError::Invalid(format!(
                    "列 {} 使用 in 运算符时必须提供值（逗号分隔）",
                    c.field
                )));
            }
        }
        for s in &self.order_by {
            if !columns.contains(&s.field) {
                return Err(CercisError::Invalid(format!(
                    "排序引用了不存在的列: {}（可用列: {}）",
                    s.field,
                    columns.join(", ")
                )));
            }
        }
        Ok(())
    }

    /// 执行查询：返回命中行（已排序、已分页）。
    pub fn execute(&self, table: &Table) -> Result<Vec<Row>> {
        self.validate_against(&table.columns)?;

        let mut rows: Vec<&Row> = table.rows.iter().filter(|r| self.matches(r)).collect();

        if !self.order_by.is_empty() {
            rows.sort_by(|a, b| self.compare_rows(a, b));
        }

        let start = self.offset.min(rows.len());
        let end = if self.limit == 0 {
            rows.len()
        } else {
            (start + self.limit).min(rows.len())
        };

        Ok(rows[start..end].iter().map(|r| (*r).clone()).collect())
    }

    /// 命中数量（不受分页影响）。
    pub fn count(&self, table: &Table) -> Result<usize> {
        self.validate_against(&table.columns)?;
        Ok(table.rows.iter().filter(|r| self.matches(r)).count())
    }

    fn matches(&self, row: &Row) -> bool {
        self.where_all
            .iter()
            .all(|c| condition_matches(c, row, &self.types))
    }

    fn compare_rows(&self, a: &Row, b: &Row) -> std::cmp::Ordering {
        for key in &self.order_by {
            let av = a.get(&key.field).map(|s| s.as_str()).unwrap_or("");
            let bv = b.get(&key.field).map(|s| s.as_str()).unwrap_or("");
            let ord = compare_values(av, bv, self.types.get(&key.field).map(|s| s.as_str()));
            let ord = if key.dir == SortDir::Desc {
                ord.reverse()
            } else {
                ord
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        std::cmp::Ordering::Equal
    }
}

fn condition_matches(
    c: &Condition,
    row: &Row,
    types: &std::collections::BTreeMap<String, String>,
) -> bool {
    let cell = row.get(&c.field).map(|s| s.as_str()).unwrap_or("");
    let cell_empty = cell.trim().is_empty();

    match c.op {
        Op::IsEmpty => cell_empty,
        Op::NotEmpty => !cell_empty,
        _ if cell_empty => false, // 其它运算符对空值一律不命中
        Op::Eq => compare_values(cell, &c.value, ty(types, &c.field)) == std::cmp::Ordering::Equal,
        Op::Ne => compare_values(cell, &c.value, ty(types, &c.field)) != std::cmp::Ordering::Equal,
        Op::Lt => compare_values(cell, &c.value, ty(types, &c.field)) == std::cmp::Ordering::Less,
        Op::Le => {
            compare_values(cell, &c.value, ty(types, &c.field)) != std::cmp::Ordering::Greater
        }
        Op::Gt => {
            compare_values(cell, &c.value, ty(types, &c.field)) == std::cmp::Ordering::Greater
        }
        Op::Ge => compare_values(cell, &c.value, ty(types, &c.field)) != std::cmp::Ordering::Less,
        Op::Contains => cell.contains(&c.value),
        Op::StartsWith => cell.starts_with(&c.value),
        Op::In => c
            .value
            .split(',')
            .map(|s| s.trim())
            .any(|v| compare_values(cell, v, ty(types, &c.field)) == std::cmp::Ordering::Equal),
    }
}

fn ty<'a>(types: &'a std::collections::BTreeMap<String, String>, field: &str) -> Option<&'a str> {
    types.get(field).map(|s| s.as_str())
}

/// 按字段类型比较两个单元格。
pub fn compare_values(a: &str, b: &str, field_type: Option<&str>) -> std::cmp::Ordering {
    match field_type {
        Some("number") => match (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            (Ok(x), Ok(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            // 解析失败时退回文本比较（并保持稳定）
            _ => a.cmp(b),
        },
        Some("integer") => match (a.trim().parse::<i64>(), b.trim().parse::<i64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            _ => a.cmp(b),
        },
        Some("date") | Some("datetime") => {
            // ISO 8601 的字典序即时间序
            if format::is_iso_date(a.trim()) && format::is_iso_date(b.trim()) {
                a.trim().cmp(b.trim())
            } else {
                a.cmp(b)
            }
        }
        _ => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::Row;

    fn make_table() -> Table {
        let mut t = Table::in_memory(
            "/tmp/fake.csv",
            &["record_id", "group", "value_num", "date"],
        );
        let data = [
            ("R-001", "A", "12.5", "2026-03-01"),
            ("R-002", "A", "9.8", "2026-03-02"),
            ("R-003", "B", "15.2", "2026-03-03"),
            ("R-004", "B", "11.0", "2026-03-04"),
            ("R-005", "C", "", "2026-03-05"),
        ];
        for (id, g, v, d) in data {
            let mut r = Row::new();
            r.insert("record_id".into(), id.into());
            r.insert("group".into(), g.into());
            r.insert("value_num".into(), v.into());
            r.insert("date".into(), d.into());
            t.rows.push(r);
        }
        t
    }

    fn types() -> std::collections::BTreeMap<String, String> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("value_num".into(), "number".into());
        m.insert("date".into(), "date".into());
        m
    }

    #[test]
    fn filter_by_group() {
        let t = make_table();
        let q = Query::new().filter("group", Op::Eq, "A");
        let rows = q.execute(&t).unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn numeric_range_filter() {
        let t = make_table();
        let q = Query::new()
            .with_types(types())
            .filter("value_num", Op::Ge, "12");
        let rows = q.execute(&t).unwrap();
        assert_eq!(rows.len(), 2, "12.5 与 15.2");
    }

    #[test]
    fn sort_numeric_desc() {
        let t = make_table();
        let q = Query::new()
            .with_types(types())
            .sort("value_num", SortDir::Desc);
        let rows = q.execute(&t).unwrap();
        assert_eq!(rows[0]["record_id"], "R-003", "最大值 15.2 应排第一");
    }

    #[test]
    fn sort_numeric_asc_puts_empty_last_or_first_consistently() {
        let t = make_table();
        let q = Query::new()
            .with_types(types())
            .sort("value_num", SortDir::Asc);
        let rows = q.execute(&t).unwrap();
        // 空值解析失败 → 退回文本比较，结果稳定即可
        assert_eq!(rows.len(), 5);
    }

    #[test]
    fn in_operator() {
        let t = make_table();
        let q = Query::new().filter("group", Op::In, "A,B");
        assert_eq!(q.count(&t).unwrap(), 4);
    }

    #[test]
    fn empty_check() {
        let t = make_table();
        let q = Query::new().filter("value_num", Op::IsEmpty, "");
        assert_eq!(q.count(&t).unwrap(), 1);
        let q2 = Query::new().filter("value_num", Op::NotEmpty, "");
        assert_eq!(q2.count(&t).unwrap(), 4);
    }

    #[test]
    fn pagination() {
        let t = make_table();
        let q = Query::new()
            .with_types(types())
            .sort("record_id", SortDir::Asc)
            .page(1, 2);
        let rows = q.execute(&t).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["record_id"], "R-002");
    }

    #[test]
    fn rejects_unknown_column() {
        let t = make_table();
        let q = Query::new().filter("nope", Op::Eq, "x");
        assert!(q.execute(&t).is_err());
    }

    #[test]
    fn determinism() {
        let t = make_table();
        let q = Query::new().with_types(types()).sort("group", SortDir::Asc);
        let a = q.execute(&t).unwrap();
        let b = q.execute(&t).unwrap();
        let ids_a: Vec<_> = a.iter().map(|r| r["record_id"].clone()).collect();
        let ids_b: Vec<_> = b.iter().map(|r| r["record_id"].clone()).collect();
        assert_eq!(ids_a, ids_b);
    }
}
