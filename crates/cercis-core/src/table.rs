//! 表：CSV 的读写（UTF-8、无 BOM、LF 换行、列名小写 ASCII 下划线）。
//!
//! 存储即 CSV——任何软件都能读；本模块只负责“读得对、写得规范”。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::format;
use crate::tx;
use crate::{CercisError, Result};

/// 一行记录：列名 → 单元格文本。
pub type Row = BTreeMap<String, String>;

/// 一张表（对应 `tables/<name>.csv`）。
#[derive(Debug, Clone)]
pub struct Table {
    path: PathBuf,
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

/// 写入前的校验报告（分级：错误阻止写入，警告放行但记录）。
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ValidationReport {
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

impl Table {
    /// 构造一张内存表（不绑定文件；用于测试与"先组装再落盘"的场景）。
    ///
    /// 注意：这种表调用 `save()` 会写入 `path` 指定的位置；若只做内存运算可忽略。
    pub fn in_memory(path: impl AsRef<Path>, columns: &[&str]) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            rows: Vec::new(),
        }
    }

    /// 从 CSV 文件载入。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let text = tx::read_optional(&path)?
            .ok_or_else(|| CercisError::NotFound(format!("表不存在: {}", path.display())))?;
        Self::from_csv_text(&path, &text)
    }

    /// 若文件不存在则创建空表（给定列）。
    pub fn load_or_create(path: impl AsRef<Path>, columns: &[&str]) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        match tx::read_optional(&path)? {
            Some(text) => Self::from_csv_text(&path, &text),
            None => {
                let t = Table {
                    path,
                    columns: columns.iter().map(|s| s.to_string()).collect(),
                    rows: Vec::new(),
                };
                t.validate_structure()?;
                tx::write_atomic_str(&t.path, &t.to_csv_text())?;
                Ok(t)
            }
        }
    }

    fn from_csv_text(path: &Path, text: &str) -> Result<Self> {
        let mut rdr = csv::ReaderBuilder::new()
            .has_headers(true)
            .flexible(false)
            .from_reader(text.as_bytes());

        let headers = rdr
            .headers()?
            .iter()
            .map(|s| s.trim().to_string())
            .collect::<Vec<_>>();

        let mut rows = Vec::new();
        for rec in rdr.records() {
            let rec = rec?;
            let mut row = Row::new();
            for (i, col) in headers.iter().enumerate() {
                row.insert(col.clone(), rec.get(i).unwrap_or("").to_string());
            }
            rows.push(row);
        }

        let t = Table {
            path: path.to_path_buf(),
            columns: headers,
            rows,
        };
        t.validate_structure()?;
        Ok(t)
    }

    /// 目录结构校验（内核级、不可关闭）。
    pub fn validate_structure(&self) -> Result<()> {
        if self.columns.is_empty() {
            return Err(CercisError::Corrupt(format!(
                "表 {} 没有列名（表头缺失）",
                self.path.display()
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for c in &self.columns {
            if !format::is_ascii_snake(c) {
                return Err(CercisError::Corrupt(format!(
                    "列名需为小写 ASCII 下划线形式（如 sample_id），实际: {c:?}（{}）",
                    self.path.display()
                )));
            }
            if !seen.insert(c.clone()) {
                return Err(CercisError::Corrupt(format!(
                    "列名重复: {c}（{}）",
                    self.path.display()
                )));
            }
        }
        for (i, row) in self.rows.iter().enumerate() {
            for (k, v) in row {
                if !self.columns.contains(k) {
                    return Err(CercisError::Corrupt(format!(
                        "第 {} 行出现表外列 {k}（{}）",
                        i + 2,
                        self.path.display()
                    )));
                }
                if v.contains('\n') || v.contains('\r') {
                    return Err(CercisError::Corrupt(format!(
                        "第 {} 行 {k} 含换行（{}）",
                        i + 2,
                        self.path.display()
                    )));
                }
            }
        }
        Ok(())
    }

    /// 内容级校验（缺失值写法、数字/日期规范等；返回报告而不中断）。
    pub fn validate_content(&self) -> ValidationReport {
        let mut rep = ValidationReport::default();
        for (i, row) in self.rows.iter().enumerate() {
            let lineno = i + 2;
            for (k, v) in row {
                if v.trim().is_empty() {
                    continue;
                }
                if format::is_missing_marker(v) {
                    rep.warnings.push(format!(
                        "第 {lineno} 行 {k}={v:?} 疑似把“缺失”写成了标记值；建议留空单元格"
                    ));
                }
                if !format::is_nfc(v) {
                    rep.warnings
                        .push(format!("第 {lineno} 行 {k} 未做 NFC 归一化"));
                }
                if k.ends_with("_date") && !format::is_iso_date(v) {
                    rep.warnings.push(format!(
                        "第 {lineno} 行 {k}={v:?} 不是 ISO 日期（YYYY-MM-DD）"
                    ));
                }
            }
        }
        rep
    }

    /// 追加一行（内存内）。
    pub fn push(&mut self, row: Row) {
        self.rows.push(row);
    }

    /// 按主键列查找（返回首个匹配的行号）。
    pub fn find_index(&self, key: &str, value: &str) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| r.get(key).map(|v| v.as_str()) == Some(value))
    }

    /// 序列化为 CSV 文本（LF 换行，末行带换行）。
    pub fn to_csv_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.columns.join(","));
        out.push('\n');
        for row in &self.rows {
            let cells = self
                .columns
                .iter()
                .map(|c| csv_escape(row.get(c).map(|s| s.as_str()).unwrap_or("")))
                .collect::<Vec<_>>();
            out.push_str(&cells.join(","));
            out.push('\n');
        }
        out
    }

    /// 原子落盘（写入前做结构校验）。
    pub fn save(&self) -> Result<()> {
        self.validate_structure()?;
        tx::write_atomic_str(&self.path, &self.to_csv_text())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join("cercis-table-test");
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("t.csv");
        let mut t = Table::load_or_create(&p, &["obs_id", "value_num"]).unwrap();
        let mut row = Row::new();
        row.insert("obs_id".into(), "01J8A001".into());
        row.insert("value_num".into(), "29.7".into());
        t.push(row);
        t.save().unwrap();

        let t2 = Table::load(&p).unwrap();
        assert_eq!(t2.rows.len(), 1);
        assert_eq!(t2.rows[0]["value_num"], "29.7");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_bad_column_name() {
        let dir = std::env::temp_dir().join("cercis-table-test2");
        let _ = std::fs::remove_dir_all(&dir);
        let p = dir.join("t.csv");
        let err = Table::load_or_create(&p, &["BadColumn"]).unwrap_err();
        assert!(format!("{err}").contains("列名"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
