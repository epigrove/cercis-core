//! 编号与时间：机器编号用 ULID（唯一、可按时间排序），人类编号用可读前缀。

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use ulid::Ulid;

use crate::{CercisError, Result};

/// 生成机器编号（ULID）。用于 `obs_id`、`change_id` 等需要全局唯一的字段。
pub fn new_machine_id() -> String {
    Ulid::new().to_string()
}

/// 生成人类可读编号，例如 `SB-20260101-G1-01-A1`。
///
/// 规则：前缀 + 日期（YYYYMMDD） + 各片段；片段中的非字母数字字符会被替换为 `_`。
pub fn human_id(prefix: &str, date: NaiveDate, parts: &[&str]) -> String {
    let mut s = String::with_capacity(32);
    s.push_str(prefix);
    s.push('-');
    s.push_str(&date.format("%Y%m%d").to_string());
    for p in parts {
        if p.is_empty() {
            continue;
        }
        s.push('-');
        s.push_str(&sanitize_part(p));
    }
    s
}

fn sanitize_part(p: &str) -> String {
    p.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// 是否为合法 ULID。
pub fn is_machine_id(s: &str) -> bool {
    Ulid::from_string(s).is_ok()
}

/// 当前时间（ISO 8601，UTC，秒精度，带 `Z`）。
pub fn now_iso8601() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// 解析 ISO 8601 时间（要求带时区）。
pub fn parse_iso8601(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| CercisError::Invalid(format!("时间格式不是 ISO 8601（需带时区）: {s}（{e}）")))
}

/// 目录/文件用的时间戳（不含冒号，兼容 Windows 与 NAS）。
pub fn timestamp_for_path(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H%M%SZ").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_id_is_readable() {
        let d = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        assert_eq!(
            human_id("SB", d, &["G1", "01", "A1"]),
            "SB-20260101-G1-01-A1"
        );
    }

    #[test]
    fn machine_id_roundtrip() {
        let id = new_machine_id();
        assert!(is_machine_id(&id));
    }

    #[test]
    fn timestamp_has_no_colon() {
        let ts = timestamp_for_path(Utc::now());
        assert!(!ts.contains(':'));
    }
}
