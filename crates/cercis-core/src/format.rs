//! 格式规则：日期、数字、文件名、缺失值、Unicode 归一化。
//!
//! 这些规则是“谁来读都能读对”的保证，属于内核的**结构完整性**检查（不可关闭）。

use chrono::NaiveDate;
use unicode_normalization::UnicodeNormalization;

use crate::{CercisError, Result};

/// 跨平台禁止出现在文件名中的字符。
pub const FORBIDDEN_FILENAME_CHARS: &[char] = &[':', '*', '?', '"', '<', '>', '|', '\\', '/'];

/// 不允许用来表示“缺失”的写法——缺失必须是**空单元格**。
pub const MISSING_MARKERS: &[&str] = &[
    "NA", "N/A", "na", "null", "NULL", "NaN", "-", "--", "?", ".",
];

/// 是否为 ISO 8601 日期（YYYY-MM-DD）。
pub fn is_iso_date(s: &str) -> bool {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// 解析日期，失败时给出可读错误。
pub fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| CercisError::Invalid(format!("日期需为 YYYY-MM-DD 形式: {s}（{e}）")))
}

/// 文本是否已是 NFC 形式（macOS 默认产出 NFD，会导致“同名不同码”）。
pub fn is_nfc(s: &str) -> bool {
    let normalized: String = s.nfc().collect();
    normalized == s
}

/// 转为 NFC。
pub fn to_nfc(s: &str) -> String {
    s.nfc().collect()
}

/// 是否含组合记号（NFD 残留的典型特征）。
pub fn has_combining_marks(s: &str) -> bool {
    s.chars().any(|c| ('\u{0300}'..='\u{036F}').contains(&c))
}

/// 文件名是否跨平台安全（长度、字符、NFC）。
pub fn check_filename(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(CercisError::Invalid("文件名不能为空".into()));
    }
    if name.chars().count() > 200 {
        return Err(CercisError::Invalid(format!(
            "文件名过长（>200 字符）: {name}"
        )));
    }
    if let Some(bad) = name.chars().find(|c| FORBIDDEN_FILENAME_CHARS.contains(c)) {
        return Err(CercisError::Invalid(format!(
            "文件名含跨平台禁用字符 {bad:?}: {name}"
        )));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(CercisError::Invalid(format!("文件名含控制字符: {name}")));
    }
    if !is_nfc(name) {
        return Err(CercisError::Invalid(format!(
            "文件名未做 Unicode NFC 归一化（macOS 上常见）: {name}"
        )));
    }
    Ok(())
}

/// 数字写法是否规范：小数点为 `.`，不含千分位逗号。
pub fn is_canonical_number(s: &str) -> bool {
    let t = s.trim();
    if t.is_empty() {
        return false;
    }
    if t.contains(',') || t.contains('，') {
        return false;
    }
    t.parse::<f64>().is_ok()
}

/// 该字符串是否被误当作“缺失值”写入了。
pub fn is_missing_marker(s: &str) -> bool {
    MISSING_MARKERS.contains(&s.trim())
}

/// 单元格文本的通用检查（换行与缺失值写法）。
pub fn check_cell_text(s: &str) -> Result<()> {
    if s.contains('\n') || s.contains('\r') {
        return Err(CercisError::Invalid(
            "单元格不允许包含换行；需要长文本请放在 notes 字段或单独文档".into(),
        ));
    }
    if is_missing_marker(s) {
        return Err(CercisError::Invalid(format!(
            "缺失值应留空单元格，不要写 {s:?}（这会让“未记录”和“值为 NA”无法区分）"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert!(is_iso_date("2026-09-05"));
        assert!(!is_iso_date("2026/09/05"));
        assert!(parse_date("2026-09-05").is_ok());
    }

    #[test]
    fn numbers() {
        assert!(is_canonical_number("29.7"));
        assert!(is_canonical_number("-1.5"));
        assert!(!is_canonical_number("1,234.5"));
        assert!(!is_canonical_number(""));
    }

    #[test]
    fn missing_markers_rejected() {
        assert!(check_cell_text("29.7").is_ok());
        assert!(check_cell_text("NA").is_err());
        assert!(check_cell_text("换行\n内容").is_err());
    }

    #[test]
    fn filenames() {
        assert!(check_filename("T1.heic").is_ok());
        assert!(check_filename("T1:new.heic").is_err());
    }
}
