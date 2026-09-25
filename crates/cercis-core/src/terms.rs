//! 术语包：**只改显示用的名字，不动数据**。
//!
//! 同一份数据，农业用户想看到「小区」「处理」，医学用户想看到「病区」「干预」，
//! 生态用户想看到「样地」「处理」。数据的列名（`unit_id`、`treatment_id`）永远不变，
//! 变的只是界面与说明书上显示的中英文标签——这就是需求文档 §5.6 说的术语包。
//!
//! ## 存放位置
//! `<项目>/terms.yaml`（一份，描述当前生效的术语包）。切换术语包 = 换这个文件，
//! **不动任何 CSV**，因此换术语包不影响数据、也不影响历史与引用。
//!
//! ## 文件长什么样
//! ```yaml
//! schema: cercis.terms/1
//! profile: agriculture
//! label_cn: 农业术语包
//! tables:
//!   units:
//!     label_cn: 小区
//!     label_en: Plot
//!     fields:
//!       unit_id: { label_cn: 小区编号 }
//! ```
//! 没写到的表/字段保持模板原样——术语包是"覆盖层"，不是重建。

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::template::Template;
use crate::tx;
use crate::Result;

/// 术语包当前格式版本。
pub const TERMS_SCHEMA: &str = "cercis.terms/1";

/// 一个字段的标签覆盖。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TermField {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_cn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_en: Option<String>,
}

/// 一张表的标签覆盖（含字段级覆盖）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TermTable {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_cn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_en: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, TermField>,
}

/// 术语包。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Terms {
    #[serde(default = "default_schema")]
    pub schema: String,
    /// 术语档：generic / agriculture / medicine / engineering / ecology …
    #[serde(default)]
    pub profile: String,
    /// 术语包自己的显示名（可选，便于界面展示"当前用的是哪一套"）
    #[serde(default)]
    pub label_cn: String,
    #[serde(default)]
    pub label_en: String,
    /// 表级 / 字段级标签覆盖
    #[serde(default)]
    pub tables: BTreeMap<String, TermTable>,
}

fn default_schema() -> String {
    TERMS_SCHEMA.to_string()
}

impl Default for Terms {
    fn default() -> Self {
        Self {
            schema: default_schema(),
            profile: "generic".into(),
            label_cn: String::new(),
            label_en: String::new(),
            tables: BTreeMap::new(),
        }
    }
}

impl Terms {
    pub fn from_yaml(text: &str) -> Result<Self> {
        let t: Terms = serde_yaml::from_str(text)?;
        t.validate()?;
        Ok(t)
    }

    /// 载入 `<项目>/terms.yaml`；没有这个文件就返回 None（表示"用模板原样"）。
    pub fn load(root: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = root.as_ref().join("terms.yaml");
        match tx::read_optional(&path)? {
            Some(text) => Ok(Some(Self::from_yaml(&text)?)),
            None => Ok(None),
        }
    }

    /// 写入 `<项目>/terms.yaml`。
    pub fn save(&self, root: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        tx::write_atomic_str(&root.as_ref().join("terms.yaml"), &serde_yaml::to_string(self)?)
    }

    fn validate(&self) -> Result<()> {
        if self.schema != TERMS_SCHEMA {
            return Err(crate::CercisError::Invalid(format!(
                "术语包格式版本不对：需要 {TERMS_SCHEMA}，实际 {}",
                self.schema
            )));
        }
        if self.profile.trim().is_empty() {
            return Err(crate::CercisError::Invalid(
                "术语包缺少 profile（如 agriculture / medicine）".into(),
            ));
        }
        Ok(())
    }

    /// 生效的覆盖条数（表 + 字段），用于界面提示"这套术语包改了多少东西"。
    pub fn override_count(&self) -> usize {
        self.tables
            .values()
            .map(|t| {
                let table_level = usize::from(t.label_cn.is_some() || t.label_en.is_some());
                table_level + t.fields.len()
            })
            .sum()
    }

    /// 把术语覆盖**只应用到标签上**：表名、列名、类型、引用、必填全都不动。
    ///
    /// 找不到对应的表/字段时**静默忽略**——术语包可能为多个模板准备，
    /// 与当前模板不完全重合是正常的。
    pub fn apply_to(&self, template: &mut Template) {
        for table in &mut template.tables {
            let Some(over) = self.tables.get(&table.name) else {
                continue;
            };
            if let Some(cn) = &over.label_cn {
                table.label_cn = cn.clone();
            }
            if let Some(en) = &over.label_en {
                table.label_en = en.clone();
            }
            for field in &mut table.fields {
                if let Some(f) = over.fields.get(&field.name) {
                    if let Some(cn) = &f.label_cn {
                        field.label_cn = cn.clone();
                    }
                    if let Some(en) = &f.label_en {
                        field.label_en = en.clone();
                    }
                }
            }
        }
        template.terms_profile = self.profile.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACK: &str = r#"
schema: cercis.terms/1
profile: agriculture
label_cn: 农业术语包
tables:
  units:
    label_cn: 小区
    label_en: Plot
    fields:
      unit_id:
        label_cn: 小区编号
"#;

    #[test]
    fn applies_labels_without_touching_structure() {
        let terms = Terms::from_yaml(PACK).unwrap();
        let mut t = Template::example_study();
        let before_cols: Vec<String> = t
            .tables
            .iter()
            .flat_map(|tb| tb.fields.iter().map(|f| f.name.clone()))
            .collect();

        terms.apply_to(&mut t);

        let units = t.table("units").unwrap();
        assert_eq!(units.label_cn, "小区");
        assert_eq!(units.fields[0].label_cn, "小区编号");
        // 列名与类型一点没变
        let after_cols: Vec<String> = t
            .tables
            .iter()
            .flat_map(|tb| tb.fields.iter().map(|f| f.name.clone()))
            .collect();
        assert_eq!(before_cols, after_cols);
        assert_eq!(units.fields[0].field_type, "text");
        assert_eq!(t.terms_profile, "agriculture");
    }

    #[test]
    fn keeps_template_labels_for_tables_not_covered() {
        let terms = Terms::from_yaml(PACK).unwrap();
        let mut t = Template::example_study();
        terms.apply_to(&mut t);
        // 术语包没写 studies，保持原样
        assert_eq!(t.table("studies").unwrap().label_cn, "研究");
    }

    #[test]
    fn rejects_wrong_schema() {
        let bad = PACK.replace("cercis.terms/1", "cercis.terms/9");
        assert!(Terms::from_yaml(&bad).is_err());
    }

    #[test]
    fn counts_overrides() {
        let terms = Terms::from_yaml(PACK).unwrap();
        assert_eq!(terms.override_count(), 2); // 表标签 1 + 字段标签 1
    }
}
