//! 插件市场清单（D41）：**清单是一份文件，下载在 app，安装落盘与记账在内核**。
//!
//! 分工（需求文档 §14.8）：
//! - ① 清单：托管在 git 仓库里的 `marketplace.json`（默认预置指向 GitHub **公开**仓库；
//!   也允许添加私有仓库，鉴权在 app 侧做）
//! - ② 看清单 / 下载 / 更新检查：app
//! - ③ 装卸与屏蔽的界面：基础工具包里的"插件管理器"
//! - ④ **协议、校验、安装落盘、来源与指纹记录：内核**（本模块）
//!
//! 所以本模块**不联网**：它做的是"给我一个已经下载好的目录/文件，我校验它、装它、记账"。
//! 这样三条路都通：app 下载后调内核、脚本手动装、断网时拖入插件包（FR-P48）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::plugin::{self, PluginManifest, PLUGIN_API_VERSION};
use crate::tx;
use crate::{CercisError, Result};

/// 市场清单格式版本。
pub const MARKETPLACE_SCHEMA: &str = "cercis.marketplace/1";

/// 安装记录（**用户级**，不在项目里）：谁从哪装的、装的哪一版、指纹是多少。
///
/// 为什么必须记：FR-P46 要求"未点安装时磁盘上没有该插件代码；安装后能查出从哪装的、装的哪一版"；
/// 卸载（FR-P50）与更新（FR-P47）也都要靠这份记录才知道"这份代码是谁放进来的"。
pub const INSTALLS_FILE: &str = "installs.json";

/// 默认预置的市场（D41：指向 GitHub 公开仓库）。
///
/// 开发期这些仓库可能还是私有的——那时拉取会失败，属于预期（鉴权在 app 侧）。
pub const PRESET_MARKETS: [PresetMarket; 1] = [PresetMarket {
    id: "research",
    label_cn: "科研工具包（官方）",
    repository: "https://github.com/epigrove/cercis-kits-research",
    marketplace_path: "marketplace.json",
    default_ref: "main",
}];

/// 预置市场条目。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PresetMarket {
    pub id: &'static str,
    pub label_cn: &'static str,
    /// 仓库地址（公开仓库；私有仓库由用户自己添加）
    pub repository: &'static str,
    /// 清单在仓库里的相对路径
    pub marketplace_path: &'static str,
    /// 默认分支/标签
    pub default_ref: &'static str,
}

/// 市场清单里的一个插件条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketPlugin {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label_cn: String,
    /// 插件目录在仓库里的相对路径
    pub path: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub tier: String,
    #[serde(default)]
    pub requires_plugin_api: u32,
    #[serde(default)]
    pub permissions: String,
    #[serde(default)]
    pub summary: String,
    /// 前置条件（与 `plugin.yaml` 里的 `requires` 同义；市场里写一份便于提前提示）
    #[serde(default)]
    pub requires: Option<crate::plugin::Requires>,
}

/// 市场清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Marketplace {
    #[serde(default = "default_schema")]
    pub schema: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub label_cn: String,
    #[serde(default)]
    pub label_en: String,
    #[serde(default)]
    pub description: String,
    /// 清单所属仓库（公开仓库写在这里；私有仓库由用户添加时给地址）
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub plugins: Vec<MarketPlugin>,
}

fn default_schema() -> String {
    MARKETPLACE_SCHEMA.to_string()
}

impl Marketplace {
    /// 从 YAML/JSON 文本解析并校验（两种都吃：清单手写时习惯 YAML）。
    pub fn from_text(text: &str) -> Result<Self> {
        let m: Marketplace = if text.trim_start().starts_with('{') {
            serde_json::from_str(text)?
        } else {
            serde_yaml::from_str(text)?
        };
        m.validate()?;
        Ok(m)
    }

