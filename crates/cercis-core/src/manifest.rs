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
    /// 这张表在加密区域里（磁盘上是密文）
    #[serde(default)]
    pub encrypted: bool,
    /// 在加密区域里、但当前没解锁（列与行数读不到）
    #[serde(default)]
    pub locked: bool,
}

/// 一个加密区域在清单里的样子（**未解锁时只报个数，不暴露名字**）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneSummary {
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub generation: u32,
    pub key_fingerprint: String,
    pub files: usize,
    /// 当前会话成员在该区域的权限（未解锁时为空）
    #[serde(default)]
    pub permission: String,
}

/// 加密共享状态摘要（清单用）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AccessSummary {
    pub members: usize,
    /// 已解锁的成员（未解锁为空）
    #[serde(default)]
    pub unlocked_as: String,
    /// 磁盘上共有多少个加密区域
    pub zones_total: usize,
    /// 未解锁时看不到区域名字，只报个数
    #[serde(default)]
    pub zones_locked: usize,
    #[serde(default)]
    pub zones: Vec<ZoneSummary>,
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
    /// **项目状态指纹**（D39）：模板 + 术语包 + 插件台账 + 停用名单 + 信任记录 的内容指纹。
    ///
    /// 为什么要有：表指纹只能说明"数据是这一版"，说明不了"项目状态是这一版"——
    /// 换了术语包、装了插件、改了信任，导出物与推理结论都会变。
    /// 导出清单引用这个指纹，就能把"数据 + 结构 + 插件状态"一起对回去。
    #[serde(default)]
    pub state_hash: String,
    /// 表清单
    pub tables: Vec<TableEntry>,
    /// 加密共享状态（默认空 = 本项目没有加密区域）
    #[serde(default)]
    pub access: AccessSummary,
    /// 变更水位数（流水条数）
    #[serde(default)]
    pub change_count: usize,
    /// 最后一条流水时间
    #[serde(default)]
    pub last_change_at: String,
}

impl Manifest {
    /// 一个"没有具体项目"的清单：给**随包交付的通用 Agent 技能包**用。
    ///
    /// 技能包按契约生成（§11.5），而契约需要模板 + 清单；用户装完 app 但还没有项目时，
    /// 就用这份通用清单（空表、通用模板）生成一份"任何项目都能用"的技能包。
    pub fn generic() -> Self {
        Self {
            schema: "cercis.manifest/1".into(),
            project_name: "Cercis（通用）".into(),
            project_id: String::new(),
            schema_version: 1,
            core_version: crate::CORE_VERSION.to_string(),
            generated_at: String::new(),
            template: "generic".into(),
            terms_profile: "generic".into(),
            state_hash: String::new(),
            tables: Vec::new(),
            access: AccessSummary::default(),
            change_count: 0,
            last_change_at: String::new(),
        }
    }

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
                let file_name = p.file_name().map(|s| s.to_string_lossy().to_string());
                let Some(file_name) = file_name else { continue };
                // 普通表：xxx.csv；加密区域里的表：xxx.csv.enc
                if let Some(stem) = file_name.strip_suffix(".csv") {
                    names.push(stem.to_string());
                } else if let Some(stem) = file_name.strip_suffix(".csv.enc") {
                    names.push(stem.to_string());
                } else if file_name.ends_with(crate::tx::TMP_SUFFIX) {
                    continue;
                }
            }
            names.sort();
            names.dedup();

            for name in names {
                let rel = format!("tables/{name}.csv");
                let p = root.join(&rel);
                let encrypted = crate::access::zone_covering(root, &rel)?.is_some();
                // 加密区域里的表：走透明解密（未解锁就只报"锁着"）
                let stored = match crate::access::read_through_zone(root, None, &rel) {
                    Ok(Some(bytes)) => Some(bytes),
                    Ok(None) => std::fs::read(&p).ok(),
                    Err(_) => None,
                };
                let Some(stored) = stored else {
                    entries.push(TableEntry {
                        name,
                        columns: Vec::new(),
                        rows: 0,
                        sha256: String::new(),
                        bytes: std::fs::metadata(crate::access::enc_path(&p))
                            .map(|m| m.len())
                            .unwrap_or(0),
                        encrypted: true,
                        locked: true,
                    });
                    continue;
                };
                let text = String::from_utf8(stored.clone()).unwrap_or_default();
                match crate::table::Table::from_csv_text(&p, &text) {
                    Ok(table) => {
                        entries.push(TableEntry {
                            name,
                            columns: table.columns.clone(),
                            rows: table.rows.len(),
                            sha256: ObjectStore::hash_bytes(&stored),
                            bytes: stored.len() as u64,
                            encrypted,
                            locked: false,
                        });
                    }
                    Err(_) => {
                        entries.push(TableEntry {
                            name,
                            columns: Vec::new(),
                            rows: 0,
                            sha256: String::new(),
                            bytes: stored.len() as u64,
                            encrypted,
                            locked: true,
                        });
                    }
                }
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
            state_hash: state_hash(root),
            tables: entries,
            access: access_summary(root)?,
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
    let rel = "tables/change_log.csv";
    let p = root.join(rel);
    // 流水表本身也可能在加密区域里：走透明解密；没解锁就报"读不到"而不是报错
    let bytes = match crate::access::read_through_zone(root, None, rel) {
        Ok(Some(b)) => b,
        Ok(None) => match std::fs::read(&p) {
            Ok(b) => b,
            Err(_) => return Ok((0, String::new())),
        },
        Err(_) => return Ok((0, String::new())),
    };
    let text = String::from_utf8_lossy(&bytes).to_string();
    let table = crate::table::Table::from_csv_text(&p, &text)?;
    let last = table
        .rows
        .last()
        .and_then(|r| r.get("ts").cloned())
        .unwrap_or_default();
    Ok((table.rows.len(), last))
}

