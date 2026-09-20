//! 项目配置（`project.yaml`）：编号、名称、语言、术语、分享策略、身份开关。
//!
//! 配置里**不出现任何学科专有词**——术语映射由插件（模板）提供。

use serde::{Deserialize, Serialize};

use crate::{CercisError, Result};

/// 语言设置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    Zh,
    En,
}

/// 身份相关开关。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdentityConfig {
    /// 是否记录录入设备（个人编辑默认关闭）。
    #[serde(default)]
    pub record_device: bool,
    /// 当前用户（显示名）。
    #[serde(default)]
    pub display_name: String,
}

/// 插件相关配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    /// 项目内插件是否只允许管理员编辑（NAS 多人场景）。
    #[serde(default)]
    pub admin_only_edit: bool,
    /// 加载范围：global / project / both。
    #[serde(default = "default_load_scope")]
    pub load_scope: String,
}

fn default_load_scope() -> String {
    "both".to_string()
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            admin_only_edit: false,
            load_scope: default_load_scope(),
        }
    }
}

/// 历史策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryConfig {
    /// 检查点保留数量（默认最近 50 个）。
    #[serde(default = "default_keep")]
    pub checkpoints_keep: usize,
    /// 逐次存档默认全保留；超过该体积（字节）时提示压缩。
    #[serde(default = "default_compress_hint")]
    pub objects_compress_hint_bytes: u64,
}

fn default_keep() -> usize {
    50
}
fn default_compress_hint() -> u64 {
    2 * 1024 * 1024 * 1024 // 2 GB
}

impl Default for HistoryConfig {
    fn default() -> Self {
        Self {
            checkpoints_keep: default_keep(),
            objects_compress_hint_bytes: default_compress_hint(),
        }
    }
}

/// 媒体策略。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaConfig {
    /// 项目媒体总量超过该值（字节）时提示（默认 512 MB）；不限制导入。
    #[serde(default = "default_media_hint")]
    pub warning_bytes: u64,
    /// 默认导入（复制进项目）还是引用原位置。
    #[serde(default = "default_media_mode")]
    pub default_mode: String,
}

fn default_media_hint() -> u64 {
    512 * 1024 * 1024
}
fn default_media_mode() -> String {
    "imported".to_string()
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self {
            warning_bytes: default_media_hint(),
            default_mode: default_media_mode(),
        }
    }
}

/// 项目配置全貌。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub schema_version: u32,
    pub name: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub terms_profile: String,
    #[serde(default)]
    pub identity: IdentityConfig,
    #[serde(default)]
    pub plugins: PluginConfig,
    #[serde(default)]
    pub history: HistoryConfig,
    #[serde(default)]
    pub media: MediaConfig,
}

impl ProjectConfig {
    pub fn new(name: &str) -> Self {
        Self {
            schema_version: crate::SCHEMA_VERSION,
            name: name.to_string(),
            id: String::new(),
            language: Language::default(),
            terms_profile: "generic".to_string(),
            identity: IdentityConfig::default(),
            plugins: PluginConfig::default(),
            history: HistoryConfig::default(),
            media: MediaConfig::default(),
        }
    }

    /// 从 YAML 文本解析，并做基本校验。
    pub fn from_yaml(text: &str) -> Result<Self> {
        let cfg: ProjectConfig = serde_yaml::from_str(text)?;
        if cfg.name.trim().is_empty() {
            return Err(CercisError::Invalid("项目名称不能为空".into()));
        }
        if cfg.schema_version > crate::SCHEMA_VERSION {
            return Err(CercisError::Invalid(format!(
                "项目结构版本 {} 高于本内核支持的 {}；请升级内核",
                cfg.schema_version,
                crate::SCHEMA_VERSION
            )));
        }
        Ok(cfg)
    }

    pub fn to_yaml(&self) -> Result<String> {
        Ok(serde_yaml::to_string(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrip() {
        let c = ProjectConfig::new("示例项目");
        let y = c.to_yaml().unwrap();
        let back = ProjectConfig::from_yaml(&y).unwrap();
        assert_eq!(back.name, "示例项目");
        assert_eq!(back.history.checkpoints_keep, 50);
        assert_eq!(back.media.warning_bytes, 512 * 1024 * 1024);
    }

    #[test]
    fn rejects_future_schema() {
        let y = "schema_version: 99\nname: x\n";
        assert!(ProjectConfig::from_yaml(y).is_err());
    }
}