    /// 从文件载入。
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let text = tx::read_optional(path.as_ref())?.ok_or_else(|| {
            CercisError::NotFound(format!("市场清单不存在: {}", path.as_ref().display()))
        })?;
        Self::from_text(&text)
    }

    /// 校验：格式版本、条目齐全、接口版本兼容、路径安全、id 不重复。
    pub fn validate(&self) -> Result<()> {
        if self.schema.trim() != MARKETPLACE_SCHEMA {
            return Err(CercisError::Invalid(format!(
                "市场清单版本不认识：{}（本内核支持 {MARKETPLACE_SCHEMA}）",
                self.schema
            )));
        }
        if self.plugins.is_empty() {
            return Err(CercisError::Invalid(
                "市场清单里一个插件都没有（plugins 为空）".into(),
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for p in &self.plugins {
            if p.id.trim().is_empty() {
                return Err(CercisError::Invalid("市场里有条目没写 id".into()));
            }
            if !ids.insert(p.id.clone()) {
                return Err(CercisError::Invalid(format!(
                    "市场里插件 id 重复：{}",
                    p.id
                )));
            }
            if p.path.trim().is_empty() {
                return Err(CercisError::Invalid(format!(
                    "插件 {} 没写仓库内路径（path）",
                    p.id
                )));
            }
            // 路径必须是仓库内的相对路径：出去的、绝对的都拦下
            if p.path.starts_with('/') || p.path.contains("..") {
                return Err(CercisError::Invalid(format!(
                    "插件 {} 的路径必须是仓库内的相对路径，且不能含 `..`：{}",
                    p.id, p.path
                )));
            }
            if p.requires_plugin_api > PLUGIN_API_VERSION {
                return Err(CercisError::Invalid(format!(
                    "插件 {} 需要接口版本 {}，本内核只支持到 {}——它需要更新的内核",
                    p.id, p.requires_plugin_api, PLUGIN_API_VERSION
                )));
            }
            if p.version.trim().is_empty() {
                return Err(CercisError::Invalid(format!(
                    "插件 {} 没写版本（version）",
                    p.id
                )));
            }
        }
        Ok(())
    }

    /// 找条目。
    pub fn plugin(&self, id: &str) -> Option<&MarketPlugin> {
        self.plugins.iter().find(|p| p.id == id)
    }

    /// 清单摘要（app 里列表用）。
    pub fn summary(&self) -> String {
        format!(
            "{}（{} 个插件）",
            if self.label_cn.is_empty() {
                &self.name
            } else {
                &self.label_cn
            },
            self.plugins.len()
        )
    }
}

/// 一次安装的记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRecord {
    pub id: String,
    /// 从哪个市场装的（市场的 id 或地址）
    #[serde(default)]
    pub market: String,
    /// 市场清单所属仓库
    #[serde(default)]
    pub repository: String,
    /// 分支 / 标签 / 提交
    #[serde(default)]
    pub git_ref: String,
    #[serde(default)]
    pub version: String,
    /// 内容指纹（与信任绑定一致：内容一变就得重新信任）
    pub fingerprint: String,
    /// 装到哪（相对用户插件目录）
    pub path: String,
    pub installed_at: String,
}

/// 安装台账（用户级）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Installs {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub records: Vec<InstallRecord>,
}

impl Installs {
    /// 载入（默认位置：`<用户插件目录>/installs.json`）。
    pub fn load(global_dir: impl AsRef<Path>) -> Result<Self> {
        let path = global_dir.as_ref().join(INSTALLS_FILE);
        match tx::read_optional(&path)? {
            None => Ok(Self {
                schema: "cercis.installs/1".into(),
                records: Vec::new(),
            }),
            Some(text) => Ok(serde_json::from_str(&text)?),
        }
    }

    pub fn save(&self, global_dir: impl AsRef<Path>) -> Result<()> {
        let dir = global_dir.as_ref();
        std::fs::create_dir_all(dir)?;
        tx::write_atomic_str(
            &dir.join(INSTALLS_FILE),
            &serde_json::to_string_pretty(self)?,
        )
    }

    pub fn get(&self, id: &str) -> Option<&InstallRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// 记一条（同一个 id 覆盖旧的：更新即替换记录）。
    pub fn put(&mut self, record: InstallRecord) {
        self.records.retain(|r| r.id != record.id);
        self.records.push(record);
    }

    /// 删一条（卸载市场插件，FR-P50）。
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.records.len();
        self.records.retain(|r| r.id != id);
        self.records.len() != before
    }
}

