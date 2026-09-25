//! 插件注册台账（D37）：**谁往项目里放了什么，以及怎么撤回去**。
//!
//! ## 为什么要有它
//! 插件可以给项目加东西：一张表、一列、一个视图、一份术语包、一个缓存目录。
//! 以前这些"登记"没有任何记录，插件停用后就留下残渣（S6 的 F-05 只是最表面的一个）；
//! 更麻烦的是**没人说得清"这块是谁加的"**——体检只能说"列 X 不在模板里"。
//!
//! 借鉴 Cordis 的 revertible effects（每个改动都带一个 inverse，卸载时按 LIFO 回滚），
//! 但按 Cercis 的规矩做了两处改造：
//!
//! 1. **inverse 落在磁盘上**：Cordis 把 inverse 放在运行时上下文里，进程一死就没了；
//!    Cercis 的真相在文件上，还要面对 NAS、离线、换设备，所以台账是一份项目文件
//!    （`plugins/registrations.json`），随时可查、可人工核对。
//! 2. **数据永远不回滚**：红线"数据只增不改"优先。台账只回滚**结构性与派生性的登记**；
//!    一旦登记背后已经有数据（表里有行、列里有值），回滚就**停下来交给人**，
//!    而不是替用户删数据——这在 Cordis 的说法里正是"边界外只能补偿、不能撤销"（§6.1）。
//!
//! ## 台账长什么样
//! 每条登记记：插件、种类、目标、详情、时间、状态（active / reverted / blocked）。
//! 回滚按**倒序**（LIFO）执行，与 Cordis 的 recovery 顺序一致。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::format;
use crate::table::Table;
use crate::template::Template;
use crate::tx;
use crate::{ids, CercisError, Result};

/// 台账文件（相对项目根）。
pub const REGISTRY_FILE: &str = "plugins/registrations.json";
/// 备份目录（术语包 / 模板的"上一份"，回滚时恢复用）。
pub const BACKUP_DIR: &str = "plugins/.registry";

/// 登记的种类：每种对应一个"怎么撤回去"的规则。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegKind {
    /// 新建了一张表（`target` = 表名）
    Table,
    /// 给某张表加了一列（`target` = `表名.列名`）
    Column,
    /// 新建了一个视图文件（`target` = 相对路径，如 `views/按处理.yaml`）
    View,
    /// 换了术语包（会备份上一份 `terms.yaml`）
    Terms,
    /// 换了模板（会备份上一份 `template.yaml`）
    Template,
    /// 在 `_labnote/` 下建了缓存或工作目录（`target` = 相对路径）
    Cache,
    /// 只是声明"我做过这件事"（没有可撤销的结构改动）
    Note,
}

impl RegKind {
    pub fn as_str(self) -> &'static str {
        match self {
            RegKind::Table => "table",
            RegKind::Column => "column",
            RegKind::View => "view",
            RegKind::Terms => "terms",
            RegKind::Template => "template",
            RegKind::Cache => "cache",
            RegKind::Note => "note",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "table" => Some(RegKind::Table),
            "column" => Some(RegKind::Column),
            "view" => Some(RegKind::View),
            "terms" => Some(RegKind::Terms),
            "template" => Some(RegKind::Template),
            "cache" => Some(RegKind::Cache),
            "note" => Some(RegKind::Note),
            _ => None,
        }
    }

    /// 人话描述"撤销时会做什么"。
    pub fn revert_hint(self) -> &'static str {
        match self {
            RegKind::Table => "删掉这张表（表里已有数据就停下来，交给人处理）",
            RegKind::Column => "删掉这一列（列里已有数据就停下来）",
            RegKind::View => "删掉这个视图文件（文件被手工改过就停下来）",
            RegKind::Terms => "恢复上一份术语包",
            RegKind::Template => "恢复上一份模板",
            RegKind::Cache => "删掉 `_labnote/` 下的这个目录或文件",
            RegKind::Note => "只把这条登记关掉（没有结构改动）",
        }
    }
}

/// 登记状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegState {
    /// 还在生效
    Active,
    /// 已撤回
    Reverted,
    /// 撤回时被拦下（背后有数据，或文件被手工改过）
    Blocked,
}

