//! 期望状态（`plugins/desired.yaml`）：把"我想要哪些插件"写成一份文件，内核负责对账。
//!
//! 为什么需要它（Cordis 的 declarative configuration 那一课，§5.2.1）：
//! 插件一旦装多、跨设备用，就会出现"这台装了这个、那台没装""停用了哪些、屏蔽了哪些"说不清
//! 的情况。把"想要的状态"写成项目里的一份 YAML，好处是：
//! 1. **可比较、可合并**——它随项目走（NAS / 微信 / Git），两台设备一 diff 就知道差在哪；
//! 2. **可解释**——对账结果能说清"缺什么、多的能删、哪些要停用/屏蔽"；
//! 3. **不越权**——对账只做两件它真做得了的事：改项目里的启用/屏蔽状态，
//!    以及给出"该装/该删"的清单；**下载仍然要 app 或用户明确动作**（内核不联网）。
//!
//! 文件长这样（手写或由 app 生成都行）：
//!
//! ```yaml
//! schema: cercis.desired/1
//! install:
//!   - market: research        # 哪个市场
//!     id: research_io         # 装哪个插件
//!     ref: main               # 分支/标签（可省，默认 main）
//! disable: [research_scribe]  # 想要"停用"的
//! block: []                   # 想要"屏蔽"的
//! ```

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::market::Installs;
use crate::plugin::PluginState;
use crate::tx;
use crate::{CercisError, Result};

/// 期望状态文件（相对项目根）。
pub const DESIRED_FILE: &str = "plugins/desired.yaml";

/// 想要装的一个插件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WantedPlugin {
    /// 市场 id（如 `research`；自建市场填用户自己的 id）
    #[serde(default)]
    pub market: String,
    /// 插件 id
    pub id: String,
    /// 分支/标签（留空 = main）
    #[serde(default)]
    pub git_ref: String,
}

/// 期望状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Desired {
    #[serde(default = "default_schema")]
    pub schema: String,
    /// 想要装哪些插件
    #[serde(default)]
    pub install: Vec<WantedPlugin>,
    /// 想要停用的（不加载，但仍列在插件列表里）
    #[serde(default)]
    pub disable: Vec<String>,
    /// 想要屏蔽的（不加载、界面不出现、命令不执行）
    #[serde(default)]
    pub block: Vec<String>,
}

fn default_schema() -> String {
    "cercis.desired/1".to_string()
}

impl Default for Desired {
    fn default() -> Self {
        Self {
            schema: default_schema(),
            install: Vec::new(),
            disable: Vec::new(),
            block: Vec::new(),
        }
    }
}

impl Desired {
    /// 从 YAML 文本解析并校验。
    pub fn from_yaml(text: &str) -> Result<Self> {
        let d: Desired = serde_yaml::from_str(text)?;
        d.validate()?;
        Ok(d)
    }

    /// 载入（没有这份文件就返回 None——对账是可选功能，老项目不受影响）。
    pub fn load_optional(project_root: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = project_root.as_ref().join(DESIRED_FILE);
        match tx::read_optional(&path)? {
            None => Ok(None),
            Some(text) => Ok(Some(Self::from_yaml(&text)?)),
        }
    }

    pub fn save(&self, project_root: impl AsRef<Path>) -> Result<()> {
        self.validate()?;
        let path = project_root.as_ref().join(DESIRED_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        tx::write_atomic_str(&path, &serde_yaml::to_string(self)?)
    }

    /// 校验：install 要写 id、id 不能重复。
    pub fn validate(&self) -> Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        for w in &self.install {
            if w.id.trim().is_empty() {
                return Err(CercisError::Invalid(
                    "desired.yaml 里有一条 install 没写 id".into(),
                ));
            }
            if !ids.insert(w.id.clone()) {
                return Err(CercisError::Invalid(format!(
                    "desired.yaml 里插件 id 重复：{}",
                    w.id
                )));
            }
        }
        Ok(())
    }

    pub fn path(project_root: impl AsRef<Path>) -> PathBuf {
        project_root.as_ref().join(DESIRED_FILE)
    }
}

/// 对账结果（只算账，不动任何文件）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DesiredDiff {
    /// 想要但还没装的（要 app 下载后装，或 `--from <目录>` 本地装）
    pub missing: Vec<WantedPlugin>,
    /// 装了但不在期望里的（市场装的可以删；随包插件只能停用/屏蔽）
    pub extra: Vec<String>,
    /// 期望停用但当前没停用的
    pub to_disable: Vec<String>,
    /// 期望启用但当前停用了的
    pub to_enable: Vec<String>,
    /// 期望屏蔽但当前没屏蔽的
    pub to_block: Vec<String>,
    /// 期望取消屏蔽但当前屏蔽着的
    pub to_unblock: Vec<String>,
    /// 其他要提醒的（例如某插件是随包插件、不能删）
    pub notes: Vec<String>,
}

impl DesiredDiff {
    /// 有没有需要做的事。
    pub fn is_empty(&self) -> bool {
        self.missing.is_empty()
            && self.extra.is_empty()
            && self.to_disable.is_empty()
            && self.to_enable.is_empty()
            && self.to_block.is_empty()
            && self.to_unblock.is_empty()
    }

    /// 只剩"改状态"这类内核自己能做完的事（不含装/删）。
    pub fn only_state_changes(&self) -> bool {
        self.missing.is_empty() && self.extra.is_empty()
    }
}

