//! 模板：表结构与字段语义的定义机制。
//!
//! ## 为什么模板在内核
//! 内核只认"表、列、类型"，**不认识任何学科概念**。那么"研究/样本/观测"这些
//! 含义从哪来？——来自**模板**。模板是一份声明文件（YAML），描述：
//! - 有哪些表、每张表有哪些列、列的类型与是否必填
//! - 每列的显示名（中/英）、单位、取值范围
//! - 表之间的引用关系（引用完整性检查的依据）
//!
//! 换一份模板，同一个内核就变成不同的系统（示例研究模板 / 企业台账模板 / …）。
//!
//! ## 引用完整性
//! 模板声明 `ref: <表名>.<列名>`，内核据此检查"引用的记录是否存在"。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::format;
use crate::tx;
use crate::value::{self, ValueDomain};
use crate::{CercisError, Result};

/// 字段（列）定义。
///
/// **类型的语义由内核提供**（[`crate::value`]）：这里的 `field_type` 只能是内核
/// 七种值域之一，校验规则也由内核执行；模板负责的是"这一列是什么意思"——
/// 显示名、单位、必填、合理范围、允许值、正则模式、小数位数、引用关系。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDef {
    /// 列名（小写 ASCII 下划线）
    pub name: String,
    /// 中文显示名
    #[serde(default)]
    pub label_cn: String,
    /// 英文显示名
    #[serde(default)]
    pub label_en: String,
    /// 值域：text / integer / number / bool / date / datetime / time（见 §5.9.1）
    #[serde(rename = "type", default = "default_type")]
    pub field_type: String,
    /// 单位（数值类）
    #[serde(default)]
    pub unit: String,
    /// 是否必填
    #[serde(default)]
    pub required: bool,
    /// 取值范围（数值类）：[min, max]，任一可为空
    #[serde(default)]
    pub range: Option<Vec<Option<f64>>>,
    /// 引用：`<表名>.<列名>`，用于引用完整性检查
    #[serde(default)]
    pub reference: Option<String>,
    /// 允许值（分类字段；分号分隔）
    #[serde(default)]
    pub allowed_values: String,
    /// 正则模式（text 专用；**整格匹配**，即内部按 `^(?:pattern)$` 执行）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
    /// 小数位数（number 专用；超出只提醒，不四舍五入）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<u8>,
    /// 说明
    #[serde(default)]
    pub notes: String,
}

fn default_type() -> String {
    "text".to_string()
}

impl FieldDef {
    /// 该字段的值域。
    ///
    /// 模板在载入/保存时都会校验类型，所以正常总能解析；万一碰到旧模板里的
    /// 未知类型，按 `text` 处理（**放行不拦**——数据永远可读）。
    pub fn domain(&self) -> ValueDomain {
        ValueDomain::parse(&self.field_type).unwrap_or(ValueDomain::Text)
    }

    /// 转成内核校验用的约束集合（[`crate::value::check_value`] 的输入）。
    pub fn constraints(&self) -> value::Constraints<'_> {
        value::Constraints {
            required: self.required,
            unit: &self.unit,
            range: self.range.as_deref(),
            allowed_values: &self.allowed_values,
            pattern: &self.pattern,
            precision: self.precision,
            reference: self.reference.as_deref(),
        }
    }
}

/// 字段补丁：改模板时的"变更单"。
///
/// 只写要改的部分；`clear_*` 表示"清掉这一项"。用"补丁"而不是"整份重写"
/// 是为了让变更留痕说得清（"把 value_num 从 text 改成 number"），
/// 也避免误伤同一张表的其他列。
#[derive(Debug, Clone, Default)]
pub struct FieldPatch {
    /// 改成哪个值域（text / integer / number / bool / date / datetime / time）
    pub field_type: Option<String>,
    /// 必填？`Some(false)` 表示改成可选
    pub required: Option<bool>,
    /// 计量单位（空串 = 清空）
    pub unit: Option<String>,
    /// 建议范围 [min, max]（数值类）
    pub range: Option<Vec<Option<f64>>>,
    /// 清掉范围
    pub clear_range: bool,
    /// 允许值（分号分隔；空串 = 清空）
    pub allowed_values: Option<String>,
    /// 正则模式（text 专用；空串 = 清空）
    pub pattern: Option<String>,
    /// 小数位数（number 专用）
    pub precision: Option<u8>,
    /// 清掉小数位数
    pub clear_precision: bool,
    /// 引用 `表名.列名`（空串 = 清空）
    pub reference: Option<String>,
    /// 说明
    pub notes: Option<String>,
}

