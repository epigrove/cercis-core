//! 插件市场（D41/D42）：**清单是一份文件，下载在 app，安装落盘与记账在内核**。
//!
//! 分工（需求文档 §14.8）：
//! - ① 清单：托管在 git 仓库里的 `marketplace.json`（预置两份——**基础包**与**科研包**，
//!   都指向 GitHub 公开仓库；也允许添加私有仓库，鉴权在 app 侧做）
//! - ② 看清单 / 下载 / 更新检查：app
//! - ③ 装卸与屏蔽的界面：基础工具包里的"插件管理器"
//! - ④ **协议、校验、安装落盘、来源与指纹记录：内核**（本模块）
//!
//! 所以本模块**不联网**：它做的是"给我一个已经下载好的目录/文件，我校验它、装它、记账"。
//! 这样三条路都通：app 下载后调内核、脚本手动装、断网时拖入插件包（FR-P48）。
//!
//! ## D42：市场就是 Cercis 管理插件的方式
//!
//! - **基础工具包市场**：`bundled = true`（随 app 交付）。插件**不可删除**——
//!   只能停用/屏蔽；市场条目本身也**不可移除**（它是产品的一部分）。
//! - **科研工具包市场**：`bundled = false`，只是**预添加**（装上就能看到有哪些可装），
//!   **不预装任何插件**；插件与市场条目都**允许删除**。
//! - 用户添加的市场（含私有仓库）：同科研包待遇——可装、可删。

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

/// 用户添加的市场条目 + 被移除的预置市场（**用户级**）。
///
/// `removed_presets` 让"预添加"名副其实：科研市场允许被移除，移除后不再出现在列表里
/// （想找回来用 `market add --preset research`）；基础市场是随包的一部分，永远移除不了。
pub const MARKETS_FILE: &str = "markets.json";

/// 默认预置的市场（D41：指向 GitHub 公开仓库；D42：基础包随包不可删，科研包预添加可删）。
///
/// 开发期这些仓库可能还是私有的——那时拉取会失败，属于预期（鉴权在 app 侧）。
pub const PRESET_MARKETS: [PresetMarket; 2] = [
    PresetMarket {
        id: "basic",
        label_cn: "基础工具包（随 app 交付）",
        repository: "https://github.com/epigrove/cercis-kits-basic",
        marketplace_path: "marketplace.json",
        default_ref: "main",
        bundled: true,
        preset_added: true,
    },
    PresetMarket {
        id: "research",
        label_cn: "科研工具包（官方）",
        repository: "https://github.com/epigrove/cercis-kits-research",
        marketplace_path: "marketplace.json",
        default_ref: "main",
        bundled: false,
        preset_added: true,
    },
];

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
    /// 是否随 app 交付：**随包的市场条目不可移除，它装的插件不可删除**（D42）
    pub bundled: bool,
    /// 是否属于"预添加"的市场（基础包与科研包都是；科研包允许被移除）
    pub preset_added: bool,
}

/// 用户添加的市场条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketEntry {
    pub id: String,
    #[serde(default)]
    pub label_cn: String,
    /// git 仓库地址（私有仓库也写在这里；鉴权在 app 侧）
    pub repository: String,
    #[serde(default = "default_marketplace_path")]
    pub marketplace_path: String,
    #[serde(default = "default_ref")]
    pub default_ref: String,
    #[serde(default)]
    pub bundled: bool,
    pub added_at: String,
}

fn default_marketplace_path() -> String {
    "marketplace.json".to_string()
}

fn default_ref() -> String {
    "main".to_string()
}

/// 用户级市场配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UserMarkets {
    #[serde(default)]
    pub schema: String,
    /// 用户自己添加的市场
    #[serde(default)]
    pub markets: Vec<MarketEntry>,
    /// 被移除的预置市场 id（"预添加"允许移除，想找回来就 add --preset）
    #[serde(default)]
    pub removed_presets: Vec<String>,
}

