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

/// 停用名单（`plugins/state.yaml`）：被停用的插件不加载、也不生效。
pub const STATE_FILE: &str = "plugins/state.yaml";

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
    /// 访问网络：`false` / `true` / 域名清单（"只允许这些域名"）
    #[serde(default)]
    pub network: NetworkPermission,
    /// **只允许碰这些表**（空 = 不限；没写时若 `requires.tables` 有值就按它算）
    #[serde(default)]
    pub tables: Vec<String>,
}

/// 网络权限：关 / 随便 / 只允许这几个域名（声明式；域名清单会在信任时给用户看）。
#[derive(Debug, Clone, PartialEq)]
pub enum NetworkPermission {
    Off,
    Any,
    Domains(Vec<String>),
}

impl Default for NetworkPermission {
    fn default() -> Self {
        NetworkPermission::Off
    }
}

impl NetworkPermission {
    pub fn is_off(&self) -> bool {
        matches!(self, NetworkPermission::Off)
    }

    /// 这个主机名允许访问吗（`Off` 一律不许；`Domains` 支持 `*.example.com` 后缀写法）。
    pub fn allows_host(&self, host: &str) -> bool {
        match self {
            NetworkPermission::Off => false,
            NetworkPermission::Any => true,
            NetworkPermission::Domains(list) => {
                let h = host.trim().to_ascii_lowercase();
                list.iter().any(|d| {
                    let d = d.trim().to_ascii_lowercase();
                    if let Some(suffix) = d.strip_prefix("*.") {
                        h == suffix || h.ends_with(&format!(".{suffix}"))
                    } else {
                        h == d
                    }
                })
            }
        }
    }

    pub fn summary(&self) -> String {
        match self {
            NetworkPermission::Off => "不联网".to_string(),
            NetworkPermission::Any => "联网（未限定域名）".to_string(),
            NetworkPermission::Domains(list) => format!("只允许访问：{}", list.join("、")),
        }
    }
}

impl<'de> Deserialize<'de> for NetworkPermission {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let v = serde_json::Value::deserialize(deserializer)?;
        match v {
            serde_json::Value::Bool(false) | serde_json::Value::Null => Ok(NetworkPermission::Off),
            serde_json::Value::Bool(true) => Ok(NetworkPermission::Any),
            serde_json::Value::String(s) => {
                if s.trim().is_empty() || s.eq_ignore_ascii_case("false") {
                    Ok(NetworkPermission::Off)
                } else if s.eq_ignore_ascii_case("true") {
                    Ok(NetworkPermission::Any)
                } else {
                    Ok(NetworkPermission::Domains(vec![s]))
                }
            }
            serde_json::Value::Array(a) => {
                let mut list = Vec::new();
                for item in a {
                    match item {
                        serde_json::Value::String(s) => list.push(s),
                        other => {
                            return Err(D::Error::custom(format!(
                                "network 清单里只写域名字符串，收到：{other}"
                            )))
                        }
                    }
                }
                Ok(NetworkPermission::Domains(list))
            }
            other => Err(D::Error::custom(format!(
                "network 只能是 false / true / 域名清单，收到：{other}"
            ))),
        }
    }
}

impl Serialize for NetworkPermission {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            NetworkPermission::Off => serializer.serialize_bool(false),
            NetworkPermission::Any => serializer.serialize_bool(true),
            NetworkPermission::Domains(list) => list.serialize(serializer),
        }
    }
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
    ///
    /// **留空表示入口不需要参数**（直接以 `<entrypoint> --project … --json` 调用）——
    /// 有些插件入口本身没有子命令，硬套一个推断出来的参数只会让它失败。
    /// 需要子命令的插件请显式写清楚。
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

    /// 调用入口时实际使用的参数文本（可能为空：入口不需要参数）。
    pub fn invocation(&self) -> String {
        self.command.trim().to_string()
    }
}

/// 插件清单：`plugin.yaml`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    /// 插件 ID（唯一，小写 ASCII 下划线）
    pub id: String,
    /// 命名空间（D40）：扩展点键必须写在它下面。
    ///
    /// 留空 = 用自己的 ID。官方工具包里的插件可以共用一个命名空间（如 `research`），
    /// 第三方插件默认用它自己的 ID，避免两个独立作者撞同一个键名。
    #[serde(default)]
    pub namespace: String,
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
    /// 声明式前置条件（D38）：项目满足这些条件才生效
    #[serde(default)]
    pub requires: Requires,
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

