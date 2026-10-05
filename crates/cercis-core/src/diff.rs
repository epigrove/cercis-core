//! 差异与对比（FR-M05：“版本浏览与对比”）——像"修订模式"那样把改了什么摊开。
//!
//! 两种比法：
//! 1. **一条记录**：`record_diff` —— 拿这条记录**最早一份**和**最新一份**快照，
//!    逐字段比出"哪一格从什么变成了什么"；再把更正记录（旧值/新值/理由/谁）并进来。
//! 2. **整张表（两份快照）**：`snapshot_diff` —— 两份历史对象逐行比，标出新增/删除的行。
//!
//! 实现上有意保持"笨而可靠"：行级对比用经典 LCS（最长公共子序列），不做启发式；
//! 表行数超过 `MAX_DIFF_LINES` 时退化成"只报计数 + 前若干处"（避免界面被十万行卡死）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{CercisError, Result};

/// 报告格式版本。
pub const DIFF_FORMAT: &str = "cercis-diff/1";
/// 超过这么多行就退化成"只报计数"（防界面卡死）。
pub const MAX_DIFF_LINES: usize = 5000;

/// 一处字段级改动。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldChange {
    pub field: String,
    /// `changed` / `added` / `removed`
    pub kind: String,
    #[serde(default)]
    pub old: String,
    #[serde(default)]
    pub new: String,
}

/// 一条记录里的更正（谁、何时、为什么）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrectionNote {
    pub ts: String,
    pub field: String,
    pub old: String,
    pub new: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub user: String,
}

/// 记录级对比结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordDiff {
    pub format: String,
    pub table: String,
    pub id: String,
    #[serde(default)]
    pub key_field: String,
    /// 最早那份快照的指纹（没有历史就是空）
    #[serde(default)]
    pub from_hash: String,
    /// 最新那份快照的指纹
    #[serde(default)]
    pub to_hash: String,
    pub changes: Vec<FieldChange>,
    #[serde(default)]
    pub corrections: Vec<CorrectionNote>,
    pub identical: bool,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl RecordDiff {
    pub fn summary(&self) -> String {
        if self.identical {
            return format!("{} / {}：没有变化（{} → {}）", self.table, self.id, short(&self.from_hash), short(&self.to_hash));
        }
        format!(
            "{} / {}：{} 处字段变化（{} → {}）",
            self.table,
            self.id,
            self.changes.len(),
            short(&self.from_hash),
            short(&self.to_hash)
        )
    }
}

/// 行级对比里的一行。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LineChange {
    /// `+` 新增 / `-` 删除 / ` ` 未变
    pub op: String,
    /// 在**旧**文件里的行号（新增行为空）
    #[serde(default)]
    pub old_line: Option<usize>,
    /// 在**新**文件里的行号（删除行为空）
    #[serde(default)]
    pub new_line: Option<usize>,
    pub text: String,
}

/// 两份快照的对比结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotDiff {
    pub format: String,
    pub from_hash: String,
    pub to_hash: String,
    pub added: usize,
    pub removed: usize,
    pub unchanged: usize,
    pub lines: Vec<LineChange>,
    /// 行数太多时只给计数（`lines` 为空，`truncated=true`）
    #[serde(default)]
    pub truncated: bool,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl SnapshotDiff {
    pub fn summary(&self) -> String {
        format!(
            "{} → {}：+{} 行 / -{} 行（未变 {} 行）{}",
            short(&self.from_hash),
            short(&self.to_hash),
            self.added,
            self.removed,
            self.unchanged,
            if self.truncated { "｜行数过多，只报计数" } else { "" }
        )
    }
}