impl UserMarkets {
    pub fn load(global_dir: impl AsRef<Path>) -> Result<Self> {
        let path = global_dir.as_ref().join(MARKETS_FILE);
        match tx::read_optional(&path)? {
            None => Ok(Self {
                schema: "cercis.markets/1".into(),
                markets: Vec::new(),
                removed_presets: Vec::new(),
            }),
            Some(text) => Ok(serde_json::from_str(&text)?),
        }
    }

    pub fn save(&self, global_dir: impl AsRef<Path>) -> Result<()> {
        let dir = global_dir.as_ref();
        std::fs::create_dir_all(dir)?;
        tx::write_atomic_str(
            &dir.join(MARKETS_FILE),
            &serde_json::to_string_pretty(self)?,
        )
    }

    /// 添加一个市场（用户级）。
    pub fn add(&mut self, entry: MarketEntry) {
        self.markets.retain(|m| m.id != entry.id);
        self.removed_presets.retain(|p| p != &entry.id);
        self.markets.push(entry);
    }

    /// 移除一个市场：预置的记进"已移除"，用户添加的直接删。
    ///
    /// 返回（是否移除成功, 人话）。**随包市场（bundled）拒绝移除**。
    pub fn remove(&mut self, id: &str) -> Result<String> {
        if let Some(p) = PRESET_MARKETS.iter().find(|p| p.id == id) {
            if p.bundled {
                return Err(CercisError::Invalid(format!(
                    "市场 {} 是随 app 交付的一部分，不能移除（它装的插件也只能停用，不能删除）",
                    p.label_cn
                )));
            }
            if !self.removed_presets.iter().any(|x| x == id) {
                self.removed_presets.push(id.to_string());
            }
            return Ok(format!(
                "已移除预置市场 {}（想找回来：market add --preset {id}）",
                p.label_cn
            ));
        }
        let before = self.markets.len();
        self.markets.retain(|m| m.id != id);
        if self.markets.len() == before {
            return Err(CercisError::NotFound(format!("没有这个市场：{id}")));
        }
        Ok(format!("已移除市场 {id}（它装的插件不会被自动卸载）"))
    }

    /// 把预置市场恢复回来（add --preset）。
    pub fn restore_preset(&mut self, id: &str) -> Result<()> {
        if !PRESET_MARKETS.iter().any(|p| p.id == id) {
            return Err(CercisError::NotFound(format!(
                "没有这个预置市场：{id}（可用：{}）",
                PRESET_MARKETS
                    .iter()
                    .map(|p| p.id)
                    .collect::<Vec<_>>()
                    .join(" / ")
            )));
        }
        self.removed_presets.retain(|p| p != id);
        Ok(())
    }

    /// 现在生效的全部市场（预置里没被移除的 + 用户添加的）。
    pub fn effective(&self) -> Vec<MarketEntry> {
        let mut out: Vec<MarketEntry> = PRESET_MARKETS
            .iter()
            .filter(|p| !self.removed_presets.iter().any(|r| r == p.id))
            .map(|p| MarketEntry {
                id: p.id.to_string(),
                label_cn: p.label_cn.to_string(),
                repository: p.repository.to_string(),
                marketplace_path: p.marketplace_path.to_string(),
                default_ref: p.default_ref.to_string(),
                bundled: p.bundled,
                added_at: String::new(),
            })
            .collect();
        out.extend(self.markets.iter().cloned());
        out
    }
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
    /// 是否随 app 交付（D42）：随包市场里的插件**不可删除**，只能停用
    #[serde(default)]
    pub bundled: bool,
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
    /// 是不是随包插件（D42）：随包的**不能删除**，只能停用/屏蔽
    #[serde(default)]
    pub bundled: bool,
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

/// 删除墓碑（D43）：用户删过的插件，升级**不许偷偷恢复**。
///
/// 为什么必须有它：随包插件躺在安装目录里，升级/重装会把文件再放回来——
/// 那时"用户删过它"这件事如果说不出、记不住，删除就是假权限。
/// 所以删除 = 删文件（能删就删）+ 记一条墓碑；`market restore` 才清墓碑。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Removal {
    pub id: String,
    /// 从哪个市场来的（留空 = 随包/未知）
    #[serde(default)]
    pub market: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub fingerprint: String,
    pub removed_at: String,
    /// 用户填的理由（可选）
    #[serde(default)]
    pub note: String,
    /// 文件是不是真的被删掉了（false = 文件在只读位置，只能"逻辑删除"）
    #[serde(default)]
    pub files_removed: bool,
}

