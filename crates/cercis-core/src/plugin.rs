//! 插件协议：插件清单、权限模型、界面扩展点声明。
//!
//! ## 分层（文档 §14.4 / §14.6 / §14.7）
//! - **核心能力**：与内核交互、完成任务（只通过内核接口读写数据）
//! - **形态适配**：与 app 交互、把功能呈现给用户（桌面 / Pad / 手机各一套）
//!
//! 本模块只负责"**描述与校验**"：插件声明自己是谁、要什么权限、在界面里插在哪。
//! 真正的执行在插件进程/脚本里，由内核提供调用通道（CLI / API）。
//!
//! ## 安全规则（硬性）
//! 1. 默认只读：需要写入必须在清单里声明 `write`
//! 2. 需要网络必须声明 `network`
//! 3. 写入必须走内核通道（插件不得直接改 CSV）
//! 4. 项目内插件默认**不加载**，需用户信任（信任绑定内容指纹）

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::tx;
use crate::{CercisError, Result};

/// 插件所属层。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginTier {
    /// 基础工具包（人人可用的基础功能）
    Basic,
    /// 科研工具包（学科相关功能）
    Research,
    /// app 插件（各平台界面）
    App,
    /// 第三方插件
    ThirdParty,
}

/// 权限声明。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Permissions {
    /// 读取项目数据
    #[serde(default)]
    pub read: bool,
    /// 写入项目数据（必须经内核通道）
    #[serde(default)]
    pub write: bool,
    /// 访问网络（NAS、下载等）
    #[serde(default)]
    pub network: bool,
}

/// 支持的形态（对应 §14.6 的形态分级）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Surfaces {
    /// 桌面图形界面
    #[serde(default)]
    pub desktop_gui: bool,
    /// 桌面命令行
    #[serde(default)]
    pub desktop_cli: bool,
    /// Pad
    #[serde(default)]
    pub tablet: bool,
    /// 手机
    #[serde(default)]
    pub phone: bool,
}

/// 界面扩展点声明（Zotero 式：插件可改变界面）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiExtension {
    /// 扩展点类型：panel / menu / column / view / detail_block / settings / command / export_format / theme
    pub kind: String,
    /// 唯一 ID（用于冲突登记）
    pub id: String,
    /// 显示名（中）
    pub label_cn: String,
    /// 显示名（英）
    #[serde(default)]
    pub label_en: String,
    /// 插入位置提示（可选）
    #[serde(default)]
    pub placement: String,
    /// `command` 扩展点专用：调用插件入口时传的参数（如 `data import`）。
    /// 留空时退回用 `id`（把点号换成空格），因此老插件不需要改。
    #[serde(default)]
    pub command: String,
}

impl UiExtension {
    fn valid_kind(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "panel"
                | "menu"
                | "column"
                | "view"
                | "detail_block"
                | "settings"
                | "command"
                | "export_format"
                | "theme"
        )
    }

    /// 调用入口时实际使用的参数文本。
    pub fn invocation(&self) -> String {
        if !self.command.trim().is_empty() {
            return self.command.trim().to_string();
        }
        // 约定：扩展点 id 用点分层级（`data.import`），退化成子命令（`data import`）
        self.id.trim().replace('.', " ")
    }
}

/// 插件清单：`plugin.yaml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// 插件 ID（唯一，小写 ASCII 下划线）
    pub id: String,
    /// 显示名
    pub name: String,
    #[serde(default)]
    pub label_cn: String,
    /// 版本
    #[serde(default)]
    pub version: String,
    /// 所属层
    #[serde(default = "default_tier")]
    pub tier: PluginTier,
    /// 要求的插件接口版本（内核会检查兼容性）
    #[serde(default)]
    pub requires_plugin_api: u32,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub surfaces: Surfaces,
    /// 界面扩展点声明
    #[serde(default)]
    pub ui: Vec<UiExtension>,
    /// 入口提示（如何调用：命令名/可执行文件/脚本）
    #[serde(default)]
    pub entrypoint: String,
    /// 说明
    #[serde(default)]
    pub description: String,
}

fn default_tier() -> PluginTier {
    PluginTier::ThirdParty
}

/// 当前内核支持的插件接口版本。
pub const PLUGIN_API_VERSION: u32 = 1;

impl PluginManifest {
    /// 从 YAML 解析并校验。
    pub fn from_yaml(text: &str) -> Result<Self> {
        let m: PluginManifest = serde_yaml::from_str(text)?;
        m.validate()?;
        Ok(m)
    }