/// 一条登记。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registration {
    /// 登记编号（ULID，天然按时间排序，"倒序回滚"就靠它）
    pub id: String,
    /// 哪个插件登记的
    pub plugin: String,
    pub kind: RegKind,
    /// 目标（表名 / 表.列 / 相对路径 / 包名）
    pub target: String,
    /// 补充信息（类型、说明、来源会话、内容指纹……）
    #[serde(default)]
    pub detail: BTreeMap<String, String>,
    pub created_at: String,
    #[serde(default = "default_state")]
    pub state: RegState,
    #[serde(default)]
    pub reverted_at: String,
    /// 被拦下的原因（state = blocked 时非空）
    #[serde(default)]
    pub blocked_reason: String,
}

fn default_state() -> RegState {
    RegState::Active
}

/// 台账。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default = "default_schema")]
    pub schema: String,
    #[serde(default)]
    pub entries: Vec<Registration>,
}

fn default_schema() -> String {
    "cercis.registrations/1".to_string()
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema: default_schema(),
            entries: Vec::new(),
        }
    }
}

/// 回滚结果的一条。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevertItem {
    pub id: String,
    pub plugin: String,
    pub kind: String,
    pub target: String,
    /// planned / reverted / blocked
    pub outcome: String,
    pub message: String,
}

impl Registry {
    /// 载入台账（没有就是空台账——老项目不必先建文件）。
    pub fn load(project_root: impl AsRef<Path>) -> Result<Self> {
        let path = project_root.as_ref().join(REGISTRY_FILE);
        match tx::read_optional(&path)? {
            None => Ok(Self::default()),
            Some(text) => Ok(serde_json::from_str(&text)?),
        }
    }