/// 安装一个**已经下载好**的插件目录（内核不联网）。
///
/// 步骤：校验清单 → 内容指纹 → 复制到全局插件目录 → 记安装台账。
/// 返回（安装记录，安装后的目录）。
pub fn install_from_dir(
    source_dir: &Path,
    global_dir: &Path,
    market: &str,
    repository: &str,
    git_ref: &str,
) -> Result<(InstallRecord, PathBuf)> {
    if !source_dir.is_dir() {
        return Err(CercisError::NotFound(format!(
            "要安装的插件目录不存在：{}",
            source_dir.display()
        )));
    }
    // 清单先行：装一个连清单都不合法的插件没有意义（错误里带上路径，便于定位）
    let manifest_path = source_dir.join("plugin.yaml");
    let manifest = PluginManifest::load(&manifest_path).map_err(|e| {
        CercisError::Invalid(format!(
            "插件清单不合法（{}）：{e}",
            manifest_path.display()
        ))
    })?;
    let fingerprint = plugin::fingerprint_dir(source_dir)?;
    let dest = global_dir.join(&manifest.id);
    if dest.exists() {
        // 允许覆盖（更新），但要说清是在替换
        let existing = PluginManifest::load(dest.join("plugin.yaml")).ok();
        if let Some(old) = existing {
            if old.id != manifest.id {
                return Err(CercisError::Invalid(format!(
                    "目标目录里已经有一个不同的插件（{}），拒绝覆盖：{}",
                    old.id,
                    dest.display()
                )));
            }
        }
        std::fs::remove_dir_all(&dest)?;
    }
    std::fs::create_dir_all(&dest)?;
    copy_dir(source_dir, &dest)?;

    let record = InstallRecord {
        id: manifest.id.clone(),
        market: market.to_string(),
        repository: repository.to_string(),
        git_ref: git_ref.to_string(),
        version: manifest.version.clone(),
        fingerprint,
        path: manifest.id.clone(),
        installed_at: crate::ids::now_iso8601(),
    };
    let mut installs = Installs::load(global_dir)?;
    installs.put(record.clone());
    installs.save(global_dir)?;
    Ok((record, dest))
}

/// 卸载市场安装的插件（删本地副本 + 清安装记录；FR-P50）。
pub fn uninstall(global_dir: &Path, id: &str, dry_run: bool) -> Result<(bool, String)> {
    let mut installs = Installs::load(global_dir)?;
    let Some(record) = installs.get(id).cloned() else {
        return Err(CercisError::NotFound(format!(
            "没有 {id} 的安装记录——它可能不是从这个市场装的（随包插件不能物理删除，只能停用）"
        )));
    };
    let dir = global_dir.join(&record.path);
    if dry_run {
        return Ok((
            true,
            format!("会删掉 {}（{}，指纹 {}）", dir.display(), record.version, &record.fingerprint[..8.min(record.fingerprint.len())]),
        ));
    }
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    installs.remove(id);
    installs.save(global_dir)?;
    Ok((
        true,
        format!("已卸载 {id}（版本 {}，来自 {}）", record.version, record.repository),
    ))
}