    /// 从文件载入。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = tx::read_optional(path.as_ref())?.ok_or_else(|| {
            CercisError::NotFound(format!("插件清单不存在: {}", path.as_ref().display()))
        })?;
        Self::from_yaml(&text)
    }

    /// 校验：ID 规范、接口版本兼容、扩展点合法、权限自洽。
    pub fn validate(&self) -> Result<()> {
        if !crate::format::is_ascii_snake(&self.id) {
            return Err(CercisError::Invalid(format!(
                "插件 ID 需为小写 ASCII 下划线形式: {:?}",
                self.id
            )));
        }
        if self.requires_plugin_api > PLUGIN_API_VERSION {
            return Err(CercisError::Invalid(format!(
                "插件 {} 需要接口版本 {}，但内核只支持到 {}；请升级内核",
                self.id, self.requires_plugin_api, PLUGIN_API_VERSION
            )));
        }
        // 写入权限必须同时具备读取权限（否则无意义）
        if self.permissions.write && !self.permissions.read {
            return Err(CercisError::Invalid(format!(
                "插件 {} 声明了写入但没有读取权限",
                self.id
            )));
        }
        let mut ids = std::collections::BTreeSet::new();
        for u in &self.ui {
            if !u.valid_kind() {
                return Err(CercisError::Invalid(format!(
                    "插件 {} 的扩展点类型未知: {}（可用：panel/menu/column/view/detail_block/settings/command/export_format/theme）",
                    self.id, u.kind
                )));
            }
            if u.id.trim().is_empty() {
                return Err(CercisError::Invalid(format!(
                    "插件 {} 的扩展点必须有 id",
                    self.id
                )));
            }
            if !ids.insert(u.id.clone()) {
                return Err(CercisError::Invalid(format!(
                    "插件 {} 的扩展点 id 重复: {}",
                    self.id, u.id
                )));
            }
        }
        Ok(())
    }

    /// 是否声明了某种形态。
    pub fn supports(&self, surface: &str) -> bool {
        match surface {
            "desktop-gui" => self.surfaces.desktop_gui,
            "desktop-cli" => self.surfaces.desktop_cli,
            "tablet" => self.surfaces.tablet,
            "phone" => self.surfaces.phone,
            _ => false,
        }
    }

    /// 权限的可读摘要（信任确认界面用）。
    pub fn permission_summary(&self) -> String {
        let mut parts = Vec::new();
        if self.permissions.read {
            parts.push("读取数据");
        }
        if self.permissions.write {
            parts.push("写入数据");
        }
        if self.permissions.network {
            parts.push("访问网络");
        }
        if parts.is_empty() {
            "无（仅界面）".to_string()
        } else {
            parts.join("、")
        }
    }
}

/// 一个已发现的插件（含来源与信任状态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredPlugin {
    pub manifest: PluginManifest,
    /// 来源：global / project
    pub origin: String,
    /// 插件目录
    pub path: PathBuf,
    /// 内容指纹（信任绑定用）
    pub fingerprint: String,
    /// 是否已被信任（项目内插件默认 false）
    pub trusted: bool,
}

/// 发现插件：扫描全局目录与项目内 `plugins/`。
///
/// 说明：项目内插件默认**不加载**；这里只做"发现 + 计算指纹"，
/// 是否启用由 app 的信任界面决定（信任记录在 `<项目>/plugins/trust.yaml`）。
pub fn discover(
    project_root: impl AsRef<Path>,
    global_dir: Option<&Path>,
) -> Result<Vec<DiscoveredPlugin>> {
    let mut found = Vec::new();

    if let Some(g) = global_dir {
        scan_dir(g, "global", &mut found)?;
    }
    let project_plugins = project_root.as_ref().join("plugins");
    scan_dir(&project_plugins, "project", &mut found)?;

    Ok(found)
}

fn scan_dir(dir: &Path, origin: &str, out: &mut Vec<DiscoveredPlugin>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let manifest_path = path.join("plugin.yaml");
        if !manifest_path.exists() {
            continue;
        }
        let manifest = PluginManifest::load(&manifest_path)?;
        // 计算整个插件目录的指纹（清单 + 代码）
        let fingerprint = fingerprint_dir(&path)?;
        out.push(DiscoveredPlugin {
            manifest,
            origin: origin.to_string(),
            path: path.clone(),
            fingerprint,
            trusted: false,
        });
    }
    Ok(())
}

