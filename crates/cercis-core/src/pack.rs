//! 打包与合并（S12 / FR-I01–FR-I03）。
//!
//! 场景（见需求文档 §12.1–12.2）：没有 NAS 或临时应急时，把项目**打包**带走
//! （微信/网盘/邮件），在另一台设备上**合并**回来。
//!
//! ## 两条硬规则
//! 1. **数据只增不改**：合并是**并集**——两台设备各自新增的记录都保留；
//!    同一条记录被两边改成不同样子时，两份都留（记进报告），让人来选。
//! 2. **不动别人的文件**：接收方本地已有的文件**永不覆盖**；两边的"可变文件"
//!    （配置、模板、文档正文、媒体）不一致时，把对方那份写成 `<名字>.conflict-…`
//!    副本留在旁边，并进报告。
//!
//! ## 包长什么样（不用本工具也能看懂）
//! ```text
//! 我的包/
//!   pack.json            清单：格式版本、水位、每个文件的 sha256/大小/所在卷
//!   part-001/…           第 1 卷（默认每卷 20 MB，适配微信）
//!   part-002/…
//! ```
//! 卷里就是项目文件的**原样副本**（相对路径不变）：CSV 还是 CSV、照片还是照片；
//! **加密区域里的文件保持密文**（`.enc` 原样搬运，打包不解密）。
//!
//! ## 增量
//! 给定"上一次的 `pack.json`"（`--since`），只装变化的文件；被删掉的文件记在
//! `removed` 里**只报告、不代删**（接收方自己决定）。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::history::ChangeEntry;
use crate::store::{Identity, Store};
use crate::{CercisError, Result};

/// 清单格式版本。
pub const PACK_FORMAT: &str = "cercis-pack/1";
/// 默认每卷大小（20 MB，适配微信）。
pub const DEFAULT_PART_SIZE: u64 = 20 * 1024 * 1024;
/// 清单文件名。
pub const PACK_MANIFEST: &str = "pack.json";

/// 打包时**不装**的东西：不是数据，要么能重建，要么是本机临时文件。
const EXCLUDE_PREFIXES: &[&str] = &[
    ".git/",
    "_labnote/cache/",
    "_labnote/thumbs/",
    "_labnote/trash/",
    "_labnote/runs/",
];
/// 文件名里带这些片段的一律不装（临时文件、冲突副本本身）。
const EXCLUDE_MARKERS: &[&str] = &[".cercis-tmp", ".conflict-"];

/// 包里的一个文件。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackFile {
    /// 项目内相对路径（`/` 分隔）
    pub path: String,
    /// 所在卷（目录名）
    pub part: String,
    pub bytes: u64,
    pub sha256: String,
    /// 加密区域里的密文（原样搬运，不解密）
    #[serde(default)]
    pub encrypted: bool,
}

/// 变更水位：用来算增量、也用来对账"这一包是哪一版"。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Watermark {
    pub change_count: usize,
    #[serde(default)]
    pub last_change_at: String,
}

/// `pack.json`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackManifest {
    pub format: String,
    pub project_name: String,
    pub project_id: String,
    pub created: String,
    pub core_version: String,
    /// 每卷上限（0 = 不分卷）
    pub part_size: u64,
    pub parts: Vec<String>,
    pub watermark: Watermark,
    /// 项目状态指纹（模板 + 术语 + 插件台账 + 加密声明）
    #[serde(default)]
    pub state_hash: String,
    /// 上一包清单的指纹（增量包才有）
    #[serde(default)]
    pub incremental_from: String,
    pub files: Vec<PackFile>,
    /// 自上一包以来被删掉的文件（**只报告，不代删**）
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl PackManifest {
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// 从目录（找 `<目录>/pack.json`）或直接的 `pack.json` 路径读取。
    pub fn load(from: impl AsRef<Path>) -> Result<Self> {
        let p = from.as_ref();
        let path = if p.is_dir() { p.join(PACK_MANIFEST) } else { p.to_path_buf() };
        let text = crate::tx::read_optional(&path)?.ok_or_else(|| {
            CercisError::NotFound(format!("找不到包清单：{}", path.display()))
        })?;
        let m: PackManifest = serde_json::from_str(&text)
            .map_err(|e| CercisError::Corrupt(format!("{} 解析失败: {e}", path.display())))?;
        if m.format != PACK_FORMAT {
            return Err(CercisError::Corrupt(format!(
                "包格式不认识：{}（本内核支持 {PACK_FORMAT}）",
                m.format
            )));
        }
        Ok(m)
    }

    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

/// 打包选项。
#[derive(Debug, Clone)]
pub struct PackOptions {
    /// 每卷上限（字节）；0 = 不分卷
    pub part_size: u64,
    /// 上一次的清单（增量打包）
    pub since: Option<PathBuf>,
    /// 装不装历史快照（`history/objects`）；默认装
    pub include_history: bool,
    /// 装不装检查点（`history/checkpoints`）；默认不装（与对象库重复）
    pub with_checkpoints: bool,
    pub dry_run: bool,
}

impl Default for PackOptions {
    fn default() -> Self {
        Self {
            part_size: DEFAULT_PART_SIZE,
            since: None,
            include_history: true,
            with_checkpoints: false,
            dry_run: false,
        }
    }
}

/// 打包结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackReport {
    pub out: String,
    pub dry_run: bool,
    pub parts: Vec<String>,
    pub files: usize,
    pub bytes: u64,
    /// 增量包里"没变、跳过"的文件数
    pub skipped_unchanged: usize,
    /// 增量包里"对方删掉了"的文件（只报告）
    pub removed: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub manifest: PackManifest,
}