/// 对账：比较"期望状态"与"实际状态"。
///
/// - `installs`：市场安装台账（"装了哪些"）
/// - `state`：项目里的停用/屏蔽名单
/// - `known`：项目里能发现到的插件 id（用来判断"想装的那个其实已经在了"，例如随包插件）
pub fn diff(
    desired: &Desired,
    installs: &Installs,
    state: &PluginState,
    known: &[String],
) -> DesiredDiff {
    let mut out = DesiredDiff::default();

    // 1) 想装但没装
    for want in &desired.install {
        let installed = installs.get(&want.id).is_some();
        let present = known.iter().any(|k| k == &want.id);
        if !installed && !present {
            out.missing.push(want.clone());
        } else if !installed && present {
            out.notes.push(format!(
                "{} 已经在项目里（可能是随包插件或手动放的），不必再从市场装",
                want.id
            ));
        }
    }

    // 2) 装了但不在期望里
    for record in &installs.records {
        if desired.install.iter().any(|w| w.id == record.id) {
            continue;
        }
        if record.bundled {
            out.notes.push(format!(
                "{} 是随 app 交付的基础插件，不删除；不想要就把它写进 disable 或 block",
                record.id
            ));
        } else {
            out.extra.push(record.id.clone());
        }
    }

    // 3) 停用 / 启用 / 屏蔽
    let wanted_disable: std::collections::BTreeSet<&str> =
        desired.disable.iter().map(|s| s.as_str()).collect();
    let wanted_block: std::collections::BTreeSet<&str> =
        desired.block.iter().map(|s| s.as_str()).collect();

    for id in &desired.disable {
        if !state.is_disabled(id) {
            out.to_disable.push(id.clone());
        }
    }
    for id in &desired.block {
        if !state.is_blocked(id) {
            out.to_block.push(id.clone());
        }
    }
    for id in state.disabled.iter().filter(|id| {
        !wanted_disable.contains(id.as_str()) && !wanted_block.contains(id.as_str())
    }) {
        out.to_enable.push(id.clone());
    }
    for id in state
        .blocked
        .iter()
        .filter(|id| !wanted_block.contains(id.as_str()))
    {
        out.to_unblock.push(id.clone());
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::InstallRecord;

    fn rec(id: &str, bundled: bool) -> InstallRecord {
        InstallRecord {
            id: id.into(),
            bundled,
            permissions: "读取数据".into(),
            market: "research".into(),
            repository: "repo".into(),
            git_ref: "main".into(),
            version: "0.1.0".into(),
            fingerprint: "f".into(),
            path: id.into(),
            installed_at: "t".into(),
        }
    }

    fn installs(ids: &[(&str, bool)]) -> Installs {
        Installs {
            schema: "cercis.installs/1".into(),
            records: ids.iter().map(|(id, b)| rec(id, *b)).collect(),
        }
    }

    #[test]
    fn yaml_roundtrip_and_validation() {
        let d = Desired::from_yaml(
            "schema: cercis.desired/1\ninstall:\n  - {market: research, id: research_io, ref: main}\ndisable: [research_scribe]\nblock: []\n",
        )
        .unwrap();
        assert_eq!(d.install.len(), 1);
        assert_eq!(d.install[0].id, "research_io");
        assert_eq!(d.disable, vec!["research_scribe".to_string()]);
        assert!(Desired::from_yaml("install:\n  - {id: a}\n  - {id: a}\n").is_err());
        assert!(Desired::from_yaml("install:\n  - {market: research}\n").is_err());
    }

    #[test]
    fn diff_reports_missing_extra_and_state_changes() {
        let desired = Desired::from_yaml(
            "install:\n  - {market: research, id: research_io}\n  - {market: research, id: research_scribe}\ndisable: [research_scribe]\nblock: [research_template]\n",
        )
        .unwrap();
        let inst = installs(&[
            ("research_io", false),
            ("research_old", false),
            ("health_check", true),
        ]);
        let mut state = PluginState::default();
        state.disable("research_template");
        let known = vec!["research_io".to_string(), "health_check".to_string()];

        let d = diff(&desired, &inst, &state, &known);
        assert_eq!(
            d.missing.iter().map(|w| w.id.as_str()).collect::<Vec<_>>(),
            vec!["research_scribe"],
            "想装但没装的要列出来"
        );
        assert_eq!(d.extra, vec!["research_old".to_string()], "多出来的市场插件");
        assert!(
            d.notes.iter().any(|n| n.contains("基础插件")),
            "随包插件不删，要提醒：{:?}",
            d.notes
        );
        assert!(d.to_block.contains(&"research_template".to_string()));
        assert!(d.to_disable.contains(&"research_scribe".to_string()));
        assert!(d.to_enable.is_empty());
    }

    #[test]
    fn diff_finds_nothing_when_aligned() {
        let desired =
            Desired::from_yaml("install:\n  - {id: research_io}\ndisable: [research_io]\n").unwrap();
        let inst = installs(&[("research_io", false)]);
        let mut state = PluginState::default();
        state.disable("research_io");
        let known = vec!["research_io".to_string()];
        let d = diff(&desired, &inst, &state, &known);
        assert!(d.is_empty(), "{d:?}");
        assert!(d.only_state_changes());
    }

    #[test]
    fn diff_wants_to_enable_what_is_not_in_desired() {
        let desired = Desired::default();
        let inst = installs(&[]);
        let mut state = PluginState::default();
        state.disable("something");
        let d = diff(&desired, &inst, &state, &[]);
        assert_eq!(d.to_enable, vec!["something".to_string()]);
    }
}