/// 计算目录内容指纹（稳定：按相对路径排序后逐个哈希）。
pub fn fingerprint_dir(dir: &Path) -> Result<String> {
    let mut files: Vec<PathBuf> = Vec::new();
    collect_files(dir, &mut files)?;
    files.sort();

    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for f in &files {
        let rel = f.strip_prefix(dir).unwrap_or(f);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        let bytes = std::fs::read(f)?;
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            collect_files(&p, out)?;
        } else {
            out.push(p);
        }
    }
    Ok(())
}

/// 信任记录：`plugins/trust.yaml`。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TrustRecord {
    /// 插件 ID → 被信任的指纹
    #[serde(default)]
    pub trusted: std::collections::BTreeMap<String, String>,
}

impl TrustRecord {
    pub fn load(project_root: impl AsRef<Path>) -> Result<Self> {
        let p = project_root.as_ref().join("plugins").join("trust.yaml");
        match tx::read_optional(&p)? {
            Some(t) => Ok(serde_yaml::from_str(&t)?),
            None => Ok(Self::default()),
        }
    }

    pub fn save(&self, project_root: impl AsRef<Path>) -> Result<()> {
        let p = project_root.as_ref().join("plugins").join("trust.yaml");
        tx::write_atomic_str(&p, &serde_yaml::to_string(self)?)
    }

    /// 该插件是否被信任（**内容必须与记录一致**）。
    pub fn is_trusted(&self, id: &str, fingerprint: &str) -> bool {
        self.trusted
            .get(id)
            .map(|f| f == fingerprint)
            .unwrap_or(false)
    }

    /// 记录信任。
    pub fn trust(&mut self, id: &str, fingerprint: &str) {
        self.trusted.insert(id.to_string(), fingerprint.to_string());
    }

    /// 撤销信任。
    pub fn untrust(&mut self, id: &str) {
        self.trusted.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
id: export_spss
name: SPSS Export
label_cn: SPSS 导出
version: 0.1.0
tier: research
requires_plugin_api: 1
permissions:
  read: true
  write: true
  network: false
surfaces:
  desktop_gui: true
  desktop_cli: true
ui:
  - kind: menu
    id: export.spss
    label_cn: 导出 SPSS 文件
    label_en: Export SPSS
entrypoint: cercis-spss
description: 导出 SPSS 语法与数据文件
"#;

    #[test]
    fn parses_and_validates_manifest() {
        let m = PluginManifest::from_yaml(SAMPLE).unwrap();
        assert_eq!(m.id, "export_spss");
        assert!(m.supports("desktop-gui"));
        assert!(!m.supports("phone"));
        assert!(m.permission_summary().contains("写入数据"));
    }

    #[test]
    fn rejects_write_without_read() {
        let bad = SAMPLE.replace(
            "  read: true\n  write: true",
            "  read: false\n  write: true",
        );
        assert!(PluginManifest::from_yaml(&bad).is_err());
    }

    #[test]
    fn rejects_unknown_ui_kind() {
        let bad = SAMPLE.replace("kind: menu", "kind: super_menu");
        assert!(PluginManifest::from_yaml(&bad).is_err());
    }

    #[test]
    fn trust_requires_matching_fingerprint() {
        let mut tr = TrustRecord::default();
        tr.trust("export_spss", "abc");
        assert!(tr.is_trusted("export_spss", "abc"));
        assert!(!tr.is_trusted("export_spss", "def"), "内容变化后信任失效");
        tr.untrust("export_spss");
        assert!(!tr.is_trusted("export_spss", "abc"));
    }

    #[test]
    fn command_extension_carries_invocation() {
        let m = PluginManifest::from_yaml(SAMPLE).unwrap();
        let export = m.ui.iter().find(|u| u.kind == "menu").unwrap();
        // 未写 command 时，用扩展点 id 退化（export.spss → "export spss"）
        assert_eq!(export.invocation(), "export spss");

        let with_command = SAMPLE.replace(
            "  - kind: menu\n    id: export.spss",
            "  - kind: command\n    id: data.import\n    command: data import\n    label_cn: 导入数据\n    label_en: Import data\n  - kind: menu\n    id: export.spss",
        );
        let m2 = PluginManifest::from_yaml(&with_command).unwrap();
        let import = m2.ui.iter().find(|u| u.id == "data.import").unwrap();
        assert_eq!(import.invocation(), "data import");
    }

    #[test]
    fn fingerprint_changes_with_content() {
        let dir = std::env::temp_dir().join("cercis-fp-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.yaml"), "id: a").unwrap();
        let f1 = fingerprint_dir(&dir).unwrap();
        std::fs::write(dir.join("plugin.yaml"), "id: b").unwrap();
        let f2 = fingerprint_dir(&dir).unwrap();
        assert_ne!(f1, f2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