/// 递归复制目录（跳过安装台账这类元文件）。
fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == INSTALLS_FILE {
            continue;
        }
        let src = entry.path();
        let dst = to.join(&name);
        if src.is_dir() {
            std::fs::create_dir_all(&dst)?;
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// 清单里所有插件的（id → 前置条件）映射，供 app 提前提示"你这项目还缺什么"。
pub fn requires_map(market: &Marketplace) -> BTreeMap<String, crate::plugin::Requires> {
    market
        .plugins
        .iter()
        .filter_map(|p| p.requires.clone().map(|r| (p.id.clone(), r)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cercis-market-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const GOOD: &str = r#"
schema: cercis.marketplace/1
name: 示例市场
label_cn: 示例市场
repository: https://github.com/example/market
plugins:
  - id: demo_io
    name: Demo IO
    path: plugins/demo_io
    version: 0.1.0
    tier: third-party
    requires_plugin_api: 1
    permissions: 读取数据
    summary: 演示用
"#;

    #[test]
    fn parses_json_and_yaml() {
        let y = Marketplace::from_text(GOOD).unwrap();
        assert_eq!(y.plugins.len(), 1);
        assert_eq!(y.plugin("demo_io").unwrap().path, "plugins/demo_io");
        let json = serde_json::to_string(&y).unwrap();
        let back = Marketplace::from_text(&json).unwrap();
        assert_eq!(back.plugins[0].id, "demo_io");
    }

    #[test]
    fn rejects_bad_markets() {
        // 版本不认识
        assert!(Marketplace::from_text(&GOOD.replace("cercis.marketplace/1", "cercis.marketplace/9")).is_err());
        // 路径跑到仓库外面
        assert!(Marketplace::from_text(&GOOD.replace("plugins/demo_io", "../外面")).is_err());
        assert!(Marketplace::from_text(&GOOD.replace("plugins/demo_io", "/etc/passwd")).is_err());
        // 接口版本高于内核
        assert!(Marketplace::from_text(&GOOD.replace("requires_plugin_api: 1", "requires_plugin_api: 99")).is_err());
        // id 重复
        let dup = GOOD.replace(
            "    summary: 演示用",
            "    summary: 演示用\n  - id: demo_io\n    name: 重复\n    path: plugins/demo_io2\n    version: 0.1.0",
        );
        assert!(Marketplace::from_text(&dup).is_err());
        // 空市场
        assert!(Marketplace::from_text("schema: cercis.marketplace/1\nplugins: []").is_err());
    }

    #[test]
    fn preset_points_at_public_github_repo() {
        let m = PRESET_MARKETS[0];
        assert!(m.repository.starts_with("https://github.com/epigrove/"));
        assert_eq!(m.marketplace_path, "marketplace.json");
        assert_eq!(m.default_ref, "main");
    }

    #[test]
    fn install_copies_and_records_source() {
        let dir = tmp("install");
        let src = dir.join("download/plugins/demo_io");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("plugin.yaml"),
            "id: demo_io\nnamespace: demo_io\nname: Demo\nversion: 0.2.0\ntier: third-party\nrequires_plugin_api: 1\npermissions:\n  read: true\nui:\n  - kind: command\n    id: demo_io.ping\n    label_cn: 打招呼\n",
        )
        .unwrap();
        let global = dir.join("global");

        let (record, dest) = install_from_dir(
            &src,
            &global,
            "示例市场",
            "https://github.com/example/market",
            "v0.2.0",
        )
        .unwrap();
        assert_eq!(record.id, "demo_io");
        assert_eq!(record.version, "0.2.0");
        assert_eq!(record.git_ref, "v0.2.0");
        assert!(record.fingerprint.len() >= 16);
        assert!(dest.join("plugin.yaml").is_file(), "插件应被复制到全局目录");

        // 安装台账能回答"从哪装的、装的哪一版"
        let installs = Installs::load(&global).unwrap();
        let r = installs.get("demo_io").unwrap();
        assert_eq!(r.repository, "https://github.com/example/market");
        assert_eq!(r.market, "示例市场");
    }

    #[test]
    fn install_refuses_broken_manifest() {
        let dir = tmp("bad-manifest");
        let src = dir.join("plugins/bad");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("plugin.yaml"), "id: bad\n").unwrap(); // 缺 name
        let err = install_from_dir(&src, &dir.join("global"), "m", "r", "main").unwrap_err();
        let text = err.to_string();
        assert!(
            text.contains("插件清单") && text.contains("missing field"),
            "坏清单要给出可读错误（含路径与原因）：{text}"
        );
    }

    #[test]
    fn uninstall_removes_copy_and_record() {
        let dir = tmp("uninstall");
        let src = dir.join("plugins/demo_io");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("plugin.yaml"),
            "id: demo_io\nnamespace: demo_io\nname: Demo\nversion: 0.1.0\ntier: third-party\n",
        )
        .unwrap();
        let global = dir.join("global");
        install_from_dir(&src, &global, "m", "r", "main").unwrap();
        assert!(global.join("demo_io").exists());

        // 先试运行：不动文件
        let (_, msg) = uninstall(&global, "demo_io", true).unwrap();
        assert!(msg.contains("会删掉"));
        assert!(global.join("demo_io").exists());

        let (_, msg) = uninstall(&global, "demo_io", false).unwrap();
        assert!(msg.contains("已卸载"));
        assert!(!global.join("demo_io").exists(), "卸载应删掉本地副本");
        assert!(Installs::load(&global).unwrap().get("demo_io").is_none());

        // 没有安装记录的（随包插件）不能物理删除
        assert!(uninstall(&global, "随包插件", false).is_err());
    }
}
