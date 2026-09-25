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
use crate::{CercisError, Result};

/// 字段（列）定义。
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
    /// 类型：text / number / integer / date / datetime / bool
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
    /// 说明
    #[serde(default)]
    pub notes: String,
}

fn default_type() -> String {
    "text".to_string()
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
                if !matches!(
                    f.field_type.as_str(),
                    "text" | "number" | "integer" | "date" | "datetime" | "bool"
                ) {
                    return Err(CercisError::Invalid(format!(
                        "未知列类型 {}.{}: {}",
                        t.name, f.name, f.field_type
                    )));
                }
                if let Some(range) = &f.range {
                    if range.len() != 2 {
                        return Err(CercisError::Invalid(format!(
                            "范围需为 [min, max] 两项: {}.{}",
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