impl FieldPatch {
    /// 是否什么都没改。
    pub fn is_empty(&self) -> bool {
        self.field_type.is_none()
            && self.required.is_none()
            && self.unit.is_none()
            && self.range.is_none()
            && !self.clear_range
            && self.allowed_values.is_none()
            && self.pattern.is_none()
            && self.precision.is_none()
            && !self.clear_precision
            && self.reference.is_none()
            && self.notes.is_none()
    }

    /// 人可读的变更摘要（写进变更流水）。
    pub fn describe(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(t) = &self.field_type {
            parts.push(format!("type→{t}"));
        }
        if let Some(r) = self.required {
            parts.push(if r { "改为必填".into() } else { "改为可选".into() });
        }
        if let Some(u) = &self.unit {
            parts.push(if u.is_empty() {
                "清空单位".into()
            } else {
                format!("unit={u}")
            });
        }
        if self.clear_range {
            parts.push("清空范围".into());
        } else if let Some(r) = &self.range {
            parts.push(format!(
                "range=[{}, {}]",
                r.first().copied().flatten().map(|v| v.to_string()).unwrap_or_default(),
                r.get(1).copied().flatten().map(|v| v.to_string()).unwrap_or_default()
            ));
        }
        if let Some(a) = &self.allowed_values {
            parts.push(if a.is_empty() {
                "清空允许值".into()
            } else {
                format!("allowed_values={a}")
            });
        }
        if let Some(p) = &self.pattern {
            parts.push(if p.is_empty() {
                "清空模式".into()
            } else {
                format!("pattern=/{p}/")
            });
        }
        if self.clear_precision {
            parts.push("清空小数位数".into());
        } else if let Some(p) = self.precision {
            parts.push(format!("precision={p}"));
        }
        if let Some(r) = &self.reference {
            parts.push(if r.is_empty() {
                "清空引用".into()
            } else {
                format!("reference={r}")
            });
        }
        if self.notes.is_some() {
            parts.push("更新说明".into());
        }
        if parts.is_empty() {
            "（没有改动）".into()
        } else {
            parts.join("、")
        }
    }
}

/// 表定义。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableDef {
    /// 表名（对应 tables/<name>.csv）
    pub name: String,
    #[serde(default)]
    pub label_cn: String,
    #[serde(default)]
    pub label_en: String,
    /// 主键列名（用于编号唯一性与引用检查）
    #[serde(default)]
    pub primary_key: String,
    /// 列定义
    pub fields: Vec<FieldDef>,
    #[serde(default)]
    pub notes: String,
}

/// 模板：一组表定义 + 术语包名称。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    /// 模板名（如 generic / example-study / company-ledger）
    pub name: String,
    #[serde(default)]
    pub label_cn: String,
    #[serde(default)]
    pub label_en: String,
    /// 术语包（显示层用，见文档 §5.6）
    #[serde(default)]
    pub terms_profile: String,
    /// 表定义
    pub tables: Vec<TableDef>,
    #[serde(default)]
    pub notes: String,
}

impl Template {
    /// 从 YAML 文本解析并校验。
    pub fn from_yaml(text: &str) -> Result<Self> {
        let t: Template = serde_yaml::from_str(text)?;
        t.validate()?;
        Ok(t)
    }

