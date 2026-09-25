//! 类型推断：**给建议，永不自动改模板**（需求文档 §5.9 / 决策 D36）。
//!
//! 用途：导入一批数据之后、或者项目跑了一段时间之后，想知道"这一列到底该是什么类型"。
//! 内核按 [`crate::value`] 的规则逐格试算，给出：
//! - **候选类型 + 置信度**（匹配格数 ÷ 非空格数）
//! - **反例**（前 3 个不匹配的值，让人一眼看出"是笔误还是类型选错"）
//! - **与模板的差异**（现在声明的是什么、要不要改）
//!
//! 纪律（写在文档里，也写在这里）：
//! 1. **≥ 95% 才给强建议**——低于这个数宁可让人保持 `text`，也不要猜；
//! 2. `time` **只校验、不推断**（`09:30` 与 `9:30` 都读得懂，猜它没意义）；
//! 3. 只出结论、不改数据、不改模板——要改模板得走
//!    [`crate::template::Template::with_field_patch`]。

use serde::{Deserialize, Serialize};

use crate::value::{self, ValueDomain};

/// 强建议门槛：匹配率必须 ≥ 95%。
pub const STRONG_THRESHOLD: f64 = 0.95;

/// 反例最多列几个（够看出"是笔误还是类型选错"）。
pub const MAX_COUNTEREXAMPLES: usize = 3;

/// 一列的统计。
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ColumnStats {
    /// 总行数
    pub total: usize,
    /// 非空单元格数
    pub non_empty: usize,
    /// 空单元格数（= 未记录，合法）
    pub empty: usize,
    /// 疑似缺失值占位符（NA / - 之类）的数量
    pub missing_markers: usize,
}

/// 一个候选类型。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub domain: String,
    /// 匹配的格子数
    pub matched: usize,
    /// 置信度 = matched / non_empty（0.0–1.0）
    pub confidence: f64,
    /// 不匹配的例子（最多 3 个）
    pub counterexamples: Vec<String>,
}

/// 一列的推断结论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub stats: ColumnStats,
    /// 候选（按置信度、具体度排序）
    pub candidates: Vec<Candidate>,
    /// 建议的类型：只有达到门槛才给；没达到就是 `null`（宁可不猜）
    pub recommended: Option<String>,
    /// 建议强度：strong（达到门槛）/ none（数据不足或太乱）/ empty（整列空）
    pub strength: String,
    /// 说明（给人和 AI 看的注意事项）
    pub notes: Vec<String>,
}

/// 一列 + 模板现状。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInference {
    pub table: String,
    pub column: String,
    /// 模板里现在声明的类型
    pub declared_type: String,
    /// 与声明类型不符的格子数
    pub declared_violations: usize,
    /// 与声明类型不符的例子（最多 3 个）
    pub declared_counterexamples: Vec<String>,
    /// 建议改类型？（false = 保持现状）
    pub suggest_change: bool,
    pub profile: Profile,
}

impl ColumnInference {
    /// 建议的类型（没有建议时为 None）。
    pub fn recommended(&self) -> Option<ValueDomain> {
        self.profile
            .recommended
            .as_deref()
            .and_then(ValueDomain::parse)
    }
}