    /// 保存台账（原子写）。
    pub fn save(&self, project_root: impl AsRef<Path>) -> Result<()> {
        let path = project_root.as_ref().join(REGISTRY_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        tx::write_atomic_str(&path, &serde_json::to_string_pretty(self)?)
    }

    /// 某个插件当前生效的登记。
    pub fn active_of(&self, plugin: &str) -> Vec<&Registration> {
        self.entries
            .iter()
            .filter(|e| e.plugin == plugin && e.state == RegState::Active)
            .collect()
    }

    /// 全部生效的登记。
    pub fn active(&self) -> Vec<&Registration> {
        self.entries
            .iter()
            .filter(|e| e.state == RegState::Active)
            .collect()
    }

    /// 被拦下的登记（体检报告用：这些需要人工处理）。
    pub fn blocked(&self) -> Vec<&Registration> {
        self.entries
            .iter()
            .filter(|e| e.state == RegState::Blocked)
            .collect()
    }

    /// 登记一条（只改内存；保存由调用方负责）。
    pub fn register(
        &mut self,
        plugin: &str,
        kind: RegKind,
        target: &str,
        detail: BTreeMap<String, String>,
    ) -> Result<Registration> {
        let id = ids::new_machine_id();
        self.register_with_id(plugin, kind, target, detail, &id)
    }

    /// 用指定编号登记。
    ///
    /// 为什么要能指定：术语包 / 模板这类"覆盖式"改动，要在改动**之前**先把上一份备份下来，
    /// 而备份文件的名字带登记编号——所以调用方先取号、备份，再拿同一个号登记。
    pub fn register_with_id(
        &mut self,
        plugin: &str,
        kind: RegKind,
        target: &str,
        detail: BTreeMap<String, String>,
        id: &str,
    ) -> Result<Registration> {
        if plugin.trim().is_empty() {
            return Err(CercisError::Invalid("登记必须写清是哪个插件".into()));
        }
        check_target(kind, target)?;
        // 同一个插件对同一个目标重复登记：只留最新一条，免得回滚做两遍
        if let Some(prev) = self.entries.iter_mut().find(|e| {
            e.plugin == plugin && e.kind == kind && e.target == target && e.state == RegState::Active
        }) {
            prev.detail = detail;
            prev.created_at = ids::now_iso8601();
            return Ok(prev.clone());
        }
        let reg = Registration {
            id: id.to_string(),
            plugin: plugin.to_string(),
            kind,
            target: target.trim().to_string(),
            detail,
            created_at: ids::now_iso8601(),
            state: RegState::Active,
            reverted_at: String::new(),
            blocked_reason: String::new(),
        };
        self.entries.push(reg.clone());
        Ok(reg)
    }

    /// 计划回滚：某个插件**倒序**的生效登记（只算账，不动文件）。
    pub fn plan_revert(&self, plugin: &str) -> Vec<Registration> {
        let mut list: Vec<Registration> = self.active_of(plugin).into_iter().cloned().collect();
        list.sort_by(|a, b| b.id.cmp(&a.id)); // ULID 大的先撤 = LIFO
        list
    }

    /// 执行回滚（`dry_run` = 只算账、不动文件、不改台账）。
    pub fn revert(
        &mut self,
        project_root: impl AsRef<Path>,
        plugin: &str,
        dry_run: bool,
    ) -> Result<Vec<RevertItem>> {
        let root = project_root.as_ref().to_path_buf();
        let plan = self.plan_revert(plugin);
        let mut out: Vec<RevertItem> = Vec::new();
        for reg in plan {
            // 用台账里的**最新**那条（前面的回滚可能刷新过它的指纹）
            let reg = self
                .entries
                .iter()
                .find(|e| e.id == reg.id)
                .cloned()
                .unwrap_or(reg);
            let (outcome, message) = if dry_run {
                ("planned".to_string(), describe_revert(&reg))
            } else {
                match revert_one(&root, &reg) {
                    Ok((msg, touched_template)) => {
                        if touched_template {
                            // 我们自己刚改了 template.yaml：把那一条模板登记的指纹刷新一下，
                            // 否则它稍后回滚时会误判成"文件被手工改过"而卡住
                            refresh_template_fingerprint(&mut self.entries, &root, &reg.plugin);
                        }
                        ("reverted".to_string(), msg)
                    }
                    Err(reason) => ("blocked".to_string(), reason),
                }
            };
            if !dry_run {
                if let Some(entry) = self.entries.iter_mut().find(|e| e.id == reg.id) {
                    match outcome.as_str() {
                        "reverted" => {
                            entry.state = RegState::Reverted;
                            entry.reverted_at = ids::now_iso8601();
                        }
                        "blocked" => {
                            entry.state = RegState::Blocked;
                            entry.blocked_reason = message.clone();
                        }
                        _ => {}
                    }
                }
            }
            out.push(RevertItem {
                id: reg.id.clone(),
                plugin: reg.plugin.clone(),
                kind: reg.kind.as_str().to_string(),
                target: reg.target.clone(),
                outcome,
                message,
            });
        }
        if !dry_run {
            self.save(&root)?;
        }
        Ok(out)
    }
}

/// 人话描述一条登记"撤销会做什么"（dry-run 用）。
fn describe_revert(reg: &Registration) -> String {
    match reg.kind {
        RegKind::Terms | RegKind::Template => {
            let prev = reg
                .detail
                .get("previous")
                .cloned()
                .unwrap_or_else(|| "（没有上一份）".into());
            format!("恢复上一份：{prev}")
        }
        _ => format!("{}：{}", reg.kind.revert_hint(), reg.target),
    }
}

/// 撤销一条登记。成功返回人话；被拦下返回原因（**不报错**——"拦下"是正常结果）。
type Reverted = (String, bool); // (人话, 是否改过 template.yaml)

fn revert_one(root: &Path, reg: &Registration) -> std::result::Result<Reverted, String> {
    match reg.kind {
        RegKind::Note => Ok(("只是声明性登记，已关闭".into(), false)),
        RegKind::Cache => {
            let rel = reg.target.trim_start_matches("./");
            if !rel.starts_with("_labnote/") {
                return Err(format!(
                    "为了安全，只删 `_labnote/` 下的东西；这条登记指向 {}",
                    reg.target
                ));
            }
            let path = root.join(rel);
            if !path.exists() {
                return Ok(("路径已不存在，无需处理".into(), false));
            }
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
            Ok((format!("已删掉 {}", reg.target), false))
        }
        RegKind::View => {
            let path = root.join(&reg.target);
            if !path.exists() {
                return Ok(("视图文件已不存在，无需处理".into(), false));
            }
            if let Some(want) = reg.detail.get("sha256") {
                if !want.is_empty() && &file_hash(&path) != want {
                    return Err(format!(
                        "{} 被手工改过（指纹不符），不敢删——确认后可自行删除，再 `plugins unregister --plugin {}`",
                        reg.target, reg.plugin
                    ));
                }
            }
            std::fs::remove_file(&path).map_err(|e| format!("删不掉 {}：{e}", reg.target))?;
            Ok((format!("已删掉视图 {}", reg.target), false))
        }
        RegKind::Terms | RegKind::Template => {
            let file = if reg.kind == RegKind::Terms {
                "terms.yaml"
            } else {
                "template.yaml"
            };
            let live = root.join(file);
            if let Some(want) = reg.detail.get("sha256") {
                if !want.is_empty() && live.exists() && &file_hash(&live) != want {
                    return Err(format!(
                        "{file} 在登记之后又被改过，不敢覆盖——确认后可自行处理，再 `plugins unregister --plugin {}`",
                        reg.plugin
                    ));
                }
            }
            match reg.detail.get("backup") {
                Some(b) if !b.is_empty() => {
                    let backup = root.join(b);
                    if !backup.exists() {
                        return Err(format!("备份文件丢了（{b}），无法恢复"));
                    }
                    std::fs::copy(&backup, &live).map_err(|e| format!("恢复 {file} 失败：{e}"))?;
                    Ok((format!("已恢复上一份 {file}"), false))
                }
                _ => {
                    if live.exists() {
                        std::fs::remove_file(&live)
                            .map_err(|e| format!("删掉 {file} 失败：{e}"))?;
                    }
                    Ok((format!("已删掉 {file}（登记之前本来就没有）"), false))
                }
            }
        }
        RegKind::Table => {
            let path = root.join("tables").join(format!("{}.csv", reg.target));
            if !path.exists() {
                return Ok(("表已不存在，无需处理".into(), false));
            }
            let table = Table::load(&path).map_err(|e| e.to_string())?;
            if !table.rows.is_empty() {
                return Err(format!(
                    "表 {} 里已有 {} 行数据——数据只增不改，不替你删（要留就保留，要清请手工处理）",
                    reg.target,
                    table.rows.len()
                ));
            }
            std::fs::remove_file(&path).map_err(|e| format!("删掉表文件失败：{e}"))?;
            let touched = remove_table_from_template(root, &reg.target);
            Ok((format!("已删掉空表 {}", reg.target), touched))
        }
        RegKind::Column => {
            let (table_name, column) = reg
                .target
                .split_once('.')
                .ok_or_else(|| format!("目标应写成 表名.列名，实际是 {}", reg.target))?;
            let path = root.join("tables").join(format!("{table_name}.csv"));
            if !path.exists() {
                return Ok(("表已不存在，无需处理".into(), false));
            }
            let table = Table::load(&path).map_err(|e| e.to_string())?;
            if !table.columns.iter().any(|c| c == column) {
                return Ok(("这一列已不存在，无需处理".into(), false));
            }
            let has_data = table
                .rows
                .iter()
                .any(|r| r.get(column).map(|v| !v.trim().is_empty()).unwrap_or(false));
            if has_data {
                return Err(format!(
                    "列 {} 里已经有数据——不替你删（数据只增不改）",
                    reg.target
                ));
            }
            let kept: Vec<&str> = table
                .columns
                .iter()
                .map(|c| c.as_str())
                .filter(|c| *c != column)
                .collect();
            let mut new_table = Table::in_memory(&path, &kept);
            for row in &table.rows {
                let mut r = row.clone();
                r.remove(column);
                new_table.push(r);
            }
            new_table.save().map_err(|e| e.to_string())?;
            let touched = remove_column_from_template(root, table_name, column);
            Ok((format!("已删掉空列 {}", reg.target), touched))
        }
    }
}

/// 从项目模板里去掉一列（模板不在、是主键、改了会校验不过 → 原样保留，让体检去报）。
fn remove_column_from_template(root: &Path, table: &str, column: &str) -> bool {
    let path = root.join("template.yaml");
    let Ok(Some(text)) = tx::read_optional(&path) else {
        return false;
    };
    let Ok(mut t) = Template::from_yaml(&text) else {
        return false;
    };
    let Some(td) = t.tables.iter_mut().find(|t| t.name == table) else {
        return false;
    };
    if td.primary_key == column {
        return false;
    }
    let before = td.fields.len();
    td.fields.retain(|f| f.name != column);
    if td.fields.len() == before || t.validate().is_err() {
        return false;
    }
    t.save(&path).is_ok()
}

/// 从模板里去掉一张表。
fn remove_table_from_template(root: &Path, table: &str) -> bool {
    let path = root.join("template.yaml");
    let Ok(Some(text)) = tx::read_optional(&path) else {
        return false;
    };
    let Ok(mut t) = Template::from_yaml(&text) else {
        return false;
    };
    let before = t.tables.len();
    t.tables.retain(|x| x.name != table);
    if t.tables.len() == before || t.validate().is_err() {
        return false;
    }
    t.save(&path).is_ok()
}

/// 把某个插件那条模板登记的指纹刷新成"现在这份"（我们自己改过它，不算被手工改）。
fn refresh_template_fingerprint(entries: &mut [Registration], root: &Path, plugin: &str) {
    let now = file_hash(&root.join("template.yaml"));
    for e in entries.iter_mut() {
        if e.plugin == plugin && e.kind == RegKind::Template && e.state == RegState::Active {
            e.detail.insert("sha256".to_string(), now.clone());
        }
    }
}

/// 文件内容指纹（用来判断"这个文件还是不是我写的那份"）。
pub fn file_hash(path: &Path) -> String {
    match std::fs::read(path) {
        Ok(bytes) => {
            let mut hasher = Sha256::new();
            hasher.update(&bytes);
            hex::encode(hasher.finalize())
        }
        Err(_) => String::new(),
    }
}

/// 备份一个文件到 `plugins/.registry/<id>.<name>`，返回（相对路径，当前指纹）。
///
/// 本来没有这个文件时返回空路径——回滚时把它删掉即可。
pub fn backup_for(
    project_root: impl AsRef<Path>,
    id: &str,
    file: &str,
) -> Result<(String, String)> {
    let root = project_root.as_ref();
    let live = root.join(file);
    let dir = root.join(BACKUP_DIR);
    std::fs::create_dir_all(&dir)?;
    if !live.exists() {
        std::fs::write(dir.join(format!("{id}.none")), b"")?;
        return Ok((String::new(), String::new()));
    }
    let name = Path::new(file)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let dest = dir.join(format!("{id}.{name}"));
    std::fs::copy(&live, &dest)?;
    let rel = dest
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| dest.to_string_lossy().to_string());
    Ok((rel, file_hash(&live)))
}