/// 一张表的并集结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMerge {
    pub table: String,
    pub added_rows: usize,
    #[serde(default)]
    pub added_columns: Vec<String>,
    /// 同一个编号出现了两份不同内容（两份都留了，请人工核对）
    #[serde(default)]
    pub key_conflicts: Vec<String>,
    pub encrypted: bool,
}

/// 一处冲突：本地已有且与对方不同 → 对方那份留成副本。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub path: String,
    /// 对方那份写到哪儿去了
    pub wrote: String,
    pub reason: String,
}

/// 合并结果（三分类：新增 / 并集 / 冲突，外加拒绝与对方删除）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeReport {
    pub pack: String,
    pub into: String,
    pub dry_run: bool,
    pub project_name: String,
    pub verified: usize,
    pub added: Vec<String>,
    pub merged: Vec<TableMerge>,
    pub conflicts: Vec<Conflict>,
    pub unchanged: Vec<String>,
    /// 没动的东西 + 为什么（没解锁、格式不认识……）
    pub refused: Vec<String>,
    /// 对方删掉的文件（**没有替你删**）
    pub removed_reported: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub report_path: String,
}

impl MergeReport {
    pub fn added_rows_total(&self) -> usize {
        self.merged.iter().map(|m| m.added_rows).sum()
    }