    /// 从文件载入。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = tx::read_optional(path.as_ref())?.ok_or_else(|| {
            CercisError::NotFound(format!("模板不存在: {}", path.as_ref().display()))
        })?;
        Self::from_yaml(&text)
    }

    /// 保存为 YAML。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        tx::write_atomic_str(path.as_ref(), &serde_yaml::to_string(self)?)
    }

    /// 结构校验：名称规范、无重复、引用目标存在。
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(CercisError::Invalid("模板名称不能为空".into()));
        }
        let mut table_names = std::collections::BTreeSet::new();
        for t in &self.tables {
            if !format::is_ascii_snake(&t.name) {
                return Err(CercisError::Invalid(format!(
                    "表名需为小写 ASCII 下划线形式: {:?}",
                    t.name
                )));
            }
            if !table_names.insert(t.name.clone()) {
                return Err(CercisError::Invalid(format!("表名重复: {}", t.name)));
            }
            if t.fields.is_empty() {
                return Err(CercisError::Invalid(format!(
                    "表 {} 没有任何列定义",
                    t.name
                )));
            }
            let mut field_names = std::collections::BTreeSet::new();
            for f in &t.fields {
                if !format::is_ascii_snake(&f.name) {
                    return Err(CercisError::Invalid(format!(
                        "列名需为小写 ASCII 下划线形式: {}.{}",
                        t.name, f.name
                    )));
                }
                if !field_names.insert(f.name.clone()) {
                    return Err(CercisError::Invalid(format!(
                        "列名重复: {}.{}",
                        t.name, f.name
                    )));
                }
                // 类型必须来自内核的七种值域（插件不得自造类型，见 D36 / §5.9）
                let Some(domain) = ValueDomain::parse(&f.field_type) else {
                    return Err(CercisError::Invalid(format!(
                        "未知列类型 {}.{}: {}（可用：{}）",
                        t.name,
                        f.name,
                        f.field_type,
                        ValueDomain::ALL
                            .iter()
                            .map(|d| d.as_str())
                            .collect::<Vec<_>>()
                            .join(" / ")
                    )));
                };
                // 约束与类型的搭配（拦住"给日期配小数位数"这类无效组合）
                for (name, used) in [
                    ("pattern", !f.pattern.is_empty()),
                    ("precision", f.precision.is_some()),
                    ("range", f.range.is_some()),
                ] {
                    if used && !value::constraint_applies_to(name, domain) {
                        return Err(CercisError::Invalid(format!(
                            "{} 不适用于 {} 类型：{}.{}",
                            name, domain, t.name, f.name
                        )));
                    }
                }
                if let Some(range) = &f.range {
                    if range.len() != 2 {
                        return Err(CercisError::Invalid(format!(
                            "范围需为 [min, max] 两项: {}.{}",
                            t.name, f.name
                        )));
                    }
                }
                if !f.pattern.is_empty() {
                    if let Err(e) = value::validate_pattern(&f.pattern) {
                        return Err(CercisError::Invalid(format!(
                            "pattern 不是合法正则 {}.{}: {}（{e}）",
                            t.name, f.name, f.pattern
                        )));
                    }
                }
                if let Some(p) = f.precision {
                    if p > 15 {
                        return Err(CercisError::Invalid(format!(
                            "小数位数最多 15 位: {}.{} = {p}",
                            t.name, f.name
                        )));
                    }
                }
            }
            if !t.primary_key.is_empty() && !field_names.contains(&t.primary_key) {
                return Err(CercisError::Invalid(format!(
                    "表 {} 的主键列 {} 不在列定义中",
                    t.name, t.primary_key
                )));
            }
        }
        // 引用完整性：引用目标必须存在
        for t in &self.tables {
            for f in &t.fields {
                if let Some(r) = &f.reference {
                    let (rt, rf) = r.split_once('.').ok_or_else(|| {
                        CercisError::Invalid(format!(
                            "引用格式应为 表名.列名: {}.{} -> {r}",
                            t.name, f.name
                        ))
                    })?;
                    let target = self.tables.iter().find(|x| x.name == rt).ok_or_else(|| {
                        CercisError::Invalid(format!(
                            "引用目标表不存在: {}.{} -> {}",
                            t.name, f.name, rt
                        ))
                    })?;
                    if !target.fields.iter().any(|x| x.name == rf) {
                        return Err(CercisError::Invalid(format!(
                            "引用目标列不存在: {}.{} -> {}",
                            t.name, f.name, r
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    /// 取表定义。
    pub fn table(&self, name: &str) -> Option<&TableDef> {
        self.tables.iter().find(|t| t.name == name)
    }

    /// 取字段定义。
    pub fn field(&self, table: &str, field: &str) -> Option<&FieldDef> {
        self.table(table)?
            .fields
            .iter()
            .find(|f| f.name == field)
    }

    /// 应用字段补丁，返回**新模板**（原模板不动）。
    ///
    /// 结果经过 [`Template::validate`]，因此非法组合（未知类型、给日期配小数位数、
    /// 非法正则、引用目标不存在）在这里就会被拦住——**写模板也必须走内核通道**。
    pub fn with_field_patch(
        &self,
        table: &str,
        field: &str,
        patch: &FieldPatch,
    ) -> Result<Template> {
        if patch.is_empty() {
            return Err(CercisError::Invalid(
                "没有要改的内容（至少给一项：--type / --required / --range / --pattern / …）"
                    .into(),
            ));
        }
        let mut next = self.clone();
        let table_index = next
            .tables
            .iter()
            .position(|t| t.name == table)
            .ok_or_else(|| CercisError::NotFound(format!("模板里没有表 {table}")))?;
        if next.tables[table_index].fields.iter().all(|f| f.name != field) {
            let available: Vec<&str> = next.tables[table_index]
                .fields
                .iter()
                .map(|f| f.name.as_str())
                .collect();
            return Err(CercisError::NotFound(format!(
                "表 {table} 里没有列 {field}（可用：{}）",
                available.join(", ")
            )));
        }
        let fd = next.tables[table_index]
            .fields
            .iter_mut()
            .find(|f| f.name == field)
            .expect("上面已确认该列存在");

        if let Some(t) = &patch.field_type {
            if ValueDomain::parse(t).is_none() {
                return Err(CercisError::Invalid(format!(
                    "未知类型 {t}（可用：{}）",
                    ValueDomain::ALL
                        .iter()
                        .map(|d| d.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ")
                )));
            }
            fd.field_type = t.trim().to_ascii_lowercase();
        }
        if let Some(r) = patch.required {
            fd.required = r;
        }
        if let Some(u) = &patch.unit {
            fd.unit = u.clone();
        }
        if patch.clear_range {
            fd.range = None;
        } else if let Some(r) = &patch.range {
            fd.range = Some(r.clone());
        }
        if let Some(a) = &patch.allowed_values {
            fd.allowed_values = a.clone();
        }
        if let Some(p) = &patch.pattern {
            fd.pattern = p.clone();
        }
        if patch.clear_precision {
            fd.precision = None;
        } else if let Some(p) = patch.precision {
            fd.precision = Some(p);
        }
        if let Some(r) = &patch.reference {
            fd.reference = if r.trim().is_empty() {
                None
            } else {
                Some(r.trim().to_string())
            };
        }
        if let Some(n) = &patch.notes {
            fd.notes = n.clone();
        }

        // 换类型之后，几项约束可能不再适用。两种情形分开处理：
        // - 用户这次**明确**给的约束不适用 → 报错说清楚（不替他猜）；
        // - 只是旧配置的残留 → 自动清掉（否则"把 text 列改成 number"这种常见操作
        //   会被遗留的 pattern 卡住）。
        let domain = fd.domain();
        let explicit: [(&str, bool); 3] = [
            ("range", patch.range.is_some()),
            ("pattern", patch.pattern.as_ref().is_some_and(|p| !p.is_empty())),
            ("precision", patch.precision.is_some()),
        ];
        for (name, given) in explicit {
            if given && !value::constraint_applies_to(name, domain) {
                return Err(CercisError::Invalid(format!(
                    "{name} 不适用于 {domain} 类型（列 {}）——去掉它，或换一个类型",
                    fd.name
                )));
            }
        }
        if patch.range.is_none() && !value::constraint_applies_to("range", domain) {
            fd.range = None;
        }
        if patch.pattern.is_none() && !value::constraint_applies_to("pattern", domain) {
            fd.pattern = String::new();
        }
        if patch.precision.is_none() && !value::constraint_applies_to("precision", domain) {
            fd.precision = None;
        }

        next.validate()?;
        Ok(next)
    }

    /// 表的列名（按定义顺序）。
    pub fn columns_of(&self, name: &str) -> Option<Vec<&str>> {
        self.table(name)
            .map(|t| t.fields.iter().map(|f| f.name.as_str()).collect())
    }

    /// 汇总为"字段 → 定义"映射，便于快速查询。
    pub fn field_map(&self) -> BTreeMap<(String, String), FieldDef> {
        let mut m = BTreeMap::new();
        for t in &self.tables {
            for f in &t.fields {
                m.insert((t.name.clone(), f.name.clone()), f.clone());
            }
        }
        m
    }

    /// 通用模板：最小可用的起步模板（不含任何学科词）。
    pub fn generic() -> Self {
        Template {
            name: "generic".into(),
            label_cn: "通用（最小可用）".into(),
            label_en: "Generic (minimal)".into(),
            terms_profile: "generic".into(),
            notes: "最小模板：只有一张记录表和一张变更说明表；真实用途请加载专用模板。".into(),
            tables: vec![TableDef {
                name: "records".into(),
                label_cn: "记录".into(),
                label_en: "Records".into(),
                primary_key: "record_id".into(),
                notes: String::new(),
                fields: vec![
                    FieldDef {
                        name: "record_id".into(),
                        label_cn: "记录编号".into(),
                        label_en: "Record ID".into(),
                        field_type: "text".into(),
                        unit: String::new(),
                        required: true,
                        range: None,
                        reference: None,
                        allowed_values: String::new(),
                        pattern: String::new(),
                        precision: None,
                        notes: "全局唯一，人可读".into(),
                    },
                    FieldDef {
                        name: "date".into(),
                        label_cn: "日期".into(),
                        label_en: "Date".into(),
                        field_type: "date".into(),
                        unit: String::new(),
                        required: true,
                        range: None,
                        reference: None,
                        allowed_values: String::new(),
                        pattern: String::new(),
                        precision: None,
                        notes: "ISO 8601（YYYY-MM-DD）".into(),
                    },
                    FieldDef {
                        name: "label".into(),
                        label_cn: "标签".into(),
                        label_en: "Label".into(),
                        field_type: "text".into(),
                        unit: String::new(),
                        required: false,
                        range: None,
                        reference: None,
                        allowed_values: String::new(),
                        pattern: String::new(),
                        precision: None,
                        notes: String::new(),
                    },
                    FieldDef {
                        name: "value_num".into(),
                        label_cn: "数值".into(),
                        label_en: "Numeric value".into(),
                        field_type: "number".into(),
                        unit: String::new(),
                        required: false,
                        range: None,
                        reference: None,
                        allowed_values: String::new(),
                        pattern: String::new(),
                        precision: None,
                        notes: String::new(),
                    },
                    FieldDef {
                        name: "notes".into(),
                        label_cn: "备注".into(),
                        label_en: "Notes".into(),
                        field_type: "text".into(),
                        unit: String::new(),
                        required: false,
                        range: None,
                        reference: None,
                        allowed_values: String::new(),
                        pattern: String::new(),
                        precision: None,
                        notes: String::new(),
                    },
                ],
            }],
        }
    }

    /// 示例研究模板（合成示例；展示多表 + 引用关系，不含任何真实内容）。
    pub fn example_study() -> Self {
        Template {
            name: "example-study".into(),
            label_cn: "示例研究模板".into(),
            label_en: "Example study template".into(),
            terms_profile: "generic".into(),
            notes: "演示用途：研究 → 实验单元 → 事件 → 样本 → 观测 的多表结构与引用关系。".into(),
            tables: vec![
                TableDef {
                    name: "studies".into(),
                    label_cn: "研究".into(),
                    label_en: "Studies".into(),
                    primary_key: "study_id".into(),
                    notes: String::new(),
                    fields: vec![
                        field("study_id", "研究编号", "Study ID", "text", true),
                        field("name", "名称", "Name", "text", true),
                        field("stage", "阶段", "Stage", "text", false),
                        field("start_date", "开始日期", "Start date", "date", false),
                        field("end_date", "结束日期", "End date", "date", false),
                        field("notes", "备注", "Notes", "text", false),
                    ],
                },
                TableDef {
                    name: "units".into(),
                    label_cn: "实验单元".into(),
                    label_en: "Units".into(),
                    primary_key: "unit_id".into(),
                    notes: String::new(),
                    fields: vec![
                        field("unit_id", "实验单元编号", "Unit ID", "text", true),
                        FieldDef {
                            name: "study_id".into(),
                            label_cn: "所属研究".into(),
                            label_en: "Study".into(),
                            field_type: "text".into(),
                            unit: String::new(),
                            required: true,
                            range: None,
                            reference: Some("studies.study_id".into()),
                            allowed_values: String::new(),
                            pattern: String::new(),
                            precision: None,
                            notes: String::new(),
                        },
                        field("treatment_id", "处理编号", "Treatment ID", "text", false),
                        field("notes", "备注", "Notes", "text", false),
                    ],
                },
                TableDef {
                    name: "events".into(),
                    label_cn: "事件".into(),
                    label_en: "Events".into(),
                    primary_key: "event_id".into(),
                    notes: String::new(),
                    fields: vec![
                        field("event_id", "事件编号", "Event ID", "text", true),
                        field("date", "日期", "Date", "date", true),
                        FieldDef {
                            name: "study_id".into(),
                            label_cn: "所属研究".into(),
                            label_en: "Study".into(),
                            field_type: "text".into(),
                            unit: String::new(),
                            required: true,
                            range: None,
                            reference: Some("studies.study_id".into()),
                            allowed_values: String::new(),
                            pattern: String::new(),
                            precision: None,
                            notes: String::new(),
                        },
                        field("stage", "阶段", "Stage", "text", false),
                        field("event_type", "类型", "Type", "text", false),
                        field("title", "名称", "Title", "text", false),
                        field("notes", "备注", "Notes", "text", false),
                    ],
                },
                TableDef {
                    name: "samples".into(),
                    label_cn: "样本".into(),
                    label_en: "Samples".into(),
                    primary_key: "sample_id".into(),
                    notes: String::new(),
                    fields: vec![
                        field("sample_id", "样本编号", "Sample ID", "text", true),
                        FieldDef {
                            name: "event_id".into(),
                            label_cn: "来源事件".into(),
                            label_en: "Event".into(),
                            field_type: "text".into(),
                            unit: String::new(),
                            required: true,
                            range: None,
                            reference: Some("events.event_id".into()),
                            allowed_values: String::new(),
                            pattern: String::new(),
                            precision: None,
                            notes: String::new(),
                        },
                        FieldDef {
                            name: "unit_id".into(),
                            label_cn: "来源实验单元".into(),
                            label_en: "Unit".into(),
                            field_type: "text".into(),
                            unit: String::new(),
                            required: false,
                            range: None,
                            reference: Some("units.unit_id".into()),
                            allowed_values: String::new(),
                            pattern: String::new(),
                            precision: None,
                            notes: String::new(),
                        },
                        // F-02（S6 验收发现）：父样本也是样本，必须声明引用，
                        // 否则"子样本指向一个不存在的父样本"检不出来
                        FieldDef {
                            name: "parent_sample_id".into(),
                            label_cn: "父样本".into(),
                            label_en: "Parent sample".into(),
                            field_type: "text".into(),
                            unit: String::new(),
                            required: false,
                            range: None,
                            reference: Some("samples.sample_id".into()),
                            allowed_values: String::new(),
                            pattern: String::new(),
                            precision: None,
                            notes: String::new(),
                        },
                        field("sample_type", "样本类型", "Sample type", "text", false),
                        field("notes", "备注", "Notes", "text", false),
                    ],
                },
                TableDef {
                    name: "observations".into(),
                    label_cn: "观测".into(),
                    label_en: "Observations".into(),
                    primary_key: "obs_id".into(),
                    notes: String::new(),
                    fields: vec![
                        field("obs_id", "记录编号", "Observation ID", "text", true),
                        FieldDef {
                            name: "sample_id".into(),
                            label_cn: "关联样本".into(),
                            label_en: "Sample".into(),
                            field_type: "text".into(),
                            unit: String::new(),
                            required: true,
                            range: None,
                            reference: Some("samples.sample_id".into()),
                            allowed_values: String::new(),
                            pattern: String::new(),
                            precision: None,
                            notes: String::new(),
                        },
                        field("trait_id", "指标编号", "Trait ID", "text", true),
                        field("value_num", "数值", "Value", "number", false),
                        field("unit", "单位", "Unit", "text", false),
                        field("batch_id", "批次", "Batch", "text", false),
                        field("measured_at", "测定时间", "Measured at", "date", false),
                        field("notes", "备注", "Notes", "text", false),
                    ],
                },
            ],
        }
    }
}

fn field(name: &str, cn: &str, en: &str, ty: &str, required: bool) -> FieldDef {
    FieldDef {
        name: name.into(),
        label_cn: cn.into(),
        label_en: en.into(),
        field_type: ty.into(),
        unit: String::new(),
        required,
        range: None,
        reference: None,
        allowed_values: String::new(),
        pattern: String::new(),
        precision: None,
        notes: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_template_is_valid() {
        let t = Template::generic();
        t.validate().unwrap();
        assert_eq!(t.columns_of("records").unwrap().len(), 5);
    }

    #[test]
    fn example_study_template_validates_references() {
        let t = Template::example_study();
        t.validate().unwrap();
        assert!(t.table("observations").is_some());
    }

    #[test]
    fn rejects_bad_reference() {
        let mut t = Template::generic();
        t.tables[0].fields[0].reference = Some("nope.nope".into());
        assert!(t.validate().is_err());
    }

    #[test]
    fn yaml_roundtrip() {
        let t = Template::example_study();
        let y = serde_yaml::to_string(&t).unwrap();
        let back = Template::from_yaml(&y).unwrap();
        assert_eq!(back.name, t.name);
        assert_eq!(back.tables.len(), t.tables.len());
    }
}
