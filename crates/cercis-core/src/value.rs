//! 值类型与约束：**内核是类型规则的唯一权威**（需求文档 §5.9 / 决策 D36）。
//!
//! 为什么要有这个模块：在写它之前，类型判断散在三处——内核 `validate.rs` 的分支、
//! 插件 `checks.rs` 的 `type_ok`、导出插件顺手写的判断。结果是"同一个值，
//! 两个工具两种判断"（例如 `1e3` 内核当初算合法数字，插件也放行）。类型是
//! **数据契约的一部分**，必须只有一份实现。
//!
//! 本模块定义：
//! - 七种**值域**（值类型）：`text` / `integer` / `number` / `bool` / `date` / `datetime` / `time`
//! - 六种**正交约束**：`required` / `range` / `allowed_values` / `pattern` / `precision` / `reference`
//!   （外加仅供展示与导出的 `unit`）
//! - 一套**校验函数**（[`check_value`]）与一份**机器可读清单**（[`contract`]，
//!   由 `cercis value-types --json` 输出）
//!
//! 三类使用者共用它：
//! 1. 内核自己：项目体检（[`crate::validate`]）、写入前检查、导出前清单；
//! 2. 插件：读 `cercis value-types --json` 的 `matcher` 规则照做，**不要另写一套**；
//! 3. AI：读同一份清单，知道"什么样的值是合法的"。
//!
//! ## 强度（对应 §5.9.2 / §7.4 FR-D01）
//! - **结构错误**（[`Severity::Error`]）阻止写入：类型不符、必填为空、不在允许值内、不符合模式、引用不存在；
//! - **提醒**（[`Severity::Warning`]）只报告不阻断：建议范围、小数位数、布尔/时间写法、缺失值占位符。
//! - **导出永不因校验阻断**（数据永远可读可导，这是"文件夹即数据库"的底线）。

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::format;
use crate::ids;

/// 校验强度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// 结构错误：阻止写入。
    Error,
    /// 提醒：放行，但记进报告。
    Warning,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 七个值域（值类型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueDomain {
    Text,
    Integer,
    Number,
    Bool,
    Date,
    Datetime,
    Time,
}