    /// 一句话总结（命令行与流水都用它）。
    pub fn summary(&self) -> String {
        format!(
            "合并包：新增 {} 个文件、{} 张表并集（+{} 行）、{} 处冲突（双方副本都留着）、{} 处没动、{} 个对方删除",
            self.added.len(),
            self.merged.len(),
            self.added_rows_total(),
            self.conflicts.len(),
            self.refused.len(),
            self.removed_reported.len()
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str("# 合并报告\n\n");
        s.push_str(&format!("- 包：`{}`\n", self.pack));
        s.push_str(&format!("- 合并到：`{}`\n", self.into));
        s.push_str(&format!("- 项目：{}\n", self.project_name));
        s.push_str(&format!("- 校验通过的文件：{}\n", self.verified));
        if self.dry_run {
            s.push_str("- **这是试运行：磁盘上没有改动**\n");
        }
        s.push_str("\n## 新增（对方带来的新文件）\n\n");
        if self.added.is_empty() {
            s.push_str("（无）\n");
        }
        for p in &self.added {
            s.push_str(&format!("- `{p}`\n"));
        }
        s.push_str("\n## 并集合并（数据表：两边记录都留）\n\n");
        if self.merged.is_empty() {
            s.push_str("（无）\n");
        }
        for m in &self.merged {
            s.push_str(&format!(
                "- `{}`：新增 {} 行{}{}\n",
                m.table,
                m.added_rows,
                if m.added_columns.is_empty() {
                    String::new()
                } else {
                    format!("，新增列 {}", m.added_columns.join("、"))
                },
                if m.encrypted { "（加密区域，已用你的钥匙合并）" } else { "" }
            ));
            if !m.key_conflicts.is_empty() {
                s.push_str(&format!(
                    "  - ⚠ 同一个编号有两份不同内容（两份都留着，请人工核对）：{}\n",
                    m.key_conflicts.join("、")
                ));
            }
        }
        s.push_str("\n## 冲突（本地那份没动，对方那份留成副本）\n\n");
        if self.conflicts.is_empty() {
            s.push_str("（无）\n");
        }
        for c in &self.conflicts {
            s.push_str(&format!(
                "- `{}` → 对方那份：`{}`（{}）\n",
                c.path, c.wrote, c.reason
            ));
        }
        s.push_str("\n## 没动的\n\n");
        if self.unchanged.is_empty() {
            s.push_str("（无）\n");
        }
        for p in &self.unchanged {
            s.push_str(&format!("- `{p}`（两边一样）\n"));
        }
        for p in &self.refused {
            s.push_str(&format!("- `{p}`\n"));
        }
        if !self.removed_reported.is_empty() {
            s.push_str("\n## 对方删掉了（本工具**没有**替你删）\n\n");
            for p in &self.removed_reported {
                s.push_str(&format!("- `{p}`\n"));
            }
        }
        if !self.notes.is_empty() {
            s.push_str("\n## 说明\n\n");
            for n in &self.notes {
                s.push_str(&format!("- {n}\n"));
            }
        }
        s
    }
}

/// 合并选项。
#[derive(Debug, Clone, Default)]
pub struct MergeOptions {
    pub dry_run: bool,
    /// 用哪个成员的会话（合并加密区域里的表要用）
    pub member: Option<String>,
}

// —————————————————————————————— 打包 ——————————————————————————————

/// 把项目打包到 `out`（目录）。
pub fn pack(root: &Path, out: &Path, opts: &PackOptions) -> Result<PackReport> {
    if !root.join("project.yaml").exists() {
        return Err(CercisError::NotFound(format!(
            "不是 Cercis 项目（缺少 project.yaml）: {}",
            root.display()
        )));
    }
    let root_abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let out_abs = fs::canonicalize(out).unwrap_or_else(|_| out.to_path_buf());
    if out_abs.starts_with(&root_abs) {
        return Err(CercisError::Invalid(format!(
            "打包输出目录不能放在项目里面（否则会把包自己装进包里）：{}",
            out.display()
        )));
    }

    // 1) 找文件
    let mut files = Vec::new();
    walk(root, root, opts, &mut files);
    files.sort();
    files.dedup();

    // 2) 加密区域里的文件必须是密文（明文不许打包出门）
    let mut plaintext_leaks = Vec::new();
    for rel in &files {
        if rel.ends_with(crate::access::ENC_SUFFIX) {
            continue;
        }
        if crate::access::zone_covering(root, rel)?.is_some() {
            plaintext_leaks.push(rel.clone());
        }
    }
    if !plaintext_leaks.is_empty() {
        return Err(CercisError::Invalid(format!(
            "加密区域里还有明文文件，不能这样打包出门（会泄密）：{}\n\
             - 先加密：cercis share apply <项目>\n\
             - 或者把这些路径从加密区域里去掉",
            plaintext_leaks.join("、")
        )));
    }

    // 3) 增量：跟上一包的清单比
    let mut skipped_unchanged = 0usize;
    let mut removed: Vec<String> = Vec::new();
    let mut incremental_from = String::new();
    if let Some(since) = &opts.since {
        let prev = PackManifest::load(since)?;
        incremental_from = crypto::sha256_hex(serde_json::to_string(&prev)?.as_bytes());
        let prev_map: BTreeMap<String, String> = prev
            .files
            .iter()
            .map(|f| (f.path.clone(), f.sha256.clone()))
            .collect();
        let now: BTreeSet<String> = files.iter().cloned().collect();
        removed = prev_map
            .keys()
            .filter(|p| !now.contains(*p))
            .cloned()
            .collect();
        let mut kept = Vec::new();
        for rel in files {
            let hash = crate::history::ObjectStore::hash_file(root.join(&rel))?;
            match prev_map.get(&rel) {
                Some(old) if *old == hash => skipped_unchanged += 1,
                _ => kept.push(rel),
            }
        }
        files = kept;
    }

    // 4) 分卷（按累计大小）
    let parts = assign_parts(&files, root, opts.part_size)?;
    let part_names: Vec<String> = {
        let mut v: BTreeSet<String> = parts.values().cloned().collect();
        v.insert("part-001".to_string());
        v.into_iter().collect()
    };

    // 5) 落盘
    let mut entries = Vec::new();
    let mut total = 0u64;
    for rel in &files {
        let src = root.join(rel);
        let bytes = fs::read(&src)?;
        let sha = crypto::sha256_hex(&bytes);
        let part = parts.get(rel).cloned().unwrap_or_else(|| "part-001".into());
        let encrypted = rel.ends_with(crate::access::ENC_SUFFIX)
            || crate::access::zone_covering(root, rel)?.is_some();
        total += bytes.len() as u64;
        if !opts.dry_run {
            let dest = out.join(&part).join(rel);
            crate::tx::write_atomic(&dest, &bytes)?;
        }
        entries.push(PackFile {
            path: rel.clone(),
            part,
            bytes: bytes.len() as u64,
            sha256: sha,
            encrypted,
        });
    }

    // 6) 清单
    let manifest_src = crate::Manifest::generate(root)?;
    let mut notes = Vec::new();
    if !opts.include_history {
        notes.push("按要求没有装历史快照（history/objects）".to_string());
    }
    if opts.with_checkpoints {
        notes.push("按要求装了检查点（history/checkpoints）".to_string());
    }
    if entries.iter().any(|f| f.encrypted) {
        notes.push(format!(
            "包里有 {} 个加密文件（密文原样搬运，打包不解密）——接收方要有授权才能读",
            entries.iter().filter(|f| f.encrypted).count()
        ));
    }
    if !removed.is_empty() {
        notes.push(format!(
            "相比上一包，有 {} 个文件在源项目里不见了（只记录，不代删）",
            removed.len()
        ));
    }
    let manifest = PackManifest {
        format: PACK_FORMAT.to_string(),
        project_name: manifest_src.project_name.clone(),
        project_id: manifest_src.project_id.clone(),
        created: crate::ids::now_iso8601(),
        core_version: crate::CORE_VERSION.to_string(),
        part_size: opts.part_size,
        parts: part_names.clone(),
        watermark: Watermark {
            change_count: manifest_src.change_count,
            last_change_at: manifest_src.last_change_at.clone(),
        },
        state_hash: manifest_src.state_hash.clone(),
        incremental_from,
        files: entries.clone(),
        removed: removed.clone(),
        notes: notes.clone(),
    };
    if !opts.dry_run {
        let manifest_path = out.join(PACK_MANIFEST);
        crate::tx::write_atomic_str(&manifest_path, &manifest.to_json()?)?;
        // 顺手放一份给人看的说明
        crate::tx::write_atomic_str(&out.join("README.md"), &pack_readme(&manifest))?;
    }

    Ok(PackReport {
        out: out.display().to_string(),
        dry_run: opts.dry_run,
        parts: part_names,
        files: entries.len(),
        bytes: total,
        skipped_unchanged,
        removed,
        notes,
        manifest,
    })
}

fn pack_readme(m: &PackManifest) -> String {
    let mut s = String::new();
    s.push_str(&format!("# Cercis 搬运包：{}\n\n", m.project_name));
    s.push_str(&format!("- 打包时间：{}\n", m.created));
    s.push_str(&format!("- 卷数：{}（每卷上限 {} 字节）\n", m.parts.len(), m.part_size));
    s.push_str(&format!("- 文件数：{}（共 {} 字节）\n", m.files.len(), m.total_bytes()));
    s.push_str(&format!(
        "- 变更水位：{} 条（最后一条 {}）\n",
        m.watermark.change_count, m.watermark.last_change_at
    ));
    s.push_str(&format!("- 项目状态指纹：`{}`\n", m.state_hash));
    if !m.removed.is_empty() {
        s.push_str(&format!("\n被删掉的文件（{} 个，合并时只报告不代删）：\n", m.removed.len()));
        for p in &m.removed {
            s.push_str(&format!("- `{p}`\n"));
        }
    }
    s.push_str("\n## 怎么用\n\n");
    s.push_str("```bash\n# 在另一台设备上（把收到的所有 part-* 目录与 pack.json 放在同一个文件夹里）\ncercis merge <这个包目录> --into <那边的项目目录>\n```\n\n");
    s.push_str("合并是**并集**：两边的记录都保留；同一个文件两边不一样时，本地那份不动，对方那份留成 `.conflict-…` 副本。\n");
    if m.files.iter().any(|f| f.encrypted) {
        s.push_str("\n> 包里有加密区域的文件（`.enc` 密文）：接收方需要是授权成员并先解锁，才能合并这些表。\n");
    }
    s
}

/// 按累计大小分卷（`part_size = 0` 表示整包一卷）。
fn assign_parts(
    files: &[String],
    root: &Path,
    part_size: u64,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    if part_size == 0 {
        for rel in files {
            out.insert(rel.clone(), "part-001".to_string());
        }
        return Ok(out);
    }
    let mut idx = 1usize;
    let mut used = 0u64;
    for rel in files {
        let size = fs::metadata(root.join(rel))?.len();
        // 单个文件就超一卷时，让它独占一卷（不切文件，保证每份文件都是完整可读的）
        if used > 0 && used + size > part_size {
            idx += 1;
            used = 0;
        }
        out.insert(rel.clone(), format!("part-{idx:03}"));
        used += size;
    }
    Ok(out)
}

/// 递归收集要打包的文件（相对路径，`/` 分隔）。
fn walk(root: &Path, dir: &Path, opts: &PackOptions, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if path.is_dir() {
            if is_excluded(&format!("{rel}/"), opts) {
                continue;
            }
            walk(root, &path, opts, out);
        } else if !is_excluded(&rel, opts) {
            let _ = name;
            out.push(rel);
        }
    }
}