/// 删除名单。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Removals {
    #[serde(default)]
    pub schema: String,
    #[serde(default)]
    pub removed: Vec<Removal>,
}

impl Removals {
    /// 载入（默认位置：`<用户插件目录>/removed.json`）。
    pub fn load(global_dir: impl AsRef<Path>) -> Result<Self> {
        let path = global_dir.as_ref().join(REMOVED_FILE);
        match tx::read_optional(&path)? {
            None => Ok(Self {
                schema: "cercis.removals/1".into(),
                removed: Vec::new(),
            }),
            Some(text) => Ok(serde_json::from_str(&text)?),
        }
    }

    pub fn save(&self, global_dir: impl AsRef<Path>) -> Result<()> {
        let dir = global_dir.as_ref();
        std::fs::create_dir_all(dir)?;
        tx::write_atomic_str(&dir.join(REMOVED_FILE), &serde_json::to_string_pretty(self)?)
    }

    pub fn get(&self, id: &str) -> Option<&Removal> {
        self.removed.iter().find(|r| r.id == id)
    }

    /// 记一条墓碑（同一个 id 覆盖旧的）。
    pub fn put(&mut self, r: Removal) {
        self.removed.retain(|x| x.id != r.id);
        self.removed.push(r);
    }

    /// 清掉墓碑（找回插件时用）。
    pub fn clear(&mut self, id: &str) -> bool {
        let before = self.removed.len();
        self.removed.retain(|r| r.id != id);
        self.removed.len() != before
    }
}

/// 删除名单文件。
pub const REMOVED_FILE: &str = "removed.json";

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
    bundled: bool,
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
        bundled,
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

/// 删除插件（D43）。
///
/// **权限怎么分**（用户"能不用它"，但不能把产品拆坏）：
/// - **随 app 交付的基础插件不能删除**——文件是产品的一部分，升级会重新放回来，
///   删了只会让"文件在、状态乱"。要它别碍事用**停用**或**屏蔽**：
///   屏蔽之后不加载、界面里不出现、命令也不执行（效果上就是"我不想要它了"）。
///   这叫**有限度地不启用**：能不用，但不能把地基拆掉。
/// - **市场装的插件（科研包 / 第三方 / 自建市场）可以删除**：删本地副本 + 记一条墓碑
///   （升级不许偷偷装回来）+ 清安装记录；`market restore` 随时找回。
/// - 没有安装记录的（既不是市场装的、也没登记过）一律当"随包 / 未知来源"，不删。
pub fn uninstall(global_dir: &Path, id: &str, dry_run: bool, note: &str) -> Result<(bool, String)> {
    let mut installs = Installs::load(global_dir)?;
    let Some(record) = installs.get(id).cloned() else {
        return Err(CercisError::Invalid(format!(
            "{id} 没有市场安装记录——随 app 交付的插件不删除。\
             让它别碍事请用：`cercis plugins <项目> block --id {id}`（屏蔽：不加载、界面不出现、命令不执行）\
             或 `cercis plugins <项目> disable --id {id}`（停用：不加载，但仍列在列表里）"
        )));
    };
    if record.bundled {
        return Err(CercisError::Invalid(format!(
            "{id} 属于随 app 交付的基础工具包，不删除（升级会重新放回来，删了只会让状态变乱）。\
             要它别碍事请用：`cercis plugins <项目> block --id {id}`（屏蔽）或 `disable`（停用）"
        )));
    }
    let mut removals = Removals::load(global_dir)?;
    let dir = global_dir.join(&record.path);
    if dry_run {
        return Ok((
            true,
            format!(
                "会删掉 {}（{}；指纹 {}；并记一条墓碑：升级不会自动装回来）",
                dir.display(),
                record.version,
                &record.fingerprint[..8.min(record.fingerprint.len())]
            ),
        ));
    }

    let files_removed = if dir.exists() {
        std::fs::remove_dir_all(&dir).is_ok()
    } else {
        true // 本来就没有文件，也算"没有留下东西"
    };
    removals.put(Removal {
        id: id.to_string(),
        market: record.market.clone(),
        version: record.version.clone(),
        fingerprint: record.fingerprint.clone(),
        removed_at: crate::ids::now_iso8601(),
        note: note.to_string(),
        files_removed,
    });
    removals.save(global_dir)?;
    if installs.remove(id) {
        installs.save(global_dir)?;
    }
    Ok((
        true,
        format!(
            "已删除 {id}{}（已记墓碑：升级不会自动装回来；找回用 `market restore --id {id}`）",
            if files_removed { "（本地文件已删）" } else { "（文件没能删掉）" }
        ),
    ))
}