/// 登记目标的合法性（拦住手滑写出来的怪目标）。
pub fn check_target(kind: RegKind, target: &str) -> Result<()> {
    match kind {
        RegKind::Table => {
            if !format::is_ascii_snake(target) {
                return Err(CercisError::Invalid(format!(
                    "表名需为小写 ASCII 下划线形式: {target}"
                )));
            }
        }
        RegKind::Column => {
            let (t, c) = target.split_once('.').ok_or_else(|| {
                CercisError::Invalid(format!("列登记要写成 表名.列名，实际：{target}"))
            })?;
            if !format::is_ascii_snake(t) || !format::is_ascii_snake(c) {
                return Err(CercisError::Invalid(format!(
                    "表名与列名都要是小写 ASCII 下划线形式: {target}"
                )));
            }
        }
        RegKind::View | RegKind::Cache => {
            if target.starts_with('/') || target.contains("..") {
                return Err(CercisError::Invalid(format!(
                    "登记路径必须是项目内的相对路径，且不能含 `..`：{target}"
                )));
            }
            if kind == RegKind::View && !target.starts_with("views/") {
                return Err(CercisError::Invalid(format!(
                    "视图登记应写在 views/ 下：{target}"
                )));
            }
        }
        RegKind::Terms | RegKind::Template | RegKind::Note => {}
    }
    Ok(())
}