/// 一条记录改了什么（最早 vs 最新）。
pub fn record_diff(root: &Path, table: &str, id: &str) -> Result<RecordDiff> {
    let store = crate::Store::new(root.to_path_buf(), crate::store::Identity::default());
    let entries = store.read_change_log()?;
    // 两种流水都要看：**指名这条记录**的（更正、导入），以及**动过这张表**的
    // （`save_table` 这类整表写入没带记录编号，但它确实改了这条记录）
    let mine: Vec<_> = entries
        .iter()
        .filter(|e| e.target_table == table)
        .collect();
    if mine.is_empty() {
        return Err(CercisError::NotFound(format!(
            "流水里没有 {table} / {id} 这条记录（编号对不对？）"
        )));
    }

    let key_field = primary_key_for(root, table, id);
    // 按时间顺序把这张表出现过的快照指纹排一排（去重、跳过空）
    let mut hashes: Vec<String> = Vec::new();
    for e in &mine {
        for h in [&e.hash_before, &e.hash_after] {
            if !h.is_empty() && !hashes.contains(h) {
                hashes.push(h.clone());
            }
        }
    }

    // 找到这条记录**最早出现**和**最后出现**的那两份快照
    let mut first_seen: Option<(String, crate::table::Row)> = None;
    let mut last_seen: Option<(String, crate::table::Row)> = None;
    let mut first_index = 0usize;
    for (i, h) in hashes.iter().enumerate() {
        if let Some(row) = load_row(&store, root, table, h, id, &key_field)? {
            if first_seen.is_none() {
                first_seen = Some((h.clone(), row.clone()));
                first_index = i;
            }
            last_seen = Some((h.clone(), row));
        }
    }
    let (from_hash, old_row) = match first_seen {
        Some((h, r)) => (h, Some(r)),
        None => (String::new(), None),
    };
    let (to_hash, new_row) = match last_seen {
        Some((h, r)) => (h, Some(r)),
        None => (String::new(), None),
    };

    // 更正记录（人写的"为什么"）
    let mut corrections = Vec::new();
    if let Ok(t) = store.load_table("corrections", &[]) {
        for r in &t.rows {
            if r.get("target_table").map(|s| s.as_str()) != Some(table)
                || r.get("target_id").map(|s| s.as_str()) != Some(id)
            {
                continue;
            }
            corrections.push(CorrectionNote {
                ts: r.get("ts").cloned().unwrap_or_default(),
                field: r.get("field").cloned().unwrap_or_default(),
                old: r.get("old_value").cloned().unwrap_or_default(),
                new: r.get("new_value").cloned().unwrap_or_default(),
                reason: r.get("reason").cloned().unwrap_or_default(),
                user: r.get("user_id").cloned().unwrap_or_default(),
            });
        }
    }

    // **生效值**：快照里的值 + 之后的更正（更正就是"改了，只是表里那一行还没跟着动"）
    let effective = |field: &str, snapshot: &str| -> (String, bool) {
        let mut value = snapshot.to_string();
        let mut applied = false;
        for c in &corrections {
            if c.field == field {
                value = c.new.clone();
                applied = true;
            }
        }
        (value, applied)
    };

    let mut fields: Vec<String> = Vec::new();
    for row in [&old_row, &new_row] {
        if let Some(r) = row {
            for k in r.keys() {
                if !fields.contains(k) {
                    fields.push(k.clone());
                }
            }
        }
    }
    for c in &corrections {
        if !fields.contains(&c.field) {
            fields.push(c.field.clone());
        }
    }
    let mut changes = Vec::new();
    let mut any_correction_applied = false;
    for f in &fields {
        let o = old_row
            .as_ref()
            .and_then(|r| r.get(f))
            .cloned()
            .unwrap_or_else(|| {
                // 老快照里没有这行：拿最早那条更正的"旧值"当老值（比"空白"更如实）
                corrections
                    .iter()
                    .find(|c| &c.field == f)
                    .map(|c| c.old.clone())
                    .unwrap_or_default()
            });
        let snapshot_new = new_row
            .as_ref()
            .and_then(|r| r.get(f))
            .cloned()
            .unwrap_or_default();
        let (n, applied) = effective(f, &snapshot_new);
        any_correction_applied |= applied && n != snapshot_new;
        if o == n {
            continue;
        }
        let kind = if o.is_empty() {
            "added"
        } else if n.is_empty() {
            "removed"
        } else {
            "changed"
        };
        changes.push(FieldChange {
            field: f.clone(),
            kind: kind.to_string(),
            old: o,
            new: n,
        });
    }

    let mut notes = Vec::new();
    if from_hash.is_empty() || first_index > 0 {
        notes.push(
            "这条记录是后来才加的：老那一份快照里没有它（所以按最早那条更正的旧值当老值）"
                .to_string(),
        );
    }
    if any_correction_applied {
        notes.push(
            "下面的「新值」是**生效值**：快照里的值 + 已记录的更正（表里那一行可能还没跟着动）"
                .to_string(),
        );
    }
    Ok(RecordDiff {
        format: DIFF_FORMAT.to_string(),
        table: table.to_string(),
        id: id.to_string(),
        key_field,
        from_hash: from_hash.clone(),
        to_hash: to_hash.clone(),
        identical: changes.is_empty(),
        changes,
        corrections,
        notes,
    })
}