impl ValueDomain {
    /// 全部值域（顺序即文档 §5.9.1 的表顺序）。
    pub const ALL: [ValueDomain; 7] = [
        ValueDomain::Text,
        ValueDomain::Integer,
        ValueDomain::Number,
        ValueDomain::Bool,
        ValueDomain::Date,
        ValueDomain::Datetime,
        ValueDomain::Time,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ValueDomain::Text => "text",
            ValueDomain::Integer => "integer",
            ValueDomain::Number => "number",
            ValueDomain::Bool => "bool",
            ValueDomain::Date => "date",
            ValueDomain::Datetime => "datetime",
            ValueDomain::Time => "time",
        }
    }

    /// 解析类型名（大小写不敏感、允许首尾空白）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "text" => Some(ValueDomain::Text),
            "integer" => Some(ValueDomain::Integer),
            "number" => Some(ValueDomain::Number),
            "bool" => Some(ValueDomain::Bool),
            "date" => Some(ValueDomain::Date),
            "datetime" => Some(ValueDomain::Datetime),
            "time" => Some(ValueDomain::Time),
            _ => None,
        }
    }

    /// 是否可以由数据推断（`time` 只校验、不推断——见决策 D36）。
    pub fn is_inferable(self) -> bool {
        !matches!(self, ValueDomain::Time)
    }

    /// 推断时的具体度排序：越具体越优先（整数比数值具体，数值比文本具体）。
    pub fn specificity(self) -> u8 {
        match self {
            ValueDomain::Integer => 6,
            ValueDomain::Number => 5,
            ValueDomain::Bool => 4,
            ValueDomain::Date => 3,
            ValueDomain::Datetime => 2,
            ValueDomain::Time => 1,
            ValueDomain::Text => 0,
        }
    }

    /// 该值域的机器可读说明。
    pub fn spec(self) -> DomainSpec {
        let (label_cn, label_en, csv_form, matcher, severity, ok, bad, notes) = match self {
            ValueDomain::Text => (
                "文本",
                "Text",
                "原样文本（默认类型）",
                Matcher::Any,
                Severity::Error,
                &["合成甲", "N1P0D1V2", "小区-01"][..],
                &[][..],
                "不检查内容；需要格式约束时用 pattern（正则）。",
            ),
            ValueDomain::Integer => (
                "整数",
                "Integer",
                "十进制整数：无千分位、无小数点",
                Matcher::Regex {
                    pattern: r"^-?[0-9]+$".into(),
                },
                Severity::Error,
                &["12", "-3", "0"][..],
                &["12.5", "1,200", "1e3", "十二"][..],
                "计数、株数、重复次数等必须是整数的量。",
            ),
            ValueDomain::Number => (
                "数值",
                "Number",
                "规范数字写法：可选负号 + 数字，小数点用 `.`，不用科学计数法",
                Matcher::Regex {
                    pattern: r"^-?[0-9]+(\.[0-9]+)?$".into(),
                },
                Severity::Error,
                &["12", "12.5", "0.05", "-1.5"][..],
                &["1e3", "1,200.5", ".5", "十二", "NaN"][..],
                "读数、重量、浓度等。小数位数用 precision 约束（只提醒）。",
            ),
            ValueDomain::Bool => (
                "是/否",
                "Boolean",
                "`是` / `否`（也接受 `true` / `false`）",
                Matcher::Enum {
                    values: vec!["是".into(), "否".into(), "true".into(), "false".into()],
                    ascii_case_insensitive: true,
                },
                Severity::Warning,
                &["是", "否", "true", "false"][..],
                &["Y", "N", "1", "0"][..],
                "建议统一写 是/否；写 Y/1 只提醒、不阻断。",
            ),
            ValueDomain::Date => (
                "日期",
                "Date",
                "ISO 8601：`YYYY-MM-DD`",
                Matcher::Builtin {
                    checker: "iso_date".into(),
                },
                Severity::Error,
                &["2026-06-15", "2026-01-01"][..],
                &["2026/6/15", "15-06-2026", "2026-13-01"][..],
                "必须是真实存在的日历日（2026-02-30 不合法）。",
            ),
            ValueDomain::Datetime => (
                "日期时间",
                "Datetime",
                "ISO 8601 且带时区：`2026-06-15T09:30+08:00` 或 `...Z`",
                Matcher::Builtin {
                    checker: "iso8601_tz".into(),
                },
                Severity::Warning,
                &["2026-06-15T09:30+08:00", "2026-06-15T01:30:00Z"][..],
                &["2026-06-15 09:30", "2026-06-15T09:30"][..],
                "跨时区设备协作时必须带时区；写法不完整只提醒。",
            ),
            ValueDomain::Time => (
                "时刻",
                "Time",
                "`HH:MM` 或 `HH:MM:SS`（24 小时制，补零）",
                Matcher::Regex {
                    pattern: r"^([01][0-9]|2[0-3]):[0-5][0-9](:[0-5][0-9])?$".into(),
                },
                Severity::Warning,
                &["09:30", "23:59:59", "00:00"][..],
                &["9:30", "24:00", "9点30"][..],
                "只做校验、不参与类型推断（09:30 与 9:30 都能读懂，推断没有意义）。",
            ),
        };
        DomainSpec {
            name: self.as_str(),
            label_cn,
            label_en,
            csv_form,
            matcher,
            severity_on_violation: severity.as_str(),
            inferable: self.is_inferable(),
            examples_ok: ok.iter().map(|s| s.to_string()).collect(),
            examples_bad: bad.iter().map(|s| s.to_string()).collect(),
            notes,
        }
    }
}