/// 声明式前置条件（D38）：**插件说清"我需要什么"，内核决定它生不生效**。
///
/// 为什么不让插件自己判断：同一份判断写在每个插件里，就会出现"装了却报错"的体验；
/// 而且项目结构一变（装模板、加列、切术语包），插件无法自动跟着变。
/// 借鉴 Cordis 的 reactive coeffects（声明需求 → 上下文变化时重新分类激活/停用），
/// 这里把需求限定成**可静态核对**的几项，内核每次列插件时现算。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Requires {
    /// 需要这些表存在（如 `observations`）
    #[serde(default)]
    pub tables: Vec<String>,
    /// 需要这些列存在（写成 `表名.列名`）
    #[serde(default)]
    pub columns: Vec<String>,
    /// 需要模板是其中某一份（留空 = 不限）
    #[serde(default)]
    pub template: Vec<String>,
    /// 需要当前术语包是其中某一个（留空 = 不限）
    #[serde(default)]
    pub terms_profile: Vec<String>,
    /// 需要内核版本满足这个范围（如 `>=0.1.0`；留空 = 不限）
    #[serde(default)]
    pub core_version: String,
}

impl Requires {
    /// 有没有写任何条件。
    pub fn is_empty(&self) -> bool {
        self.tables.is_empty()
            && self.columns.is_empty()
            && self.template.is_empty()
            && self.terms_profile.is_empty()
            && self.core_version.trim().is_empty()
    }
}

/// 生效判定结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Activation {
    /// 是否生效（项目满足全部前置条件）
    pub active: bool,
    /// 不生效的原因（人话；生效时为空）
    pub reasons: Vec<String>,
}

impl Activation {
    fn ok() -> Self {
        Self {
            active: true,
            reasons: Vec::new(),
        }
    }
}

/// 版本是否满足要求（支持 `>=x.y.z` / `>x.y` / `<=` / `<` / `=x.y.z` / `^x.y` / 纯 `x.y.z`）。
///
/// 只比数字段，够用就好——插件接口版本是整数（`requires_plugin_api`），
/// 这个函数是给"内核对得上内核版本"这类附加要求用的。
pub fn version_satisfies(current: &str, requirement: &str) -> bool {
    let req = requirement.trim();
    if req.is_empty() {
        return true;
    }
    let (op, want) = if let Some(r) = req.strip_prefix(">=") {
        (">=", r)
    } else if let Some(r) = req.strip_prefix("<=") {
        ("<=", r)
    } else if let Some(r) = req.strip_prefix('>') {
        (">", r)
    } else if let Some(r) = req.strip_prefix('<') {
        ("<", r)
    } else if let Some(r) = req.strip_prefix('^') {
        ("^", r)
    } else if let Some(r) = req.strip_prefix('=') {
        ("=", r)
    } else {
        ("=", req)
    };
    let cur = parse_version(current);
    let want = parse_version(want);
    match op {
        ">=" => cur >= want,
        ">" => cur > want,
        "<=" => cur <= want,
        "<" => cur < want,
        "^" => cur >= want && cur.0 == want.0,
        _ => cur == want,
    }
}