/// 推断一列（输入：该列的全部单元格文本，按表内顺序）。
pub fn infer_values<'a, I>(values: I) -> Profile
where
    I: IntoIterator<Item = &'a str>,
{
    let mut stats = ColumnStats::default();
    let mut collected: Vec<String> = Vec::new();
    for v in values {
        stats.total += 1;
        if v.trim().is_empty() {
            stats.empty += 1;
        } else {
            stats.non_empty += 1;
            if crate::format::is_missing_marker(v) {
                stats.missing_markers += 1;
            }
            collected.push(v.to_string());
        }
    }

    let mut notes: Vec<String> = Vec::new();
    if stats.missing_markers > 0 {
        notes.push(format!(
            "有 {} 处写成占位符（{}），建议留空",
            stats.missing_markers,
            crate::format::MISSING_MARKERS.join(" / ")
        ));
    }

    if stats.non_empty == 0 {
        notes.push("整列没有非空数据，无法推断（先录数据，或按计划直接定类型）".into());
        return Profile {
            stats,
            candidates: Vec::new(),
            recommended: None,
            strength: "empty".into(),
            notes,
        };
    }

    let mut candidates: Vec<Candidate> = Vec::new();
    for domain in ValueDomain::ALL {
        if !domain.is_inferable() {
            continue;
        }
        let mut matched = 0usize;
        let mut counterexamples: Vec<String> = Vec::new();
        for v in &collected {
            if value::matches_domain(domain, v) {
                matched += 1;
            } else if counterexamples.len() < MAX_COUNTEREXAMPLES {
                counterexamples.push(shorten(v));
            }
        }
        candidates.push(Candidate {
            domain: domain.as_str().to_string(),
            matched,
            confidence: matched as f64 / stats.non_empty as f64,
            counterexamples,
        });
    }

    // 排序：置信度高的在前；同分时更具体的类型在前（integer 优先于 number，number 优先于 text）
    candidates.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| specificity(&b.domain).cmp(&specificity(&a.domain)))
            .then_with(|| a.domain.cmp(&b.domain))
    });

    // 建议：先筛掉 text（兜底类型）与没到门槛的；**再从过关的里挑最具体的**。
    //
    // 为什么按"最具体"而不是"匹配率最高"：99 个整数 + 1 个 `12.5` 时，
    // number 是 100%、integer 是 99%——但这一列该报 integer，然后把 `12.5`
    // 当反例摆出来给人看（很可能就是笔误）。反过来，若选 number，笔误就沉底了。
    let best = candidates
        .iter()
        .filter(|c| c.domain != ValueDomain::Text.as_str())
        .max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .cloned();

    let strong = candidates
        .iter()
        .filter(|c| {
            c.domain != ValueDomain::Text.as_str() && c.confidence >= STRONG_THRESHOLD
        })
        .max_by_key(|c| specificity(&c.domain))
        .map(|c| c.domain.clone());

    if let Some(name) = &strong {
        if let Some(c) = candidates.iter().find(|c| &c.domain == name) {
            if !c.counterexamples.is_empty() {
                notes.push(format!(
                    "建议 {}，但有 {} 格不符（{}）——很可能是笔误，改类型前先看一眼这几格",
                    name,
                    c.matched.abs_diff(stats.non_empty),
                    c.counterexamples.join("、")
                ));
            }
        }
    } else {
        match best {
            // 一个都不匹配：说明它本来就是文本列（编号、名称、备注……），不用瞎猜
            Some(c) if c.confidence <= 0.0 => notes.push(
                "内容不符合任何特定类型（不像数值、日期、时间、是/否）——保持 text 即可".into(),
            ),
            Some(c) => notes.push(format!(
                "最高匹配是 {}（{:.0}%），低于强建议门槛 {:.0}%——保持 text 更稳，或先看看反例是不是笔误",
                c.domain,
                c.confidence * 100.0,
                STRONG_THRESHOLD * 100.0
            )),
            None => notes.push("没有可用的候选类型——保持 text 即可".into()),
        }
    }

    // 顺带一提：整列都像时刻（time 不参与推断，只能靠人指定）
    if collected
        .iter()
        .all(|v| value::matches_domain(ValueDomain::Time, v))
    {
        notes.push(
            "整列看起来都是时刻（09:30 这种）；time 类型不参与推断，需要的话请手工指定".into(),
        );
    }

    let strength = if strong.is_some() { "strong" } else { "none" };
    Profile {
        stats,
        candidates,
        recommended: strong,
        strength: strength.into(),
        notes,
    }
}

/// 推断一张表的所有列（`declared` 是模板给的"列名 → 当前类型"）。
pub fn infer_table(
    table: &crate::table::Table,
    declared: &[(String, String)],
) -> Vec<ColumnInference> {
    let mut out = Vec::new();
    for col in &table.columns {
        let declared_type = declared
            .iter()
            .find(|(c, _)| c == col)
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| ValueDomain::Text.as_str().to_string());
        let cells: Vec<String> = table
            .rows
            .iter()
            .map(|r| r.get(col).cloned().unwrap_or_default())
            .collect();
        let profile = infer_values(cells.iter().map(|s| s.as_str()));

        // 与"现在声明的类型"对账
        let domain = ValueDomain::parse(&declared_type).unwrap_or(ValueDomain::Text);
        let mut violations = 0usize;
        let mut counterexamples: Vec<String> = Vec::new();
        for cell in cells.iter().filter(|c| !c.trim().is_empty()) {
            if !value::matches_domain(domain, cell) {
                violations += 1;
                if counterexamples.len() < MAX_COUNTEREXAMPLES {
                    counterexamples.push(shorten(cell));
                }
            }
        }

        let suggested = profile
            .recommended
            .as_deref()
            .and_then(ValueDomain::parse);
        let suggest_change = match suggested {
            None => false,
            Some(s) if s == domain => false,
            // 整数是数值的子集：声明 number、数据全是整数时不必改
            Some(ValueDomain::Integer) if domain == ValueDomain::Number => false,
            Some(_) => true,
        };

        out.push(ColumnInference {
            table: String::new(),
            column: col.clone(),
            declared_type,
            declared_violations: violations,
            declared_counterexamples: counterexamples,
            suggest_change,
            profile,
        });
    }
    out
}