impl fmt::Display for ValueDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一个值域的机器可读说明（`cercis value-types --json` 的每个元素）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainSpec {
    pub name: &'static str,
    pub label_cn: &'static str,
    pub label_en: &'static str,
    /// 在 CSV 里长什么样
    pub csv_form: &'static str,
    /// **机器可执行的判定规则**：插件照它实现即可，不必抄内核代码
    pub matcher: Matcher,
    /// 违反该类型时的强度：error（阻止写入）/ warning（只提醒）
    pub severity_on_violation: &'static str,
    /// 是否参与类型推断
    pub inferable: bool,
    pub examples_ok: Vec<String>,
    pub examples_bad: Vec<String>,
    pub notes: &'static str,
}

/// 判定规则（供插件/导出工具按同一份规则执行）。
///
/// 只有几种「动词」，任何语言都能照着实现：
/// - `any`：不做类型检查；
/// - `regex`：**整格匹配**（等价于 `^(?:pattern)$`）；
/// - `enum`：取值必须出现在 `values` 里（可选忽略 ASCII 大小写）；
/// - `builtin`：正则表达不了的具名检查器，见 [`run_builtin`]。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Matcher {
    Any,
    Regex {
        pattern: String,
    },
    #[serde(rename = "enum")]
    Enum {
        values: Vec<String>,
        #[serde(default)]
        ascii_case_insensitive: bool,
    },
    Builtin {
        checker: String,
    },
}

/// 具名检查器：正则表达不了的规则在这里实现（**唯一实现**，插件按名字调用）。
///
/// 未知检查器一律**放行**（返回 `true`）：将来内核加了新检查器，老插件只是
/// "少检查一项"，不会把好数据拦在门外。
pub fn run_builtin(checker: &str, value: &str) -> bool {
    match checker {
        "iso_date" => format::is_iso_date(value.trim()),
        "iso8601_tz" => is_iso8601_with_tz(value.trim()),
        _ => true,
    }
}

/// ISO 8601 且**带时区**；允许省略秒（文档示例就是 `2026-06-15T09:30+08:00`）。
///
/// RFC 3339 语法上要求秒，因此先按它解析，再退回到不带秒的 ISO 8601 写法。
fn is_iso8601_with_tz(s: &str) -> bool {
    if ids::parse_iso8601(s).is_ok() {
        return true;
    }
    const WITH_TZ: [&str; 3] = [
        "%Y-%m-%dT%H:%M%:z",
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%dT%H:%M:%S%.f%:z",
    ];
    WITH_TZ
        .iter()
        .any(|f| chrono::DateTime::parse_from_str(s, f).is_ok())
}

/// 约束的机器可读说明。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintSpec {
    pub name: &'static str,
    pub label_cn: &'static str,
    /// 适用于哪些类型（可读文本；机器判断用 [`constraint_applies_to`]）
    pub applies_to: &'static str,
    pub severity: &'static str,
    pub notes: &'static str,
}

/// 六种约束 + `unit`（不校验，仅展示与导出用）。
///
/// 对应 §5.9.2：`required` / `allowed_values` / `pattern` / `reference` 是**错误**；
/// `range` / `precision` 是**提醒**。
pub const CONSTRAINT_SPECS: [ConstraintSpec; 7] = [
    ConstraintSpec {
        name: "required",
        label_cn: "必填",
        applies_to: "所有类型",
        severity: "error",
        notes: "空单元格即违规。",
    },
    ConstraintSpec {
        name: "range",
        label_cn: "建议范围",
        applies_to: "integer / number",
        severity: "warning",
        notes: "[min, max]；超出只提醒，用来挡笔误（1750 写成 17500）。",
    },
    ConstraintSpec {
        name: "allowed_values",
        label_cn: "允许值",
        applies_to: "所有类型",
        severity: "error",
        notes: "分号分隔的清单；不在清单内即违规（写错分类会污染下游统计）。",
    },
    ConstraintSpec {
        name: "pattern",
        label_cn: "正则模式",
        applies_to: "text",
        severity: "error",
        notes: "整格匹配（内部按 ^(?:pattern)$ 执行）；编号格式、样品编号用它。",
    },
    ConstraintSpec {
        name: "precision",
        label_cn: "小数位数",
        applies_to: "number",
        severity: "warning",
        notes: "超过位数只提醒（称量/仪器精度）；不四舍五入、不改数据。",
    },
    ConstraintSpec {
        name: "unit",
        label_cn: "计量单位",
        applies_to: "integer / number",
        severity: "none",
        notes: "不校验，仅展示与导出用。",
    },
    ConstraintSpec {
        name: "reference",
        label_cn: "引用",
        applies_to: "所有类型",
        severity: "error",
        notes: "`表名.列名`；被引用的记录必须存在（内核查引用完整性）。",
    },
];