/// 加密共享摘要（未解锁时**不暴露区域名**，只报个数）。
fn access_summary(root: &Path) -> Result<AccessSummary> {
    let zones = crate::access::Zones::load(root)?;
    if zones.zones.is_empty() {
        return Ok(AccessSummary::default());
    }
    let members = crate::access::Members::load(root)?.members.len();
    let session = crate::access::resolve_session(root, None)?;
    let mut out = AccessSummary {
        members,
        unlocked_as: session
            .as_ref()
            .map(|s| s.member.clone())
            .unwrap_or_default(),
        zones_total: zones.zones.len(),
        zones_locked: 0,
        zones: Vec::new(),
    };
    for z in &zones.zones {
        let permission = session
            .as_ref()
            .and_then(|s| s.zone(&z.name))
            .map(|u| u.permission.as_str().to_string());
        let Some(permission) = permission else {
            out.zones_locked += 1;
            continue;
        };
        let files = crate::access::zone_files(root, z)
            .into_iter()
            .filter(|rel| crate::access::enc_path(&root.join(rel)).exists())
            .count();
        out.zones.push(ZoneSummary {
            name: z.name.clone(),
            label: z.label.clone(),
            generation: z.generation,
            key_fingerprint: z.key_fingerprint.clone(),
            files,
            permission,
        });
    }
    Ok(out)
}

/// 项目状态指纹（D39）：把"非数据表"的项目状态一起算进去。
///
/// 覆盖：`template.yaml`、`terms.yaml`、插件台账、停用名单、信任记录。
/// 缺哪个就跳过哪个（老项目没有这些文件也应该能生成清单）。
fn state_hash(root: &Path) -> String {
    use sha2::{Digest, Sha256};
    const PARTS: [&str; 7] = [
        "template.yaml",
        "terms.yaml",
        crate::registry::REGISTRY_FILE,
        crate::plugin::STATE_FILE,
        "plugins/trust.yaml",
        crate::access::ZONES_REL,
        crate::access::MEMBERS_REL,
    ];
    let mut hasher = Sha256::new();
    for part in PARTS {
        let path = root.join(part);
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        // 文件名一起进摘要，免得"有模板没术语"与"有术语没模板"撞成同一个指纹
        hasher.update(part.as_bytes());
        hasher.update([0u8]);
        hasher.update(&bytes);
    }
    hex::encode(hasher.finalize())
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

    /// 通用清单：给"还没建项目，但想先把 AI 技能包装上"的用户用。
    #[test]
    fn generic_manifest_has_no_project_specifics() {
        let m = Manifest::generic();
        assert_eq!(m.schema, "cercis.manifest/1");
        assert_eq!(m.template, "generic");
        assert!(m.tables.is_empty(), "通用清单里不该有具体表");
        assert!(m.project_id.is_empty());
        assert_eq!(m.core_version, crate::CORE_VERSION);
        // 能被序列化/反序列化（技能包生成时会走 JSON）
        let json = serde_json::to_string(&m).unwrap();
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.template, "generic");
    }
}