/// 两份快照逐行比（整张表 / 任意历史对象）。
pub fn snapshot_diff(root: &Path, from_hash: &str, to_hash: &str) -> Result<SnapshotDiff> {
    let store = crate::Store::new(root.to_path_buf(), crate::store::Identity::default());
    let a = store.read_object(from_hash)?;
    let b = store.read_object(to_hash)?;
    let a_text = String::from_utf8(a)
        .map_err(|e| CercisError::Corrupt(format!("老那份快照不是 UTF-8：{e}")))?;
    let b_text = String::from_utf8(b)
        .map_err(|e| CercisError::Corrupt(format!("新那份快照不是 UTF-8：{e}")))?;
    let a_lines: Vec<&str> = a_text.lines().collect();
    let b_lines: Vec<&str> = b_text.lines().collect();

    let mut out = SnapshotDiff {
        format: DIFF_FORMAT.to_string(),
        from_hash: from_hash.to_string(),
        to_hash: to_hash.to_string(),
        added: 0,
        removed: 0,
        unchanged: 0,
        lines: Vec::new(),
        truncated: false,
        notes: Vec::new(),
    };

    if a_lines.len() > MAX_DIFF_LINES || b_lines.len() > MAX_DIFF_LINES {
        // 太大：只报计数（用集合差算个大概，够用且不卡）
        use std::collections::BTreeSet;
        let sa: BTreeSet<&str> = a_lines.iter().copied().collect();
        let sb: BTreeSet<&str> = b_lines.iter().copied().collect();
        out.added = sb.difference(&sa).count();
        out.removed = sa.difference(&sb).count();
        out.unchanged = sa.intersection(&sb).count();
        out.truncated = true;
        out.notes.push(format!(
            "行数超过 {MAX_DIFF_LINES}，只报计数（按行的集合差算；要逐行看得挑小一点的快照）"
        ));
        return Ok(out);
    }

    // LCS 动态规划（行数受上限保护）
    let n = a_lines.len();
    let m = b_lines.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a_lines[i] == b_lines[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0usize, 0usize);
    let mut pending_removed: Vec<LineChange> = Vec::new();
    while i < n && j < m {
        if a_lines[i] == b_lines[j] {
            for r in pending_removed.drain(..) {
                out.lines.push(r);
            }
            out.unchanged += 1;
            out.lines.push(LineChange {
                op: " ".into(),
                old_line: Some(i + 1),
                new_line: Some(j + 1),
                text: a_lines[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.removed += 1;
            pending_removed.push(LineChange {
                op: "-".into(),
                old_line: Some(i + 1),
                new_line: None,
                text: a_lines[i].to_string(),
            });
            i += 1;
        } else {
            out.added += 1;
            out.lines.push(LineChange {
                op: "+".into(),
                old_line: None,
                new_line: Some(j + 1),
                text: b_lines[j].to_string(),
            });
            j += 1;
        }
    }
    for r in pending_removed.drain(..) {
        out.lines.push(r);
    }
    while i < n {
        out.removed += 1;
        out.lines.push(LineChange {
            op: "-".into(),
            old_line: Some(i + 1),
            new_line: None,
            text: a_lines[i].to_string(),
        });
        i += 1;
    }
    while j < m {
        out.added += 1;
        out.lines.push(LineChange {
            op: "+".into(),
            old_line: None,
            new_line: Some(j + 1),
            text: b_lines[j].to_string(),
        });
        j += 1;
    }
    Ok(out)
}

/// 人读的差异文本（`-` 旧 / `+` 新）。
pub fn render_record(d: &RecordDiff) -> String {
    let mut s = String::new();
    s.push_str(&format!("{} / {}\n", d.table, d.id));
    if d.identical {
        s.push_str("没有变化。\n");
    } else {
        for c in &d.changes {
            s.push_str(&format!("  {}：{} → {}\n", c.field, show(&c.old), show(&c.new)));
        }
    }
    if !d.corrections.is_empty() {
        s.push_str("\n更正记录：\n");
        for c in &d.corrections {
            s.push_str(&format!(
                "  {} {}：{} → {}（{}｜{}）\n",
                c.ts, c.field, c.old, c.new, c.reason, c.user
            ));
        }
    }
    for n in &d.notes {
        s.push_str(&format!("注：{n}\n"));
    }
    s
}

/// 人读的整表差异（`-` 删除 / `+` 新增，未变的行不显示）。
pub fn render_snapshot(d: &SnapshotDiff) -> String {
    let mut s = String::new();
    s.push_str(&format!("{}\n", d.summary()));
    if d.truncated {
        return s;
    }
    for l in d.lines.iter().filter(|l| l.op != " ") {
        s.push_str(&format!("{} {}\n", l.op, l.text));
    }
    s
}

fn show(v: &str) -> String {
    if v.is_empty() {
        "（空）".to_string()
    } else {
        v.to_string()
    }
}

fn short(hash: &str) -> String {
    if hash.is_empty() {
        "（无）".to_string()
    } else {
        hash.chars().take(8).collect()
    }
}

/// 从一份快照里取出某条记录（快照指纹为空 / 对象不存在 → None）。
fn load_row(
    store: &crate::Store,
    root: &Path,
    table: &str,
    hash: &str,
    id: &str,
    key_field: &str,
) -> Result<Option<crate::table::Row>> {
    if hash.is_empty() {
        return Ok(None);
    }
    let bytes = match store.read_object(hash) {
        Ok(b) => b,
        Err(_) => return Ok(None), // 老对象不在了（比如被清理过）：当作"没有老版本"
    };
    let text = String::from_utf8(bytes)
        .map_err(|e| CercisError::Corrupt(format!("快照不是 UTF-8：{e}")))?;
    let path = root.join(format!("tables/{table}.csv"));
    let t = crate::table::Table::from_csv_text(&path, &text)?;
    Ok(t.rows
        .iter()
        .find(|r| r.get(key_field).map(|s| s.as_str()) == Some(id))
        .cloned())
}

/// 主键列名：模板里有就用模板的；没有就用已知系统表约定；再没有就试 `id`。
fn primary_key_for(root: &Path, table: &str, id: &str) -> String {
    let from_template = crate::tx::read_optional(&root.join("template.yaml"))
        .ok()
        .flatten()
        .and_then(|t| crate::Template::from_yaml(&t).ok())
        .and_then(|t| t.table(table).map(|td| td.primary_key.clone()))
        .filter(|s| !s.is_empty());
    if let Some(k) = from_template {
        return k;
    }
    match table {
        "change_log" => "change_id".to_string(),
        "corrections" => "correction_id".to_string(),
        "tasks" => "task_id".to_string(),
        _ => {
            // 兜底：看看哪一列真的存着这个编号
            if id.is_empty() {
                "id".to_string()
            } else {
                "record_id".to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Identity;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("cercis-diff-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn record_diff_shows_field_level_changes() {
        let dir = tmp("record");
        let p = crate::Project::create(&dir, "对比试验", Identity::default()).unwrap();
        let mut row = crate::table::Row::new();
        row.insert("record_id".into(), "R-1".into());
        row.insert("value_num".into(), "12.5".into());
        row.insert("notes".into(), "起初".into());
        p.store
            .append_row_extending("records", &[], row, "R-1", "写一条")
            .unwrap();
        // 更正一个字段（走内核通道：写更正记录 + 改当前行）
        p.store
            .correct("records", "R-1", "value_num", "12.5", "12.7", "笔误", "")
            .unwrap();
        let mut t = p.store.load_table("records", &[]).unwrap();
        for r in t.rows.iter_mut() {
            if r.get("record_id").map(|s| s.as_str()) == Some("R-1") {
                r.insert("value_num".into(), "12.7".into());
            }
        }
        p.store.save_table(&t, "update", "改 value_num").unwrap();

        let d = record_diff(&dir, "records", "R-1").unwrap();
        assert!(!d.identical);
        let changed = d
            .changes
            .iter()
            .find(|c| c.field == "value_num")
            .expect("value_num 应该有变化");
        assert_eq!(changed.old, "12.5");
        assert_eq!(changed.new, "12.7");
        assert!(d.corrections.iter().any(|c| c.reason == "笔误"));
        let text = render_record(&d);
        assert!(text.contains("12.5 → 12.7"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_diff_marks_added_and_removed_lines() {
        let dir = tmp("snapshot");
        let p = crate::Project::create(&dir, "快照对比", Identity::default()).unwrap();
        let mut row = crate::table::Row::new();
        row.insert("record_id".into(), "R-1".into());
        row.insert("value_num".into(), "1".into());
        p.store
            .append_row_extending("records", &[], row, "R-1", "第一条")
            .unwrap();
        let entries = p.store.read_change_log().unwrap();
        let h1 = entries
            .iter()
            .rev()
            .find(|e| !e.hash_after.is_empty())
            .unwrap()
            .hash_after
            .clone();

        let mut row = crate::table::Row::new();
        row.insert("record_id".into(), "R-2".into());
        row.insert("value_num".into(), "2".into());
        p.store
            .append_row_extending("records", &[], row, "R-2", "第二条")
            .unwrap();
        let entries = p.store.read_change_log().unwrap();
        let h2 = entries
            .iter()
            .rev()
            .find(|e| !e.hash_after.is_empty())
            .unwrap()
            .hash_after
            .clone();

        let d = snapshot_diff(&dir, &h1, &h2).unwrap();
        assert_eq!(d.added, 1, "应该多了一行 R-2：{d:?}");
        assert_eq!(d.removed, 0);
        assert!(d.lines.iter().any(|l| l.op == "+" && l.text.contains("R-2")));
        let text = render_snapshot(&d);
        assert!(text.contains("+ ") && text.contains("R-2"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