/// 某个约束是否适用于某个值域（模板校验用；拦住"给日期配小数位数"这类无效组合）。
pub fn constraint_applies_to(constraint: &str, domain: ValueDomain) -> bool {
    match constraint {
        "required" | "allowed_values" | "reference" => true,
        "range" | "unit" => matches!(domain, ValueDomain::Integer | ValueDomain::Number),
        "pattern" => domain == ValueDomain::Text,
        "precision" => domain == ValueDomain::Number,
        _ => true,
    }
}

/// 一个字段上的约束（由模板的 `FieldDef` 转换而来）。
#[derive(Debug, Clone, Default)]
pub struct Constraints<'a> {
    pub required: bool,
    pub unit: &'a str,
    pub range: Option<&'a [Option<f64>]>,
    pub allowed_values: &'a str,
    pub pattern: &'a str,
    pub precision: Option<u8>,
    pub reference: Option<&'a str>,
}

/// 一条校验结论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub severity: Severity,
    /// 规则代号：required / type / range / allowed_values / pattern / precision /
    /// missing_marker / nfc
    pub kind: &'static str,
    pub message: String,
}

impl Finding {
    fn error(kind: &'static str, message: String) -> Self {
        Finding {
            severity: Severity::Error,
            kind,
            message,
        }
    }

    fn warning(kind: &'static str, message: String) -> Self {
        Finding {
            severity: Severity::Warning,
            kind,
            message,
        }
    }
}

/// 一个单元格是否匹配某个值域（**不含**约束）。
pub fn matches_domain(domain: ValueDomain, raw: &str) -> bool {
    match &domain.spec().matcher {
        Matcher::Any => true,
        Matcher::Regex { pattern } => matches_regex(pattern, raw),
        Matcher::Enum {
            values,
            ascii_case_insensitive,
        } => {
            let v = raw.trim();
            values.iter().any(|x| {
                if *ascii_case_insensitive {
                    x.eq_ignore_ascii_case(v)
                } else {
                    x == v
                }
            })
        }
        Matcher::Builtin { checker } => run_builtin(checker, raw),
    }
}

/// 整格匹配正则（`^(?:pattern)$`）。模式非法时返回 `false`（模板校验会先拦住非法模式）。
fn matches_regex(pattern: &str, value: &str) -> bool {
    match regex::Regex::new(&format!("^(?:{pattern})$")) {
        Ok(re) => re.is_match(value.trim()),
        Err(_) => false,
    }
}