fn is_excluded(rel: &str, opts: &PackOptions) -> bool {
    if EXCLUDE_PREFIXES.iter().any(|p| rel.starts_with(p)) {
        return true;
    }
    if rel == "_labnote/" || rel.starts_with("_labnote/") {
        // 缓存类已排除；其余（manifest.json、技能包）可以装
    }
    if rel.ends_with('/') && rel == ".git/" {
        return true;
    }
    if !opts.include_history && rel.starts_with("history/objects/") {
        return true;
    }
    if !opts.with_checkpoints && rel.starts_with("history/checkpoints/") {
        return true;
    }
    if rel.ends_with("/") {
        return false;
    }
    if rel.rsplit('/').next().map(|n| n == ".DS_Store").unwrap_or(false) {
        return true;
    }
    EXCLUDE_MARKERS.iter().any(|m| rel.contains(m))
}

// —————————————————————————————— 合并 ——————————————————————————————

/// 把包合并进 `into` 指向的项目（并集；本地已有文件永不覆盖）。
pub fn merge(pack_dir: &Path, into: &Path, opts: &MergeOptions) -> Result<MergeReport> {
    let manifest = PackManifest::load(pack_dir)?;
    if !into.join("project.yaml").exists() {
        return Err(CercisError::NotFound(format!(
            "不是 Cercis 项目（缺少 project.yaml）: {}",
            into.display()
        )));
    }

    // 1) 先校验（FR-I03）：一个文件对不上就整包拒绝，不猜
    let mut bad = Vec::new();
    for f in &manifest.files {
        let p = pack_dir.join(&f.part).join(&f.path);
        match fs::read(&p) {
            Ok(bytes) => {
                let sha = crypto::sha256_hex(&bytes);
                if sha != f.sha256 {
                    bad.push(format!("{}（指纹不符：文件可能损坏或被改过）", f.path));
                } else if bytes.len() as u64 != f.bytes {
                    bad.push(format!("{}（大小不符）", f.path));
                }
            }
            Err(_) => bad.push(format!("{}（找不到：{}）", f.path, p.display())),
        }
    }
    if !bad.is_empty() {
        return Err(CercisError::Corrupt(format!(
            "包校验没过，已停止合并（一个文件都不要动）：\n- {}",
            bad.join("\n- ")
        )));
    }

    let mut rep = MergeReport {
        pack: pack_dir.display().to_string(),
        into: into.display().to_string(),
        dry_run: opts.dry_run,
        project_name: manifest.project_name.clone(),
        verified: manifest.files.len(),
        added: Vec::new(),
        merged: Vec::new(),
        conflicts: Vec::new(),
        unchanged: Vec::new(),
        refused: Vec::new(),
        removed_reported: manifest.removed.clone(),
        notes: Vec::new(),
        report_path: String::new(),
    };

    let template = load_template(into);
    let stamp = crate::ids::timestamp_for_path(chrono::Utc::now());
    let mut table_log: Vec<(String, usize)> = Vec::new();

    for f in &manifest.files {
        let rel = f.path.clone();
        let incoming = fs::read(pack_dir.join(&f.part).join(&rel))?;
        let local_path = into.join(&rel);
        let local = fs::read(&local_path).ok();

        match local {
            None => {
                // 新增：直接放过去
                rep.added.push(rel.clone());
                if !opts.dry_run {
                    if let Some(parent) = local_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    crate::tx::write_atomic(&local_path, &incoming)?;
                }
                continue;
            }
            Some(local_bytes) if local_bytes == incoming => {
                rep.unchanged.push(rel.clone());
                continue;
            }
            Some(local_bytes) => {
                // 两边都有且不一样
                if is_table_path(&rel) {
                    match merge_table(
                        into,
                        &rel,
                        &local_bytes,
                        &incoming,
                        &template,
                        opts,
                    ) {
                        Ok(Some((bytes, outcome))) => {
                            if !opts.dry_run {
                                write_table_bytes(into, &rel, &bytes, opts)?;
                            }
                            table_log.push((outcome.table.clone(), outcome.added_rows));
                            rep.merged.push(outcome);
                        }
                        Ok(None) => continue, // 两边内容等价（比如行序不同）
                        Err(e) => rep.refused.push(format!("`{rel}`：{e}")),
                    }
                } else {
                    // 可变文件：本地不动，对方那份留成副本
                    let short = crypto::sha256_hex(manifest.created.as_bytes());
                    let conflict_rel = format!(
                        "{rel}.conflict-{stamp}-{}",
                        &short[..6]
                    );
                    rep.conflicts.push(Conflict {
                        path: rel.clone(),
                        wrote: conflict_rel.clone(),
                        reason: "两边都改过这个文件：本地那份没动，对方那份留成副本，请人工挑一份"
                            .to_string(),
                    });
                    if !opts.dry_run {
                        let dest = into.join(&conflict_rel);
                        if let Some(parent) = dest.parent() {
                            fs::create_dir_all(parent)?;
                        }
                        crate::tx::write_atomic(&dest, &incoming)?;
                    }
                }
            }
        }
    }

    // 2) 报告 + 流水（试运行不写）
    if !opts.dry_run {
        let dir = into.join("_labnote");
        fs::create_dir_all(&dir)?;
        let md_path = dir.join(format!("merge-report-{stamp}.md"));
        crate::tx::write_atomic_str(&md_path, &rep.to_markdown())?;
        let json_path = dir.join(format!("merge-report-{stamp}.json"));
        crate::tx::write_atomic_str(&json_path, &serde_json::to_string_pretty(&rep)?)?;
        rep.report_path = md_path.display().to_string();

        let store = Store::new(
            into,
            Identity {
                user_id: opts
                    .member
                    .clone()
                    .unwrap_or_else(|| "local-user".to_string()),
                device_id: "local-device".to_string(),
                record_device: false,
            },
        );
        let detail = table_log
            .iter()
            .map(|(t, n)| format!("{t}+{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        let summary = format!(
            "{}｜{}{}",
            rep.summary(),
            if detail.is_empty() { String::new() } else { format!("｜表：{detail}") },
            if manifest.incremental_from.is_empty() {
                String::new()
            } else {
                format!("｜增量包（源水位 {} 条）", manifest.watermark.change_count)
            }
        );
        match store.append_change(&ChangeEntry::new(
            &store.identity.user_id,
            &store.identity.device_for_log(),
            "merge",
            "pack",
            &manifest.project_id,
            &summary,
            "",
            &manifest.state_hash,
        )) {
            Ok(()) => {}
            Err(e) => rep.notes.push(format!("流水没写成功（不影响数据）：{e}")),
        }
    }

    Ok(rep)
}

fn is_table_path(rel: &str) -> bool {
    if let Some(stem) = rel.strip_prefix("tables/") {
        return stem.ends_with(".csv") || stem.ends_with(".csv.enc");
    }
    false
}

/// 表名（`tables/observations.csv` / `.csv.enc` → `observations`）。
fn table_name_of(rel: &str) -> Option<String> {
    let stem = rel.strip_prefix("tables/")?;
    let stem = stem.strip_suffix(".enc").unwrap_or(stem);
    stem.strip_suffix(".csv").map(|s| s.to_string())
}

/// 并集合并一张表。返回 `None` 表示两边其实等价（不用写）。
fn merge_table(
    into: &Path,
    rel: &str,
    local_bytes: &[u8],
    incoming_bytes: &[u8],
    template: &Option<crate::Template>,
    opts: &MergeOptions,
) -> Result<Option<(Vec<u8>, TableMerge)>> {
    let table = table_name_of(rel)
        .ok_or_else(|| CercisError::Invalid("不是表文件".to_string()))?;
    let encrypted = rel.ends_with(crate::access::ENC_SUFFIX);

    // 加密区域里的表：先解密（要用你自己的钥匙）
    let (local_text, incoming_text) = if encrypted {
        let zone = crate::access::zone_covering(into, &format!("tables/{table}.csv"))?
            .ok_or_else(|| {
                CercisError::Invalid(format!(
                    "`tables/{table}.csv.enc` 是密文，但本项目的加密声明里找不到它（声明被改过？）"
                ))
            })?;
        let key = crate::access::key_for_zone(into, &zone.name, opts.member.as_deref())
            .map_err(|e| {
                CercisError::Invalid(format!(
                    "这张表在加密区域 {} 里，需要先解锁才能合并（并集要看得懂内容）：{e}",
                    zone.display()
                ))
            })?;
        (
            String::from_utf8(crate::access::decrypt_bytes(&key, local_bytes)?)
                .map_err(|e| CercisError::Corrupt(format!("本地那份解密后不是 UTF-8: {e}")))?,
            String::from_utf8(crate::access::decrypt_bytes(&key, incoming_bytes)?)
                .map_err(|e| CercisError::Corrupt(format!("对方那份解密后不是 UTF-8: {e}")))?,
        )
    } else {
        (
            String::from_utf8(local_bytes.to_vec())
                .map_err(|e| CercisError::Corrupt(format!("本地那份不是 UTF-8: {e}")))?,
            String::from_utf8(incoming_bytes.to_vec())
                .map_err(|e| CercisError::Corrupt(format!("对方那份不是 UTF-8: {e}")))?,
        )
    };

    let path = into.join(&format!("tables/{table}.csv"));
    let local = crate::table::Table::from_csv_text(&path, &local_text)?;
    let remote = crate::table::Table::from_csv_text(&path, &incoming_text)?;

    // 列：本地顺序为主，对方多出来的列追加
    let mut columns = local.columns.clone();
    let mut added_columns = Vec::new();
    for c in &remote.columns {
        if !columns.contains(c) {
            columns.push(c.clone());
            added_columns.push(c.clone());
        }
    }

    let pk = primary_key_for(template, &table);
    let mut out_rows = local.rows.clone();
    let mut index: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, row) in out_rows.iter().enumerate() {
        index.entry(row_key(row, &pk, &columns)).or_default().push(i);
    }

    let mut added_rows = 0usize;
    let mut key_conflicts = Vec::new();
    for row in &remote.rows {
        let key = row_key(row, &pk, &columns);
        match index.get(&key) {
            Some(idxs) => {
                let same = idxs
                    .iter()
                    .any(|&i| rows_equal(&out_rows[i], row, &columns));
                if same {
                    continue; // 两边都有这一条 → 忽略
                }
                // 同一个编号、不同内容 → 两份都留，记冲突
                key_conflicts.push(if pk.is_empty() {
                    format!("（内容不同，无主键无法定位）")
                } else {
                    key.clone()
                });
            }
            None => {}
        }
        out_rows.push(row.clone());
        index.entry(key).or_default().push(out_rows.len() - 1);
        added_rows += 1;
    }

    if added_rows == 0 && added_columns.is_empty() {
        return Ok(None);
    }

    let merged = crate::table::Table::in_memory(&path, &[]);
    let mut merged = merged;
    merged.columns = columns;
    merged.rows = out_rows;
    Ok(Some((
        merged.to_csv_text().into_bytes(),
        TableMerge {
            table,
            added_rows,
            added_columns,
            key_conflicts,
            encrypted,
        },
    )))
}