fn parse_version(s: &str) -> (u64, u64, u64) {
    let mut parts = s
        .trim()
        .split(['.', '-', '+'])
        .map(|p| p.trim().parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

/// 停用与屏蔽名单（`plugins/state.yaml`）。
///
/// 两者的区别只在**用户看到的程度**（D43）：停用 = 不加载但还列在插件列表里；
/// 屏蔽 = 连列表里都不出现、命令也不执行（相当于"我不想知道它存在"）。
/// 共同点：都不加载、都在项目流水里留痕、都能随时取消。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginState {
    #[serde(default)]
    pub disabled: Vec<String>,
    #[serde(default)]
    pub blocked: Vec<String>,
}

impl PluginState {
    pub fn load(project_root: impl AsRef<Path>) -> Result<Self> {
        let path = project_root.as_ref().join(STATE_FILE);
        match tx::read_optional(&path)? {
            None => Ok(Self::default()),
            Some(text) => Ok(serde_yaml::from_str(&text)?),
        }
    }

    pub fn save(&self, project_root: impl AsRef<Path>) -> Result<()> {
        let path = project_root.as_ref().join(STATE_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        tx::write_atomic_str(&path, &serde_yaml::to_string(self)?)
    }

    pub fn is_disabled(&self, id: &str) -> bool {
        self.disabled.iter().any(|d| d == id)
    }

    /// 是否被屏蔽（D43）：比"停用"更彻底——不加载、不执行命令，界面里也直接不出现。
    pub fn is_blocked(&self, id: &str) -> bool {
        self.blocked.iter().any(|d| d == id)
    }

    /// 屏蔽（同时会把它当成停用：两者都不加载）。
    pub fn block(&mut self, id: &str) {
        if !self.is_blocked(id) {
            self.blocked.push(id.to_string());
        }
        self.disable(id);
    }

    /// 取消屏蔽（不自动重新启用：要不要用由用户再决定）。
    pub fn unblock(&mut self, id: &str) {
        self.blocked.retain(|d| d != id);
    }

    pub fn disable(&mut self, id: &str) {
        if !self.is_disabled(id) {
            self.disabled.push(id.to_string());
        }
    }

    pub fn enable(&mut self, id: &str) {
        self.disabled.retain(|d| d != id);
    }
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
        // D40：`cercis` 前缀留给官方，第三方不得占用
        if self.id.starts_with("cercis") && self.tier == PluginTier::ThirdParty {
            return Err(CercisError::Invalid(format!(
                "插件 ID 不得以 `cercis` 开头（留给官方）：{}",
                self.id
            )));
        }
        let ns = self.namespace_id();
        if !crate::format::is_ascii_snake(&ns) {
            return Err(CercisError::Invalid(format!(
                "命名空间需为小写 ASCII 下划线形式: {:?}（留空时自动用插件 ID）",
                self.namespace
            )));
        }
        if (ns == "cercis" || ns.starts_with("cercis_")) && self.tier == PluginTier::ThirdParty {
            return Err(CercisError::Invalid(format!(
                "命名空间 `{ns}` 留给官方：第三方插件请用自己的 ID 或自己的前缀"
            )));
        }
        if self.requires_plugin_api > PLUGIN_API_VERSION {
            return Err(CercisError::Invalid(format!(
                "插件 {} 需要接口版本 {}，但内核只支持到 {}；请升级内核",
                self.id, self.requires_plugin_api, PLUGIN_API_VERSION
            )));
        }
        for c in &self.requires.columns {
            if c.split_once('.').is_none() {
                return Err(CercisError::Invalid(format!(
                    "前置条件里的列要写成 表名.列名：{}（插件 {}）",
                    c, self.id
                )));
            }
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
            // D40：扩展点键必须写在命名空间下面（防两份独立插件撞同名键）
            if !u.id.starts_with(&format!("{ns}.")) {
                return Err(CercisError::Invalid(format!(
                    "扩展点 id 必须带命名空间前缀（`{ns}.`）：{}（插件 {}）",
                    u.id, self.id
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

    /// 这个插件实际使用的命名空间（留空 = 用插件 ID）。
    pub fn namespace_id(&self) -> String {
        if self.namespace.trim().is_empty() {
            self.id.clone()
        } else {
            self.namespace.trim().to_string()
        }
    }

    /// 生效判定（D38）：拿项目现状核对该插件的前置条件。
    ///
    /// 只看四件事：模板名、表与列在不在、术语包、内核版本。判断尽量"宽进"——
    /// 读不到的信息不当成失败（比如项目还没装模板时，只按 `tables` / `columns` 报缺）。
    pub fn activation(&self, project_root: impl AsRef<Path>) -> Activation {
        let root = project_root.as_ref();
        let mut reasons: Vec<String> = Vec::new();

        if !self.requires.core_version.trim().is_empty()
            && !version_satisfies(crate::CORE_VERSION, &self.requires.core_version)
        {
            reasons.push(format!(
                "需要内核 {}，当前是 {}",
                self.requires.core_version,
                crate::CORE_VERSION
            ));
        }

        let template = crate::Template::load(root.join("template.yaml")).ok();
        if !self.requires.template.is_empty() {
            let name = template.as_ref().map(|t| t.name.clone()).unwrap_or_default();
            if !self.requires.template.contains(&name) {
                reasons.push(format!(
                    "需要模板是 {}，当前是 {}",
                    self.requires.template.join(" / "),
                    if name.is_empty() { "（未装模板）" } else { &name }
                ));
            }
        }

        if !self.requires.terms_profile.is_empty() {
            let profile = crate::Terms::load(root)
                .ok()
                .flatten()
                .map(|t| t.profile)
                .unwrap_or_else(|| {
                    template
                        .as_ref()
                        .map(|t| t.terms_profile.clone())
                        .unwrap_or_default()
                });
            if !self.requires.terms_profile.contains(&profile) {
                reasons.push(format!(
                    "需要术语包是 {}，当前是 {}",
                    self.requires.terms_profile.join(" / "),
                    if profile.is_empty() { "（默认）" } else { &profile }
                ));
            }
        }

        for table in &self.requires.tables {
            if !root.join("tables").join(format!("{table}.csv")).exists() {
                reasons.push(format!("缺少表 {table}"));
            }
        }
        for pair in &self.requires.columns {
            let Some((table, column)) = pair.split_once('.') else {
                continue;
            };
            let path = root.join("tables").join(format!("{table}.csv"));
            match crate::Table::load(&path) {
                Ok(t) => {
                    if !t.columns.iter().any(|c| c == column) {
                        reasons.push(format!("表 {table} 里缺少列 {column}"));
                    }
                }
                Err(_) => reasons.push(format!("缺少表 {table}（因而没有列 {column}）")),
            }
        }

        if reasons.is_empty() {
            Activation::ok()
        } else {
            Activation {
                active: false,
                reasons,
            }
        }
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
        if !self.permissions.network.is_off() {
            parts.push(match &self.permissions.network {
                NetworkPermission::Any => "访问网络",
                NetworkPermission::Domains(_) => "访问指定域名",
                NetworkPermission::Off => "不联网",
            });
        }
        if !self.permissions.tables.is_empty() {
            parts.push("限这几张表");
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
    /// 被明确批准"可以联网"的插件（清单里声明了网络、且用户点过头的）
    #[serde(default)]
    pub network_allowed: std::collections::BTreeSet<String>,
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

    /// 这个插件被批准联网了吗（声明里要联网 + 用户点过头）。
    pub fn is_network_allowed(&self, id: &str) -> bool {
        self.network_allowed.contains(id)
    }

    /// 批准 / 取消"允许联网"。
    pub fn set_network_allowed(&mut self, id: &str, allowed: bool) {
        if allowed {
            self.network_allowed.insert(id.to_string());
        } else {
            self.network_allowed.remove(id);
        }
    }
}

/// 插件运行上下文（**权限降级的执行点**，Cordis §6.3）。
///
/// 插件宿主（app）在拉起插件时注入环境变量，内核在读写通道上按它执法：
///
/// | 环境变量 | 含义 |
/// |---|---|
/// | `CERCIS_PLUGIN_ID` | 是哪个插件在调用内核 |
/// | `CERCIS_PLUGIN_TABLES` | 只允许碰这些表（逗号分隔；空 = 清单里没声明就按 `requires.tables`，仍为空则不限） |
/// | `CERCIS_PLUGIN_WRITE` | `1`/`0`：清单里 `permissions.write` 是否为真 |
/// | `CERCIS_PLUGIN_NETWORK` | `off` / `any` / 域名清单（逗号分隔） |
///
/// **诚实边界**：这是"协作边界"而不是沙箱——同一台机器上有心的程序总能绕过环境变量。
/// 它挡住的是**误伤**：插件没声明读哪张表就不会读到，没声明写就写不动；真要防恶意程序，
/// 靠的是信任指纹（内容一变信任就失效）+ 不装来路不明的插件。
#[derive(Debug, Clone)]
pub struct PluginContext {
    pub plugin_id: String,
    /// 允许的表（空 = 不限）
    pub tables: Vec<String>,
    pub write: bool,
    pub network: NetworkPermission,
}

impl PluginContext {
    /// 从环境变量读；**没有 `CERCIS_PLUGIN_ID` 就不是插件在调用**（人、脚本、app 本身）。
    pub fn from_env() -> Option<Self> {
        let plugin_id = std::env::var("CERCIS_PLUGIN_ID").ok()?;
        if plugin_id.trim().is_empty() {
            return None;
        }
        let tables = std::env::var("CERCIS_PLUGIN_TABLES")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let write = std::env::var("CERCIS_PLUGIN_WRITE")
            .map(|v| matches!(v.trim(), "1" | "true" | "yes"))
            .unwrap_or(false);
        let network = match std::env::var("CERCIS_PLUGIN_NETWORK") {
            Ok(v) if v.trim().eq_ignore_ascii_case("any") => NetworkPermission::Any,
            Ok(v) if v.trim().is_empty() || v.trim().eq_ignore_ascii_case("off") => {
                NetworkPermission::Off
            }
            Ok(v) => NetworkPermission::Domains(
                v.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect(),
            ),
            Err(_) => NetworkPermission::Off,
        };
        Some(Self {
            plugin_id,
            tables,
            write,
            network,
        })
    }

    pub fn table_allowed(&self, table: &str) -> bool {
        // 系统表（流水、更正）是内核自己的账本：插件每次写入都要留痕，
        // 把它挡掉等于让"写入必须留痕"这条硬规则做不到——所以永远放行。
        matches!(table, "change_log" | "corrections")
            || self.tables.is_empty()
            || self.tables.iter().any(|t| t == table)
    }

    /// 读这张表行不行。
    pub fn ensure_table(&self, table: &str, project_root: &Path) -> Result<()> {
        if self.table_allowed(table) {
            return Ok(());
        }
        deny(
            project_root,
            &self.plugin_id,
            &format!("想读表 {table}，但清单里只允许它碰：{}", self.tables.join("、")),
        );
        Err(CercisError::Invalid(format!(
            "插件 {} 没有读表 {table} 的权限（清单里限定了：{}）。\n\
             - 要让它读，请在 plugin.yaml 的 permissions.tables 里加上；\n\
             - 或者不用它（停用：cercis plugins <项目> disable --id {}）",
            self.plugin_id,
            self.tables.join("、"),
            self.plugin_id
        )))
    }

    /// 写行不行（清单里 `permissions.write: false` 的插件一律只读）。
    pub fn ensure_write(&self, project_root: &Path) -> Result<()> {
        if self.write {
            return Ok(());
        }
        deny(
            project_root,
            &self.plugin_id,
            "想写数据，但清单里 permissions.write 是 false",
        );
        Err(CercisError::Invalid(format!(
            "插件 {} 是只读的（清单里 permissions.write: false），不能写入项目数据。\n\
             - 这是插件的声明，不是内核临时拦你；要写就得改清单并重新信任",
            self.plugin_id
        )))
    }

    /// 访问某个主机行不行（域名叫什么由清单声明，信任时给用户看过）。
    pub fn ensure_host(&self, host: &str, project_root: &Path) -> Result<()> {
        if self.network.allows_host(host) {
            return Ok(());
        }
        deny(
            project_root,
            &self.plugin_id,
            &format!("想访问 {host}，但清单里网络权限是：{}", self.network.summary()),
        );
        Err(CercisError::Invalid(format!(
            "插件 {} 没有访问 {host} 的权限（清单里写的是：{}）",
            self.plugin_id,
            self.network.summary()
        )))
    }
}

/// 把一次越权尝试写进 `_labnote/denied.log`（**故意不走内核写入通道**：
/// 越权日志本身不能被同一个权限挡住，否则就没证据了）。
fn deny(project_root: &Path, plugin_id: &str, what: &str) {
    let dir = project_root.join("_labnote");
    let _ = std::fs::create_dir_all(&dir);
    let line = format!("{}  {}  {}\n", crate::ids::now_iso8601(), plugin_id, what);
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("denied.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// 跨插件的键冲突检查（D40）。
///
/// 为什么必须有：内核按**键名**连接界面扩展点。两份互不相干的插件用了同一个键，
/// 界面就会把某一个的实现当成另一个来调（Cordis §6.6 说的 "key collision"）。
/// 规则：
/// 1. 两份插件声明的界面扩展点 id 不得相同；
/// 2. 第三方插件的命名空间不得与别的插件重复（官方工具包内部共用一个命名空间是允许的）。
pub fn conflicts(found: &[DiscoveredPlugin]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (i, a) in found.iter().enumerate() {
        for b in found.iter().skip(i + 1) {
            let shared: Vec<String> = a
                .manifest
                .ui
                .iter()
                .filter(|u| b.manifest.ui.iter().any(|v| v.id == u.id))
                .map(|u| u.id.clone())
                .collect();
            if !shared.is_empty() {
                out.push(format!(
                    "插件 {} 与 {} 都声明了扩展点 {}——请各自换一个键名",
                    a.manifest.id,
                    b.manifest.id,
                    shared.join("、")
                ));
            }
            let third = |p: &DiscoveredPlugin| p.manifest.tier == PluginTier::ThirdParty;
            if (third(a) || third(b)) && a.manifest.namespace_id() == b.manifest.namespace_id() {
                out.push(format!(
                    "插件 {} 与 {} 争用同一个命名空间 `{}`——第三方插件请用自己的前缀",
                    a.manifest.id,
                    b.manifest.id,
                    a.manifest.namespace_id()
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod context_tests {
    use super::*;

    fn ctx(tables: &[&str], write: bool) -> PluginContext {
        PluginContext {
            plugin_id: "demo".into(),
            tables: tables.iter().map(|s| s.to_string()).collect(),
            write,
            network: NetworkPermission::Domains(vec!["github.com".into(), "*.example.org".into()]),
        }
    }

    #[test]
    fn table_scope_is_enforced_and_logged() {
        let dir = std::env::temp_dir().join("cercis-plugin-guard-table");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let c = ctx(&["observations"], true);
        c.ensure_table("observations", &dir).unwrap();
        let err = c.ensure_table("private_notes", &dir).unwrap_err();
        assert!(err.to_string().contains("没有读表"), "{err}");
        // 越权要留痕（故意不走内核写入通道，免得被同一个权限挡住）
        let log = std::fs::read_to_string(dir.join("_labnote/denied.log")).unwrap();
        assert!(log.contains("demo") && log.contains("private_notes"), "{log}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_only_plugin_cannot_write() {
        let dir = std::env::temp_dir().join("cercis-plugin-guard-write");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let c = ctx(&[], false);
        let err = c.ensure_write(&dir).unwrap_err();
        assert!(err.to_string().contains("只读"), "{err}");
        assert!(ctx(&[], true).ensure_write(&dir).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_domains_are_checked() {
        let c = ctx(&[], true);
        assert!(c.network.allows_host("github.com"));
        assert!(c.network.allows_host("api.example.org"));
        assert!(c.network.allows_host("example.org"));
        assert!(!c.network.allows_host("evil.com"));
        assert!(!NetworkPermission::Off.allows_host("github.com"));
        assert!(NetworkPermission::Any.allows_host("anything.example"));

        let dir = std::env::temp_dir().join("cercis-plugin-guard-net");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(c.ensure_host("github.com", &dir).is_ok());
        let err = c.ensure_host("evil.com", &dir).unwrap_err();
        assert!(err.to_string().contains("evil.com"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn network_permission_parses_bool_and_list() {
        let m = crate::plugin::PluginManifest::from_yaml(
            "id: demo\nnamespace: demo\nname: Demo\nlabel_cn: 演示\nversion: 0.1.0\n\
             tier: third-party\nrequires_plugin_api: 1\nentrypoint: demo\n\
             permissions:\n  read: true\n  write: false\n  network: [github.com, '*.example.org']\n\
             ui: []\n",
        )
        .unwrap();
        match &m.permissions.network {
            NetworkPermission::Domains(list) => assert_eq!(list.len(), 2),
            other => panic!("应解析成域名清单：{other:?}"),
        }
        let m2 = crate::plugin::PluginManifest::from_yaml(
            "id: demo2\nnamespace: demo2\nname: Demo2\nlabel_cn: 演示2\nversion: 0.1.0\n\
             tier: third-party\nrequires_plugin_api: 1\nentrypoint: demo2\n\
             permissions:\n  read: true\n  write: false\n  network: false\nui: []\n",
        )
        .unwrap();
        assert!(m2.permissions.network.is_off());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::*;

    const SAMPLE: &str = r#"
id: export_spss
namespace: export
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
        // 未写 command 时不留推断：入口不需要参数
        assert_eq!(export.invocation(), "");

        let with_command = SAMPLE.replace(
            "  - kind: menu\n    id: export.spss",
            "  - kind: command\n    id: export.data_import\n    command: data import\n    label_cn: 导入数据\n    label_en: Import data\n  - kind: command\n    id: export.health_check\n    label_cn: 体检\n    label_en: Health check\n  - kind: menu\n    id: export.spss",
        );
        let m2 = PluginManifest::from_yaml(&with_command).unwrap();
        let import = m2.ui.iter().find(|u| u.id == "export.data_import").unwrap();
        assert_eq!(import.invocation(), "data import");
        let health = m2.ui.iter().find(|u| u.id == "export.health_check").unwrap();
        assert_eq!(
            health.invocation(),
            "",
            "没有子命令的入口应保持空参数（不推断 id）"
        );
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

    #[test]
    fn ui_keys_must_be_namespaced() {
        // 没写命名空间时用插件 ID：`export.spss` 就不合法了
        let bad = SAMPLE.replace("namespace: export\n", "");
        assert!(
            PluginManifest::from_yaml(&bad).is_err(),
            "扩展点键必须带命名空间前缀"
        );
        // 命名空间可以显式声明，官方工具包里多个插件共用一个是允许的
        let ok = PluginManifest::from_yaml(SAMPLE).unwrap();
        assert_eq!(ok.namespace_id(), "export");
    }

    #[test]
    fn third_party_cannot_use_cercis_prefix() {
        let bad = SAMPLE
            .replace("id: export_spss", "id: cercis_something")
            .replace("tier: research", "tier: third-party");
        assert!(PluginManifest::from_yaml(&bad).is_err());
    }

    #[test]
    fn version_requirements() {
        assert!(version_satisfies("0.1.0", ">=0.1.0"));
        assert!(version_satisfies("0.2.0", ">=0.1.0"));
        assert!(!version_satisfies("0.1.0", ">=0.2.0"));
        assert!(version_satisfies("1.4.0", "^1.0"));
        assert!(!version_satisfies("2.0.0", "^1.0"));
        assert!(version_satisfies("0.1.0", ""));
        assert!(version_satisfies("0.1.0", "=0.1.0"));
        assert!(version_satisfies("0.1.0", "<0.2"));
    }

    #[test]
    fn activation_reports_missing_tables_and_columns() {
        let dir = std::env::temp_dir().join("cercis-activation-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tables")).unwrap();
        let m = PluginManifest::from_yaml(
            &SAMPLE.replace(
                "entrypoint: cercis-spss",
                "entrypoint: cercis-spss\nrequires:\n  tables: [observations]\n  columns: [observations.value_num]\n  core_version: \">=0.1.0\"",
            ),
        )
        .unwrap();

        let a = m.activation(&dir);
        assert!(!a.active);
        assert!(a.reasons.iter().any(|r| r.contains("缺少表 observations")), "{:?}", a.reasons);

        // 建了表但还没这一列
        let path = dir.join("tables/observations.csv");
        let mut t = crate::Table::in_memory(&path, &["obs_id"]);
        t.push(crate::table::Row::new());
        t.save().unwrap();
        let a2 = m.activation(&dir);
        assert!(!a2.active);
        assert!(
            a2.reasons.iter().any(|r| r.contains("缺少列 value_num")),
            "{:?}",
            a2.reasons
        );

        // 列也齐了 → 生效
        let mut t = crate::Table::in_memory(&path, &["obs_id", "value_num"]);
        t.push(crate::table::Row::new());
        t.save().unwrap();
        assert!(m.activation(&dir).active, "条件齐了就应当生效");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cross_plugin_key_conflicts_are_reported() {
        let dir = std::env::temp_dir().join("cercis-conflict-test");
        let _ = std::fs::remove_dir_all(&dir);
        for (sub, yaml) in [
            ("a", SAMPLE.replace("id: export_spss", "id: alpha")),
            ("b", SAMPLE.replace("id: export_spss", "id: beta")),
        ] {
            let d = dir.join("plugins").join(sub);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("plugin.yaml"), yaml).unwrap();
        }
        let found = discover(&dir, None).unwrap();
        let list = conflicts(&found);
        assert!(list.iter().any(|c| c.contains("都声明了扩展点")), "{list:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