fn specificity(domain: &str) -> u8 {
    ValueDomain::parse(domain).map(|d| d.specificity()).unwrap_or(0)
}

/// 太长或含换行的值压短，便于打印。
fn shorten(s: &str) -> String {
    let t = s.trim().replace('\n', " ");
    let truncated: String = t.chars().take(32).collect();
    if truncated.chars().count() < t.chars().count() {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(values: &[&str]) -> Profile {
        infer_values(values.iter().copied())
    }

    #[test]
    fn integers_are_strongly_suggested() {
        let p = profile(&["1", "2", "3", "48", "0"]);
        assert_eq!(p.recommended.as_deref(), Some("integer"));
        assert_eq!(p.strength, "strong");
        assert_eq!(p.stats.non_empty, 5);
    }

    #[test]
    fn one_bad_cell_in_a_hundred_is_still_strong_with_counterexample() {
        let mut v: Vec<String> = (0..99).map(|i| format!("{i}")).collect();
        v.push("12.5".into()); // 1/100 不符 → 99% ≥ 95%
        let refs: Vec<&str> = v.iter().map(|s| s.as_str()).collect();
        let p = infer_values(refs);
        assert_eq!(p.recommended.as_deref(), Some("integer"));
        let integer = p.candidates.iter().find(|c| c.domain == "integer").unwrap();
        assert_eq!(integer.counterexamples, vec!["12.5".to_string()]);
    }

    #[test]
    fn messy_column_gets_no_recommendation() {
        let p = profile(&["12", "abc", "2026-06-15"]);
        assert!(p.recommended.is_none(), "{:?}", p.recommended);
        assert_eq!(p.strength, "none");
        assert!(p.notes.iter().any(|n| n.contains("门槛")));
    }

    #[test]
    fn empty_column_has_no_candidates() {
        let p = profile(&["", "  "]);
        assert!(p.candidates.is_empty());
        assert_eq!(p.strength, "empty");
        assert!(p.recommended.is_none());
    }

    #[test]
    fn time_is_never_recommended_but_noted() {
        let p = profile(&["09:30", "10:00", "23:59:59"]);
        assert!(p.recommended.is_none());
        assert!(p.notes.iter().any(|n| n.contains("time")));
    }

    #[test]
    fn dates_bools_and_numbers() {
        assert_eq!(
            profile(&["2026-06-15", "2026-06-16"]).recommended.as_deref(),
            Some("date")
        );
        assert_eq!(
            profile(&["是", "否", "是"]).recommended.as_deref(),
            Some("bool")
        );
        assert_eq!(
            profile(&["1.5", "2.25"]).recommended.as_deref(),
            Some("number")
        );
    }

    #[test]
    fn missing_markers_are_noted() {
        let p = profile(&["1", "2", "NA"]);
        assert_eq!(p.stats.non_empty, 3);
        assert_eq!(p.stats.missing_markers, 1);
        assert!(p.notes.iter().any(|n| n.contains("占位符")));
    }

    #[test]
    fn declared_type_mismatch_is_reported() {
        use crate::table::{Row, Table};
        let mut table = Table::in_memory("/tmp/cercis-infer.csv", &["plot_id", "height"]);
        let mut good: Row = Row::new();
        good.insert("plot_id".into(), "P1".into());
        good.insert("height".into(), "12.5".into());
        let mut bad: Row = Row::new();
        bad.insert("plot_id".into(), "P2".into());
        bad.insert("height".into(), "abc".into());
        table.push(good);
        table.push(bad);
        let declared = vec![
            ("plot_id".to_string(), "text".to_string()),
            ("height".to_string(), "number".to_string()),
        ];
        let out = infer_table(&table, &declared);
        let height = out.iter().find(|c| c.column == "height").unwrap();
        assert_eq!(height.declared_type, "number");
        assert_eq!(height.declared_violations, 1);
        assert_eq!(height.declared_counterexamples, vec!["abc".to_string()]);
        // 50% 匹配，达不到门槛 → 不建议改类型
        assert!(!height.suggest_change);
        let plot = out.iter().find(|c| c.column == "plot_id").unwrap();
        assert_eq!(plot.declared_type, "text");
        assert!(!plot.suggest_change);
    }
}