/// 编译一个模板里的 pattern，检查是否合法（模板校验用）。
pub fn validate_pattern(pattern: &str) -> Result<(), String> {
    regex::Regex::new(&format!("^(?:{pattern})$"))
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// 校验一个单元格（**唯一实现**：体检、写入前检查、导出前清单都调它）。
///
/// 规则顺序：必填 → 缺失值写法（提醒）→ 类型 → 允许值 → 范围 → 精度 → 模式 → NFC（提醒）。
/// 空单元格只受 `required` 约束；其余检查一律跳过（"未记录"不等于"值错"）。
pub fn check_value(domain: ValueDomain, c: &Constraints<'_>, raw: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    let value = raw.trim();

    if value.is_empty() {
        if c.required {
            out.push(Finding::error("required", "必填项为空".into()));
        }
        return out;
    }

    if format::is_missing_marker(raw) {
        out.push(Finding::warning(
            "missing_marker",
            format!("疑似把缺失写成 {raw:?}；建议留空单元格（未记录 ≠ 值为 0）"),
        ));
    }

    if !matches_domain(domain, value) {
        let severity = if domain.spec().severity_on_violation == "warning" {
            Severity::Warning
        } else {
            Severity::Error
        };
        out.push(Finding {
            severity,
            kind: "type",
            message: domain_violation_message(domain, value),
        });
    }

    // 允许值（错误级）
    if !c.allowed_values.trim().is_empty() {
        let allowed: Vec<&str> = c
            .allowed_values
            .split(';')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        if !allowed.is_empty() && !allowed.contains(&value) {
            out.push(Finding::error(
                "allowed_values",
                format!("取值 {value:?} 不在允许值列表（{}）中", allowed.join("; ")),
            ));
        }
    }

    // 建议范围（提醒级）
    if let (Some(range), Ok(num)) = (c.range, value.parse::<f64>()) {
        if let Some(Some(min)) = range.first() {
            if num < *min {
                out.push(Finding::warning(
                    "range",
                    format!("小于建议下限 {min}（实际 {num}）；确认是否笔误"),
                ));
            }
        }
        if let Some(Some(max)) = range.get(1) {
            if num > *max {
                out.push(Finding::warning(
                    "range",
                    format!("大于建议上限 {max}（实际 {num}）；确认是否笔误"),
                ));
            }
        }
    }

    // 小数位数（提醒级）
    if let Some(p) = c.precision {
        if let Some(decimals) = decimal_places(value) {
            if decimals > p as usize {
                out.push(Finding::warning(
                    "precision",
                    format!("小数位数 {decimals} 超过约定的 {p} 位"),
                ));
            }
        }
    }

    // 正则模式（错误级）
    if !c.pattern.trim().is_empty() {
        if validate_pattern(c.pattern).is_err() {
            out.push(Finding::warning(
                "pattern",
                format!("模板里的 pattern 不是合法正则：{:?}", c.pattern),
            ));
        } else if !matches_regex(c.pattern, value) {
            out.push(Finding::error(
                "pattern",
                format!("不符合约定的格式 /{}/", c.pattern),
            ));
        }
    }

    if !format::is_nfc(raw) {
        out.push(Finding::warning("nfc", "未做 Unicode NFC 归一化".into()));
    }

    out
}

/// 类型不符时的可读说明（说清"应该长什么样"）。
fn domain_violation_message(domain: ValueDomain, value: &str) -> String {
    let spec = domain.spec();
    let hint = spec
        .examples_ok
        .first()
        .map(|e| format!("，例如 {e:?}"))
        .unwrap_or_default();
    let extra = if format::is_missing_marker(value) {
        "（缺失应留空，不要写占位符）"
    } else {
        ""
    };
    format!(
        "不是{}：{:?}（应为 {}{hint}）{extra}",
        spec.label_cn, value, spec.csv_form
    )
}

/// 小数位数（不带小数点或不是数字时返回 None）。
pub fn decimal_places(value: &str) -> Option<usize> {
    let v = value.trim();
    if v.contains(',') {
        return None;
    }
    let frac = v.split_once('.')?.1;
    if frac.is_empty() || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(frac.len())
}

/// 机器可读的类型契约（`cercis value-types --json` 的主体）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeContract {
    /// 输出用的 schema 标识
    pub schema: &'static str,
    pub core_version: &'static str,
    /// 七种值域
    pub domains: Vec<DomainSpec>,
    /// 六种约束（含不校验的 unit）
    pub constraints: Vec<ConstraintSpec>,
    /// 缺失值规则（一句话）
    pub missing_value_rule: &'static str,
    /// 不许当缺失值用的写法
    pub missing_markers: Vec<String>,
    /// 校验时点
    pub validation_points: Vec<&'static str>,
    /// 强度总原则
    pub severity_policy: &'static str,
}