/// 一行在并集里的"身份"：有主键就用主键；没有就按整行内容（完全一样的行视为同一条）。
fn row_key(row: &crate::table::Row, pk: &str, columns: &[String]) -> String {
    if !pk.is_empty() {
        if let Some(v) = row.get(pk) {
            if !v.trim().is_empty() {
                return format!("{pk}={v}");
            }
        }
    }
    columns
        .iter()
        .map(|c| row.get(c).cloned().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

fn rows_equal(a: &crate::table::Row, b: &crate::table::Row, columns: &[String]) -> bool {
    columns
        .iter()
        .all(|c| a.get(c).cloned().unwrap_or_default() == b.get(c).cloned().unwrap_or_default())
}

fn primary_key_for(template: &Option<crate::Template>, table: &str) -> String {
    if let Some(t) = template {
        if let Some(td) = t.table(table) {
            if !td.primary_key.is_empty() {
                return td.primary_key.clone();
            }
        }
    }
    match table {
        "change_log" => "change_id".to_string(),
        "corrections" => "correction_id".to_string(),
        _ => String::new(),
    }
}

fn load_template(root: &Path) -> Option<crate::Template> {
    let text = crate::tx::read_optional(&root.join("template.yaml")).ok()??;
    crate::Template::from_yaml(&text).ok()
}

/// 写表：加密区域里走密文，否则普通原子写。
fn write_table_bytes(
    into: &Path,
    rel: &str,
    bytes: &[u8],
    opts: &MergeOptions,
) -> Result<()> {
    let table = table_name_of(rel).unwrap_or_default();
    let plain_rel = format!("tables/{table}.csv");
    match crate::access::write_through_zone(into, opts.member.as_deref(), &plain_rel, bytes)? {
        Some(_) => Ok(()),
        None => crate::tx::write_atomic(&into.join(&plain_rel), bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Identity;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cercis-pack-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn add_row(p: &crate::Project, id: &str) {
        let mut row = crate::table::Row::new();
        row.insert("record_id".into(), id.into());
        row.insert("notes".into(), format!("备注 {id}"));
        row.insert("value_num".into(), "1".into());
        p.store
            .append_row_extending("records", &[], row, id, "测试写入")
            .unwrap();
    }

    #[test]
    fn pack_then_merge_unions_rows() {
        let dir = tmp("union");
        // 甲机
        let a = dir.join("a");
        let pa = crate::Project::create(&a, "试验", Identity::default()).unwrap();
        add_row(&pa, "R-1");
        // 乙机：复制一份（模拟第二台设备）
        let b = dir.join("b");
        copy_dir(&a, &b);
        let pb = crate::Project::open(&b, None).unwrap();
        add_row(&pb, "R-2");

        // 乙机打包（增量：以甲机"上一次的包"为准）
        let out1 = dir.join("pack-a");
        fs::create_dir_all(&out1).unwrap();
        pack(&a, &out1, &PackOptions::default()).unwrap();
        let out2 = dir.join("pack-b");
        fs::create_dir_all(&out2).unwrap();
        let rep = pack(
            &b,
            &out2,
            &PackOptions {
                since: Some(out1.join(PACK_MANIFEST)),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rep.skipped_unchanged > 0, "增量包应跳过没变的文件");

        // 合并回甲机
        let merged = merge(&out2, &a, &MergeOptions::default()).unwrap();
        let records = merged
            .merged
            .iter()
            .find(|m| m.table == "records")
            .expect("records 表应做一次并集");
        assert_eq!(records.added_rows, 1);
        assert!(records.added_columns.is_empty());
        // 变更流水也要并起来（谁做了什么同样是对账依据）
        assert!(
            merged.merged.iter().any(|m| m.table == "change_log"),
            "流水表也应并集：{:?}",
            merged.merged.iter().map(|m| &m.table).collect::<Vec<_>>()
        );
        let table = crate::table::Table::load(a.join("tables/records.csv")).unwrap();
        let ids: Vec<String> = table
            .rows
            .iter()
            .map(|r| r.get("record_id").cloned().unwrap_or_default())
            .collect();
        assert!(ids.contains(&"R-1".to_string()) && ids.contains(&"R-2".to_string()));
        assert_eq!(table.rows.len(), 2, "并集不能把同一行装两遍");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_keeps_conflict_copies_for_docs() {
        let dir = tmp("conflict");
        let a = dir.join("a");
        let pa = crate::Project::create(&a, "试验", Identity::default()).unwrap();
        add_row(&pa, "R-1");
        let b = dir.join("b");
        copy_dir(&a, &b);

        // 两边各自改同一份文档
        fs::write(a.join("methods/记录/方法.md"), "甲机写的\n").unwrap();
        fs::write(b.join("methods/记录/方法.md"), "乙机写的\n").unwrap();

        let out = dir.join("pack-b");
        fs::create_dir_all(&out).unwrap();
        pack(&b, &out, &PackOptions::default()).unwrap();
        let rep = merge(&out, &a, &MergeOptions::default()).unwrap();

        assert_eq!(rep.conflicts.len(), 1);
        assert_eq!(
            fs::read_to_string(a.join("methods/记录/方法.md")).unwrap(),
            "甲机写的\n",
            "本地那份永远不被覆盖"
        );
        let conflict = a.join(&rep.conflicts[0].wrote);
        assert!(conflict.exists(), "对方那份应留成副本");
        assert_eq!(fs::read_to_string(&conflict).unwrap(), "乙机写的\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_pack_is_refused_before_touching_anything() {
        let dir = tmp("corrupt");
        let a = dir.join("a");
        let pa = crate::Project::create(&a, "试验", Identity::default()).unwrap();
        add_row(&pa, "R-1");
        let out = dir.join("pack");
        fs::create_dir_all(&out).unwrap();
        pack(&a, &out, &PackOptions::default()).unwrap();

        // 把包里的一个文件改坏
        let target = out.join("part-001/tables/records.csv");
        let mut bytes = fs::read(&target).unwrap();
        bytes.push(b'\n');
        fs::write(&target, bytes).unwrap();

        let b = dir.join("b");
        copy_dir(&a, &b);
        fs::remove_file(b.join("tables/records.csv")).unwrap();
        let err = merge(&out, &b, &MergeOptions::default()).unwrap_err();
        assert!(err.to_string().contains("校验"), "{err}");
        assert!(
            !b.join("tables/records.csv").exists(),
            "校验没过时一个文件都不该动"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parts_split_by_size() {
        let dir = tmp("parts");
        let a = dir.join("a");
        let pa = crate::Project::create(&a, "试验", Identity::default()).unwrap();
        add_row(&pa, "R-1");
        let out = dir.join("pack");
        fs::create_dir_all(&out).unwrap();
        let rep = pack(
            &a,
            &out,
            &PackOptions {
                part_size: 200, // 200 字节一卷 → 必然分很多卷
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rep.parts.len() > 1, "应分成多卷");
        let back = PackManifest::load(&out).unwrap();
        assert_eq!(back.parts.len(), rep.parts.len());
        for f in &back.files {
            assert!(out.join(&f.part).join(&f.path).exists(), "每个文件都在它的卷里");
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zone_covered_plaintext_refuses_to_pack() {
        let dir = tmp("leak");
        let a = dir.join("a");
        let pa = crate::Project::create(&a, "试验", Identity::default()).unwrap();
        add_row(&pa, "R-1");
        std::env::set_var("CERCIS_SHARE_HOME", dir.join("share-home"));
        std::env::set_var("CERCIS_SHARE_KDF", "light");
        crate::access::create_member(
            &a,
            "alice",
            "alice",
            "pw",
            crate::crypto::KdfParams::light(),
            "",
        )
        .unwrap();
        // 只写声明、先不加密 → 打包必须拒绝
        crate::access::create_zone(
            &a,
            "secret",
            "机密",
            &["tables/records.csv".to_string()],
            &[("alice".to_string(), crate::Permission::Admin)],
            false,
            false,
        )
        .unwrap();
        let out = dir.join("pack");
        fs::create_dir_all(&out).unwrap();
        let err = pack(&a, &out, &PackOptions::default()).unwrap_err();
        assert!(err.to_string().contains("明文"), "{err}");
        std::env::remove_var("CERCIS_SHARE_HOME");
        std::env::remove_var("CERCIS_SHARE_KDF");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tmp("dry");
        let a = dir.join("a");
        let pa = crate::Project::create(&a, "试验", Identity::default()).unwrap();
        add_row(&pa, "R-1");
        let out = dir.join("pack");
        fs::create_dir_all(&out).unwrap();
        pack(
            &a,
            &out,
            &PackOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!out.join(PACK_MANIFEST).exists(), "试运行不该写清单");
        let _ = fs::remove_dir_all(&dir);
    }

    /// 简单递归复制（测试用；不处理符号链接）。
    fn copy_dir(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        let rd = fs::read_dir(from).unwrap();
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name();
            if name.to_string_lossy() == ".git" {
                continue;
            }
            let dest = to.join(&name);
            if p.is_dir() {
                copy_dir(&p, &dest);
            } else {
                fs::copy(&p, &dest).unwrap();
            }
        }
    }
}