/// 台账文件路径。
pub fn registry_path(project_root: impl AsRef<Path>) -> PathBuf {
    project_root.as_ref().join(REGISTRY_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Row, Table};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cercis-registry-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("tables")).unwrap();
        dir
    }

    /// 建一张表（可带数据）。
    fn make_table(root: &Path, name: &str, columns: &[&str], rows: &[&[(&str, &str)]]) {
        let path = root.join("tables").join(format!("{name}.csv"));
        let mut t = Table::in_memory(&path, columns);
        for r in rows {
            let mut row = Row::new();
            for (k, v) in r.iter() {
                row.insert(k.to_string(), v.to_string());
            }
            t.push(row);
        }
        t.save().unwrap();
    }

    #[test]
    fn register_dedupes_and_lists_by_plugin() {
        let root = tmp("basic");
        let mut reg = Registry::load(&root).unwrap();
        reg.register("research_io", RegKind::Table, "extra_notes", BTreeMap::new())
            .unwrap();
        reg.register(
            "research_io",
            RegKind::Column,
            "observations.spad_note",
            BTreeMap::new(),
        )
        .unwrap();
        reg.register("other", RegKind::Note, "做过某件事", BTreeMap::new())
            .unwrap();
        // 重复登记同一目标：只留一条
        reg.register("research_io", RegKind::Table, "extra_notes", BTreeMap::new())
            .unwrap();
        reg.save(&root).unwrap();

        let back = Registry::load(&root).unwrap();
        assert_eq!(back.active_of("research_io").len(), 2);
        assert_eq!(back.active().len(), 3);
    }

    #[test]
    fn revert_is_lifo_and_removes_empty_structures() {
        let root = tmp("revert");
        make_table(&root, "extra", &["a", "b"], &[]);
        let mut reg = Registry::load(&root).unwrap();
        reg.register("p", RegKind::Table, "extra", BTreeMap::new())
            .unwrap();
        reg.register("p", RegKind::Note, "先装了什么", BTreeMap::new())
            .unwrap();

        let plan = reg.plan_revert("p");
        assert_eq!(plan[0].kind, RegKind::Note, "后登记的先撤（LIFO）");
        assert_eq!(plan[1].kind, RegKind::Table);

        let out = reg.revert(&root, "p", false).unwrap();
        assert!(out.iter().all(|i| i.outcome == "reverted"), "{out:?}");
        assert!(!root.join("tables/extra.csv").exists(), "空表应被删掉");
        assert!(reg.active_of("p").is_empty());
    }

    #[test]
    fn revert_stops_when_there_is_data() {
        let root = tmp("blocked");
        make_table(&root, "extra", &["a"], &[&[("a", "1")]]);
        let mut reg = Registry::load(&root).unwrap();
        reg.register("p", RegKind::Table, "extra", BTreeMap::new())
            .unwrap();
        let out = reg.revert(&root, "p", false).unwrap();
        assert_eq!(out[0].outcome, "blocked");
        assert!(out[0].message.contains("已有 1 行数据"), "{}", out[0].message);
        assert!(root.join("tables/extra.csv").exists(), "有数据的表必须留着");
        assert_eq!(reg.blocked().len(), 1);
    }

    #[test]
    fn revert_empty_column_but_not_filled_column() {
        let root = tmp("column");
        make_table(
            &root,
            "obs",
            &["obs_id", "spad_note"],
            &[&[("obs_id", "O-1"), ("spad_note", "")]],
        );
        let mut reg = Registry::load(&root).unwrap();
        reg.register("p", RegKind::Column, "obs.spad_note", BTreeMap::new())
            .unwrap();
        let out = reg.revert(&root, "p", false).unwrap();
        assert_eq!(out[0].outcome, "reverted", "{out:?}");
        let t = Table::load(root.join("tables/obs.csv")).unwrap();
        assert_eq!(t.columns, vec!["obs_id".to_string()]);

        let root2 = tmp("column-data");
        make_table(
            &root2,
            "obs",
            &["obs_id", "spad_note"],
            &[&[("obs_id", "O-1"), ("spad_note", "记了点东西")]],
        );
        let mut reg2 = Registry::load(&root2).unwrap();
        reg2.register("p", RegKind::Column, "obs.spad_note", BTreeMap::new())
            .unwrap();
        assert_eq!(reg2.revert(&root2, "p", false).unwrap()[0].outcome, "blocked");
    }

    #[test]
    fn revert_restores_previous_terms() {
        let root = tmp("terms");
        std::fs::write(root.join("terms.yaml"), "profile: generic\n").unwrap();
        let (backup, _hash) = backup_for(&root, "01AAA", "terms.yaml").unwrap();
        assert!(!backup.is_empty());
        std::fs::write(root.join("terms.yaml"), "profile: agriculture\n").unwrap();
        let mut detail = BTreeMap::new();
        detail.insert("backup".to_string(), backup);
        detail.insert("sha256".to_string(), file_hash(&root.join("terms.yaml")));
        detail.insert("previous".to_string(), "generic".to_string());
        let mut reg = Registry::load(&root).unwrap();
        reg.register("p", RegKind::Terms, "agriculture", detail)
            .unwrap();
        let out = reg.revert(&root, "p", false).unwrap();
        assert_eq!(out[0].outcome, "reverted", "{out:?}");
        let now = std::fs::read_to_string(root.join("terms.yaml")).unwrap();
        assert!(now.contains("generic"), "应恢复上一份：{now}");
    }

    #[test]
    fn revert_refuses_when_file_was_hand_edited() {
        let root = tmp("view");
        std::fs::create_dir_all(root.join("views")).unwrap();
        std::fs::write(root.join("views/按处理.yaml"), "query: 1").unwrap();
        let mut detail = BTreeMap::new();
        detail.insert(
            "sha256".to_string(),
            file_hash(&root.join("views/按处理.yaml")),
        );
        let mut reg = Registry::load(&root).unwrap();
        reg.register("p", RegKind::View, "views/按处理.yaml", detail)
            .unwrap();
        std::fs::write(root.join("views/按处理.yaml"), "query: 2（我改过）").unwrap();
        let out = reg.revert(&root, "p", false).unwrap();
        assert_eq!(out[0].outcome, "blocked");
        assert!(root.join("views/按处理.yaml").exists());
    }

    #[test]
    fn cache_revert_is_confined_to_labnote() {
        let root = tmp("cache");
        std::fs::create_dir_all(root.join("_labnote/cache/duckdb")).unwrap();
        std::fs::write(root.join("_labnote/cache/duckdb/x.bin"), b"x").unwrap();
        assert!(check_target(RegKind::Cache, "/etc/passwd").is_err());
        assert!(check_target(RegKind::Cache, "../外面").is_err());
        let mut reg = Registry::load(&root).unwrap();
        reg.register("p", RegKind::Cache, "_labnote/cache/duckdb", BTreeMap::new())
            .unwrap();
        let out = reg.revert(&root, "p", false).unwrap();
        assert_eq!(out[0].outcome, "reverted");
        assert!(!root.join("_labnote/cache/duckdb").exists());
    }

    #[test]
    fn dry_run_changes_nothing() {
        let root = tmp("dry");
        make_table(&root, "extra", &["a"], &[]);
        let mut reg = Registry::load(&root).unwrap();
        reg.register("p", RegKind::Table, "extra", BTreeMap::new())
            .unwrap();
        let out = reg.revert(&root, "p", true).unwrap();
        assert_eq!(out[0].outcome, "planned");
        assert!(root.join("tables/extra.csv").exists());
        assert_eq!(reg.active_of("p").len(), 1, "dry-run 不该改台账");
    }

    #[test]
    fn target_validation() {
        assert!(check_target(RegKind::Table, "good_name").is_ok());
        assert!(check_target(RegKind::Table, "Bad Name").is_err());
        assert!(check_target(RegKind::Column, "obs.spad_note").is_ok());
        assert!(check_target(RegKind::Column, "obs").is_err());
        assert!(check_target(RegKind::View, "views/x.yaml").is_ok());
        assert!(check_target(RegKind::View, "x.yaml").is_err());
        assert!(check_target(RegKind::Note, "随便写").is_ok());
    }
}