/// 找回被删掉的插件（清墓碑；`from` 给了就顺手从已下载目录装回来）。
pub fn restore(
    global_dir: &Path,
    id: &str,
    from: Option<&Path>,
    repository: &str,
    market: &str,
    git_ref: &str,
    bundled: bool,
) -> Result<(bool, String, Option<PathBuf>)> {
    let mut removals = Removals::load(global_dir)?;
    let had = removals.clear(id);
    removals.save(global_dir)?;
    match from {
        Some(dir) => {
            let repo = if repository.trim().is_empty() {
                dir.display().to_string()
            } else {
                repository.trim().to_string()
            };
            let (_, dest) = install_from_dir(dir, global_dir, market, &repo, git_ref, bundled)?;
            Ok((
                true,
                format!("已找回 {id}：墓碑已清，插件已从本地目录装回"),
                Some(dest),
            ))
        }
        None => Ok((
            had,
            format!(
                "已清掉 {id} 的墓碑（不再算\"用户删过\"）；想立刻可用就再跑一次 \
                 `market install --from <已下载目录> --plugin {id}`"
            ),
            None,
        )),
    }
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
    fn presets_are_public_and_follow_d42() {
        // 两份预置市场都指向公开 GitHub 仓库
        for m in PRESET_MARKETS.iter() {
            assert!(m.repository.starts_with("https://github.com/epigrove/"), "{m:?}");
            assert_eq!(m.marketplace_path, "marketplace.json");
            assert_eq!(m.default_ref, "main");
            assert!(m.preset_added, "两份都是预添加");
        }
        let basic = PRESET_MARKETS.iter().find(|m| m.id == "basic").unwrap();
        let research = PRESET_MARKETS.iter().find(|m| m.id == "research").unwrap();
        assert!(basic.bundled, "基础包随 app 交付 → 不可删");
        assert!(!research.bundled, "科研包只是预添加 → 可删");
    }

    #[test]
    fn d42_market_removal_rules() {
        let dir = tmp("markets");
        let mut m = UserMarkets::load(&dir).unwrap();
        // 基础市场：不可移除
        assert!(m.remove("basic").is_err());
        // 科研市场：可以移除，也能找回来
        assert!(m.remove("research").unwrap().contains("已移除"));
        assert!(m.effective().iter().all(|x| x.id != "research"), "移除后不再出现");
        m.restore_preset("research").unwrap();
        assert!(m.effective().iter().any(|x| x.id == "research"));
        // 用户自己添加的市场：加了能删
        m.add(MarketEntry {
            id: "myteam".into(),
            label_cn: "团队私有市场".into(),
            repository: "https://git.example.com/team/market".into(),
            marketplace_path: "marketplace.json".into(),
            default_ref: "main".into(),
            bundled: false,
            added_at: crate::ids::now_iso8601(),
        });
        assert!(m.effective().iter().any(|x| x.id == "myteam" && !x.bundled));
        assert!(m.remove("myteam").is_ok());
        assert!(m.remove("myteam").is_err(), "再删就找不到了");
    }

    #[test]
    fn bundled_plugin_cannot_be_deleted_but_can_be_blocked() {
        // D43：基础插件不可删（文件是产品的一部分），但用户可以让它"有限度地不启用"
        let dir = tmp("bundled");
        let src = dir.join("plugins/health_check");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("plugin.yaml"),
            "id: health_check\nnamespace: health\nname: Health\nversion: 0.1.0\ntier: basic\n",
        )
        .unwrap();
        let global = dir.join("global");
        let (record, _) =
            install_from_dir(&src, &global, "基础工具包", "repo", "main", true).unwrap();
        assert!(record.bundled);

        let err = uninstall(&global, "health_check", false, "").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("不删除"), "{text}");
        assert!(text.contains("block") && text.contains("disable"), "要指路：{text}");
        assert!(global.join("health_check").exists(), "文件必须留着");

        // 屏蔽：不加载、界面不出现、命令不执行（D43 的"有限度地不启用"）
        let mut state = crate::plugin::PluginState::default();
        state.block("health_check");
        assert!(state.is_blocked("health_check"));
        assert!(state.is_disabled("health_check"), "屏蔽同时也等于停用");
        state.unblock("health_check");
        assert!(!state.is_blocked("health_check"));
        assert!(state.is_disabled("health_check"), "取消屏蔽不自动启用");
        state.enable("health_check");
        assert!(!state.is_disabled("health_check"));
    }

    #[test]
    fn unknown_origin_cannot_be_deleted() {
        let dir = tmp("unknown");
        let global = dir.join("global");
        let err = uninstall(&global, "某个随包插件", false, "").unwrap_err();
        assert!(err.to_string().contains("没有市场安装记录"), "{err}");
    }

    #[test]
    fn restore_can_install_again_from_local_dir() {
        let dir = tmp("restore");
        let src = dir.join("plugins/demo_io");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("plugin.yaml"),
            "id: demo_io\nnamespace: demo_io\nname: Demo\nversion: 0.3.0\ntier: third-party\n",
        )
        .unwrap();
        let global = dir.join("global");
        install_from_dir(&src, &global, "演示市场", "repo", "main", false).unwrap();
        uninstall(&global, "demo_io", false, "").unwrap();
        assert!(!global.join("demo_io").exists());

        let (_, msg, dest) = restore(
            &global,
            "demo_io",
            Some(&src),
            "https://github.com/example/market",
            "演示市场",
            "v0.3.0",
            false,
        )
        .unwrap();
        assert!(msg.contains("装回"), "{msg}");
        assert!(dest.unwrap().join("plugin.yaml").is_file());
        assert_eq!(
            Installs::load(&global).unwrap().get("demo_io").unwrap().version,
            "0.3.0"
        );
        assert!(Removals::load(&global).unwrap().get("demo_io").is_none());
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
            false,
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
        let err = install_from_dir(&src, &dir.join("global"), "m", "r", "main", false).unwrap_err();
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
        install_from_dir(&src, &global, "m", "r", "main", false).unwrap();
        assert!(global.join("demo_io").exists());

        // 先试运行：不动文件
        let (_, msg) = uninstall(&global, "demo_io", true, "").unwrap();
        assert!(msg.contains("会删掉"));
        assert!(global.join("demo_io").exists());

        let (_, msg) = uninstall(&global, "demo_io", false, "").unwrap();
        assert!(msg.contains("已删除"));
        assert!(!global.join("demo_io").exists(), "卸载应删掉本地副本");
        assert!(Installs::load(&global).unwrap().get("demo_io").is_none());

        // 没有安装记录（随包/未知来源）不删——但会给"怎么让它别碍事"的指路
        let err = uninstall(&global, "某个随包插件", false, "不想用").unwrap_err();
        assert!(err.to_string().contains("没有市场安装记录"), "{err}");
    }
}
