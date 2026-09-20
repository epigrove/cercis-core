//! 机器清单（`_labnote/manifest.json`）：AI 与外部工具进入项目的入口。
//!
//! ## 为什么需要它
//! AI 冷启动时最想知道的是：**这个项目里有什么、结构如何、数据在哪、有多大**。
//! 让它扫描全部 CSV 既慢又易错；先读清单即可。
//!
//! ## 内容
//! - 项目元信息（名称、结构版本、内核版本）
//! - 每张表：列名、行数、内容指纹、最后更新时间
//! - 模板与术语信息
//! - 变更水位（最后一条流水编号/时间），用于计算增量与打包

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::history::ObjectStore;
use crate::tx;
use crate::{CercisError, Result};

/// 清单中的一个表条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableEntry {
    pub name: String,
    pub columns: Vec<String>,
    pub rows: usize,
    pub sha256: String,
    pub bytes: u64,
}

/// 项目清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// 清单格式版本
    pub schema: String,
    pub project_name: String,
    pub project_id: String,
    pub schema_version: u32,
    pub core_version: String,
    /// 生成时间
    pub generated_at: String,
    /// 模板名（若有）
    #[serde(default)]
    pub template: String,
    /// 术语包
    #[serde(default)]
    pub terms_profile: String,
    /// 表清单
    pub tables: Vec<TableEntry>,
    /// 变更水位数（流水条数）
    #[serde(default)]
    pub change_count: usize,
    /// 最后一条流水时间
    #[serde(default)]
    pub last_change_at: String,
}

impl Manifest {
    /// 生成清单（扫描 tables/*.csv）。
    pub fn generate(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref();
        let mut entries = Vec::new();

        let tables_dir = root.join("tables");
        if tables_dir.exists() {
            let mut names: Vec<String> = Vec::new();
            for e in std::fs::read_dir(&tables_dir)? {
                let e = e?;
                let p = e.path();
                if p.extension().map(|x| x == "csv").unwrap_or(false) {
                    if let Some(stem) = p.file_stem() {
                        names.push(stem.to_string_lossy().to_string());
                    }
                }
            }
            names.sort();

            for name in names {
                let p = tables_dir.join(format!("{name}.csv"));
                let text = std::fs::read_to_string(&p)?;
                let table = crate::table::Table::load(&p)?;
                entries.push(TableEntry {
                    name,
                    columns: table.columns.clone(),
                    rows: table.rows.len(),
                    sha256: ObjectStore::hash_bytes(text.as_bytes()),
                    bytes: std::fs::metadata(&p)?.len(),
                });
            }
        }

        // 项目元信息
        let cfg_text = tx::read_optional(&root.join("project.yaml"))?
            .ok_or_else(|| CercisError::NotFound("不是 Cercis 项目（缺少 project.yaml）".into()))?;
        let cfg = crate::config::ProjectConfig::from_yaml(&cfg_text)?;

        // 模板名
        let template = match tx::read_optional(&root.join("template.yaml"))? {
            Some(t) => crate::template::Template::from_yaml(&t)?.name,
            None => String::new(),
        };

        // 变更水位
        let (change_count, last_change_at) = read_change_watermark(root)?;

        Ok(Manifest {
            schema: "cercis.manifest/1".into(),
            project_name: cfg.name.clone(),
            project_id: cfg.id.clone(),
            schema_version: cfg.schema_version,
            core_version: crate::CORE_VERSION.to_string(),
            generated_at: crate::ids::now_iso8601(),
            template,
            terms_profile: cfg.terms_profile.clone(),
            tables: entries,
            change_count,
            last_change_at,
        })
    }

    /// 写入 `_labnote/manifest.json`。
    pub fn write(&self, root: impl AsRef<Path>) -> Result<()> {
        let p = root.as_ref().join("_labnote").join("manifest.json");
        tx::write_atomic_str(&p, &serde_json::to_string_pretty(self)?)
    }

    /// 读取已有清单。
    pub fn load(root: impl AsRef<Path>) -> Result<Self> {
        let p = root.as_ref().join("_labnote").join("manifest.json");
        let text = tx::read_optional(&p)?
            .ok_or_else(|| CercisError::NotFound("清单不存在；请先生成".into()))?;
        Ok(serde_json::from_str(&text)?)
    }
}

fn read_change_watermark(root: &Path) -> Result<(usize, String)> {
    let p = root.join("tables").join("change_log.csv");
    if !p.exists() {
        return Ok((0, String::new()));
    }
    let table = crate::table::Table::load(&p)?;
    let last = table
        .rows
        .last()
        .and_then(|r| r.get("ts").cloned())
        .unwrap_or_default();
    Ok((table.rows.len(), last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Identity;

    #[test]
    fn generates_manifest_with_tables() {
        let dir = std::env::temp_dir().join("cercis-manifest-test");
        let _ = std::fs::remove_dir_all(&dir);
        let p = crate::Project::create(&dir, "示例", Identity::default()).unwrap();
        let m = Manifest::generate(p.root()).unwrap();
        assert_eq!(m.schema, "cercis.manifest/1");
        assert!(m.tables.iter().any(|t| t.name == "records"));
        assert!(m.change_count >= 1);
        m.write(p.root()).unwrap();
        let back = Manifest::load(p.root()).unwrap();
        assert_eq!(back.project_name, "示例");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