/// 生成类型契约。
pub fn contract() -> TypeContract {
    TypeContract {
        schema: "cercis.value_types/1",
        core_version: crate::CORE_VERSION,
        domains: ValueDomain::ALL.iter().map(|d| d.spec()).collect(),
        constraints: CONSTRAINT_SPECS.to_vec(),
        missing_value_rule: "缺失值一律留空单元格；写 NA / - / 0 代替只给提醒，不给错误",
        missing_markers: format::MISSING_MARKERS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        validation_points: vec![
            "写入前（录入 / 导入）：有问题整批不写，先试运行",
            "项目体检（cercis validate）：逐格校验，只报告不改数据",
            "导出前（可选）：说明书附未通过校验的格子清单，导出永不阻断",
        ],
        severity_policy:
            "结构错误（类型/必填/允许值/模式/引用）阻止写入；范围与精度只提醒；导出永不因校验阻断",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain() -> Constraints<'static> {
        Constraints::default()
    }

    fn errors(domain: ValueDomain, value: &str) -> Vec<String> {
        check_value(domain, &plain(), value)
            .into_iter()
            .filter(|f| f.severity == Severity::Error)
            .map(|f| f.message)
            .collect()
    }

    fn warnings(domain: ValueDomain, value: &str) -> Vec<String> {
        check_value(domain, &plain(), value)
            .into_iter()
            .filter(|f| f.severity == Severity::Warning)
            .map(|f| f.message)
            .collect()
    }

    #[test]
    fn number_and_integer_rules() {
        assert!(errors(ValueDomain::Number, "12.5").is_empty());
        assert!(errors(ValueDomain::Number, "0.05").is_empty());
        assert!(errors(ValueDomain::Number, "-1.5").is_empty());
        for bad in ["1e3", "1,200.5", "十二", "NaN", ".5"] {
            assert!(
                !errors(ValueDomain::Number, bad).is_empty(),
                "{bad} 应为错误"
            );
        }
        assert!(errors(ValueDomain::Integer, "12").is_empty());
        assert!(!errors(ValueDomain::Integer, "12.5").is_empty());
        assert!(!errors(ValueDomain::Integer, "1,200").is_empty());
    }

    #[test]
    fn date_datetime_bool_time_severity() {
        // 日期写错是错误（会污染按日期的排序与筛选）
        assert!(errors(ValueDomain::Date, "2026-06-15").is_empty());
        assert!(!errors(ValueDomain::Date, "2026/6/15").is_empty());
        assert!(!errors(ValueDomain::Date, "2026-02-30").is_empty());
        // 其余三种只提醒
        assert!(errors(ValueDomain::Bool, "Y").is_empty());
        assert!(!warnings(ValueDomain::Bool, "Y").is_empty());
        assert!(errors(ValueDomain::Time, "9:30").is_empty());
        assert!(!warnings(ValueDomain::Time, "9:30").is_empty());
        assert!(warnings(ValueDomain::Time, "09:30").is_empty());
        assert!(errors(ValueDomain::Datetime, "2026-06-15 09:30").is_empty());
        assert!(!warnings(ValueDomain::Datetime, "2026-06-15 09:30").is_empty());
        assert!(warnings(ValueDomain::Datetime, "2026-06-15T09:30+08:00").is_empty());
    }

    #[test]
    fn required_stops_at_empty_and_missing_marker_only_warns() {
        let c = Constraints {
            required: true,
            ..Default::default()
        };
        let findings = check_value(ValueDomain::Number, &c, "");
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, "required");
        assert_eq!(findings[0].severity, Severity::Error);

        // 缺失写成 NA：只提醒，不阻止
        let findings = check_value(ValueDomain::Number, &plain(), "NA");
        assert!(findings
            .iter()
            .any(|f| f.kind == "missing_marker" && f.severity == Severity::Warning));

        // 空值不算类型错误（未记录 ≠ 值错）
        assert!(check_value(ValueDomain::Number, &plain(), "  ").is_empty());
    }

    #[test]
    fn allowed_values_is_error_range_and_precision_only_warn() {
        let c = Constraints {
            allowed_values: "是;否",
            ..Default::default()
        };
        assert!(check_value(ValueDomain::Bool, &c, "是").is_empty());
        assert!(check_value(ValueDomain::Text, &c, "也许")
            .iter()
            .any(|f| f.kind == "allowed_values" && f.severity == Severity::Error));

        let range = vec![Some(0.0), Some(250.0)];
        let c = Constraints {
            range: Some(&range),
            ..Default::default()
        };
        assert!(check_value(ValueDomain::Number, &c, "180")
            .iter()
            .all(|f| f.severity == Severity::Warning));
        assert!(check_value(ValueDomain::Number, &c, "1800")
            .iter()
            .any(|f| f.kind == "range" && f.severity == Severity::Warning));

        let c = Constraints {
            precision: Some(1),
            ..Default::default()
        };
        assert!(check_value(ValueDomain::Number, &c, "12.3").is_empty());
        assert!(check_value(ValueDomain::Number, &c, "12.34")
            .iter()
            .any(|f| f.kind == "precision" && f.severity == Severity::Warning));
    }

    #[test]
    fn pattern_is_full_cell_match_and_error() {
        let c = Constraints {
            pattern: r"SB-\d{4}",
            ..Default::default()
        };
        assert!(check_value(ValueDomain::Text, &c, "SB-0001").is_empty());
        // 整格匹配：前后有多余字符也算不符
        assert!(check_value(ValueDomain::Text, &c, "SB-0001-A")
            .iter()
            .any(|f| f.kind == "pattern" && f.severity == Severity::Error));
        // 非法正则不拦数据，只提醒（免得模板笔误把数据锁死）
        let c = Constraints {
            pattern: "([",
            ..Default::default()
        };
        let findings = check_value(ValueDomain::Text, &c, "随便");
        assert!(findings
            .iter()
            .all(|f| f.severity == Severity::Warning));
    }

    #[test]
    fn contract_matches_document() {
        let c = contract();
        assert_eq!(c.domains.len(), 7);
        assert_eq!(c.constraints.len(), 7);
        let names: Vec<&str> = c.domains.iter().map(|d| d.name).collect();
        assert_eq!(
            names,
            vec!["text", "integer", "number", "bool", "date", "datetime", "time"]
        );
        assert!(!c.domains.iter().find(|d| d.name == "time").unwrap().inferable);
        // 机器规则要具体到能照着实现
        let num = c.domains.iter().find(|d| d.name == "number").unwrap();
        match &num.matcher {
            Matcher::Regex { pattern } => assert_eq!(pattern, r"^-?[0-9]+(\.[0-9]+)?$"),
            other => panic!("number 应为 regex：{other:?}"),
        }
    }

    #[test]
    fn constraint_applicability_follows_spec() {
        assert!(constraint_applies_to("pattern", ValueDomain::Text));
        assert!(!constraint_applies_to("pattern", ValueDomain::Date));
        assert!(!constraint_applies_to("precision", ValueDomain::Integer));
        assert!(constraint_applies_to("precision", ValueDomain::Number));
        assert!(constraint_applies_to("required", ValueDomain::Time));
        assert!(!constraint_applies_to("range", ValueDomain::Text));
    }

    #[test]
    fn unknown_builtin_checker_passes() {
        assert!(run_builtin("未来才有的检查器", "随便"));
        assert!(!run_builtin("iso_date", "2026/01/01"));
    }
}
