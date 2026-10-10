//! NAS / 网络位置与工作副本（S14 内核段 / FR-N01–FR-N07）。
//!
//! 需求（§12.1）把跨设备通道分三条：**A. NAS 直连**、**B. NAS 工作副本**、C. 打包搬运。
//! C 已在 S12 做完（`pack` / `merge`）；本模块补 A 与 B，并把"网络目录体检"与
//! "凭据进钥匙串"一起落地。
//!
//! ## 三条规矩（写进代码里，不只是口号）
//! 1. **内核不联网、不挂载**：SMB / WebDAV / SFTP 由操作系统或 app 挂载成本地路径，
//!    内核只认"一个路径"。所以 `remote add` 记的是**挂载点**（如 `/Volumes/lab-nas`）。
//! 2. **凭据绝不进项目**：口令/密钥存系统钥匙串（`keychain.rs`），项目里只记
//!    "这个远程用哪个钥匙串账号"（`access/remotes.yaml`）。
//! 3. **工作副本 = 打包 + 合并**：不另造一套同步算法，双向同步就是
//!    "从主副本 `pack` 一份 → `merge` 进本地" ＋ "从本地 `pack` 一份 → `merge` 进主副本"，
//!    于是并集、冲突副本、校验、流水这些性质全都自动一致。
//!
//! ## 断网怎么办
//! 主副本不可达时 `workcopy sync` **明确报"离线"** 并告诉你怎么继续：本地照记，
//! 联网后再 sync。宁可停下，不产生半写文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{CercisError, Result};

/// 网络位置清单（随项目走，但**不含任何口令**）。
pub const REMOTES_REL: &str = "access/remotes.yaml";
/// 工作副本标记（放在**副本**里，指向主副本）。
pub const WORKCOPY_REL: &str = "_labnote/workcopy.yaml";

/// 一个网络位置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remote {
    /// 短名（如 `nas`、`lab`）
    pub name: String,
    /// 挂载方式：local / smb / webdav / sftp / nfs（用于提示与记录）
    #[serde(default)]
    pub kind: String,
    /// **挂载后的本地路径**（如 `/Volumes/lab-nas`）
    pub path: String,
    #[serde(default)]
    pub note: String,
    pub created: String,
    /// 钥匙串账号（口令本身不在这里）
    #[serde(default)]
    pub credential_account: String,
}

/// `access/remotes.yaml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Remotes {
    pub format: String,
    #[serde(default)]
    pub remotes: Vec<Remote>,
}

impl Default for Remotes {
    fn default() -> Self {
        Self {
            format: "cercis-remotes/1".to_string(),
            remotes: Vec::new(),
        }
    }
}

impl Remotes {
    pub fn load(root: &Path) -> Result<Self> {
        match crate::tx::read_optional(&root.join(REMOTES_REL))? {
            None => Ok(Self::default()),
            Some(text) => serde_yaml::from_str(&text)
                .map_err(|e| CercisError::Corrupt(format!("{REMOTES_REL} 解析失败: {e}"))),
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(REMOTES_REL);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::tx::write_atomic_str(&path, &serde_yaml::to_string(self)?)
    }

    pub fn get(&self, name: &str) -> Option<&Remote> {
        self.remotes.iter().find(|r| r.name == name)
    }
}

/// 网络目录体检结果（FR-N06）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteTest {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub reachable: bool,
    pub readable: bool,
    pub writable: bool,
    /// 这个路径本身是不是一个 Cercis 项目
    pub is_project: bool,
    /// 支持"先写临时文件再改名"（断线不产生半写文件的前提）
    pub atomic_rename: bool,
    /// 文件系统不区分大小写（macOS/Windows 默认，NAS 上常见）
    pub case_insensitive: bool,
    /// 能建中文名文件
    pub unicode_names: bool,
    /// 写 64 KB 用了多少毫秒
    pub write_ms: u128,
    #[serde(default)]
    pub findings: Vec<String>,
    /// 项目里已有的分叉迹象（云盘副本、残留临时文件）
    #[serde(default)]
    pub conflicts: Vec<String>,
}

impl RemoteTest {
    pub fn summary(&self) -> String {
        format!(
            "{}（{}）：{}｜读 {}｜写 {}｜原子改名 {}｜大小写{}敏感｜中文名 {}｜写 64KB {} ms",
            self.name,
            self.path,
            if self.reachable { "可达" } else { "不可达" },
            if self.readable { "✓" } else { "✗" },
            if self.writable { "✓" } else { "✗" },
            if self.atomic_rename { "✓" } else { "✗" },
            if self.case_insensitive { "不" } else { "" },
            if self.unicode_names { "✓" } else { "✗" },
            self.write_ms
        )
    }
}

/// 工作副本标记。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkCopy {
    pub format: String,
    /// 用哪个远程（`access/remotes.yaml` 里的名字；本机自测可用空串）
    #[serde(default)]
    pub remote: String,
    /// 主副本路径
    pub main: String,
    pub created: String,
    #[serde(default)]
    pub last_sync: String,
    /// 上次同步时两边的水位（用来算"各有多少新变更"）
    #[serde(default)]
    pub last_sync_main_changes: usize,
    #[serde(default)]
    pub last_sync_copy_changes: usize,
}

/// 工作副本状态（FR-N07 同步状态面板）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkCopyStatus {
    pub copy: String,
    pub main: String,
    pub remote: String,
    pub reachable: bool,
    pub last_sync: String,
    /// 本地（副本）自上次同步以来新增的流水条数
    pub local_changes: usize,
    /// 主副本自上次同步以来新增的流水条数（不可达时为 0）
    pub remote_changes: usize,
    /// 需要人处理的冲突副本
    pub conflicts: Vec<String>,
    /// `ok` / `needs-sync` / `offline`
    pub verdict: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl WorkCopyStatus {
    pub fn summary(&self) -> String {
        format!(
            "工作副本：{}｜主副本：{}（{}）｜上次同步 {}｜本地新变更 {}｜主副本新变更 {}｜冲突 {}",
            self.copy,
            self.main,
            if self.reachable { "可达" } else { "不可达" },
            if self.last_sync.is_empty() {
                "从未".to_string()
            } else {
                self.last_sync.clone()
            },
            self.local_changes,
            self.remote_changes,
            self.conflicts.len()
        )
    }
}

/// 双向同步结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkCopySync {
    pub copy: String,
    pub main: String,
    pub dry_run: bool,
    /// 主副本 → 本地：拉回来多少
    pub pulled: crate::pack::MergeReport,
    /// 本地 → 主副本：推上去多少
    pub pushed: crate::pack::MergeReport,
    pub conflicts_total: usize,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl WorkCopySync {
    pub fn summary(&self) -> String {
        format!(
            "双向同步{}：拉回来（新增 {} 文件 / +{} 行 / {} 冲突），推上去（新增 {} 文件 / +{} 行 / {} 冲突）",
            if self.dry_run { "（试运行）" } else { "" },
            self.pulled.added.len(),
            self.pulled.added_rows_total(),
            self.pulled.conflicts.len(),
            self.pushed.added.len(),
            self.pushed.added_rows_total(),
            self.pushed.conflicts.len()
        )
    }
}

// —————————————————————————————— 网络位置 ——————————————————————————————

/// 登记一个网络位置（FR-N01）。`path` 是**挂载后的本地路径**。
pub fn add(root: &Path, name: &str, kind: &str, path: &str, note: &str) -> Result<Remote> {
    let name = name.trim();
    if name.is_empty() {
        return Err(CercisError::Invalid("远程名不能为空（如 nas / lab）".into()));
    }
    if path.trim().is_empty() {
        return Err(CercisError::Invalid(
            "路径不能为空：请填挂载后的本地路径（如 /Volumes/lab-nas）".into(),
        ));
    }
    let mut remotes = Remotes::load(root)?;
    if remotes.get(name).is_some() {
        return Err(CercisError::Conflict(format!(
            "远程 {name} 已经登记过（要改就先 remote remove）"
        )));
    }
    let rec = Remote {
        name: name.to_string(),
        kind: if kind.trim().is_empty() {
            guess_kind(path).to_string()
        } else {
            kind.trim().to_string()
        },
        path: path.trim().to_string(),
        note: note.to_string(),
        created: crate::ids::now_iso8601(),
        credential_account: String::new(),
    };
    remotes.remotes.push(rec.clone());
    remotes.save(root)?;
    log_access(
        root,
        "create",
        "remote",
        name,
        &format!("登记网络位置 {name} → {}（{}）", rec.path, rec.kind),
    )?;
    Ok(rec)
}

/// 移除一个网络位置（只从清单里去掉，不动任何文件）。
pub fn remove(root: &Path, name: &str) -> Result<Remote> {
    let mut remotes = Remotes::load(root)?;
    let idx = remotes
        .remotes
        .iter()
        .position(|r| r.name == name)
        .ok_or_else(|| CercisError::NotFound(format!("没有登记过远程 {name}")))?;
    let rec = remotes.remotes.remove(idx);
    if !rec.credential_account.is_empty() {
        let _ = crate::keychain::delete(&rec.credential_account);
    }
    remotes.save(root)?;
    log_access(
        root,
        "delete",
        "remote",
        name,
        "移除网络位置（钥匙串里的凭据也删了）",
    )?;
    Ok(rec)
}

pub fn list(root: &Path) -> Result<Vec<Remote>> {
    Ok(Remotes::load(root)?.remotes)
}

/// 体检一个网络位置（FR-N06）：可达性、可写性、原子改名、大小写、中文名、速度。
pub fn test(root: &Path, name_or_path: &str) -> Result<RemoteTest> {
    let (name, kind, path) = match Remotes::load(root)?.get(name_or_path) {
        Some(r) => (r.name.clone(), r.kind.clone(), PathBuf::from(&r.path)),
        None => (
            name_or_path.to_string(),
            guess_kind(name_or_path).to_string(),
            PathBuf::from(name_or_path),
        ),
    };
    let mut t = RemoteTest {
        name,
        kind,
        path: path.display().to_string(),
        reachable: path.exists(),
        readable: false,
        writable: false,
        is_project: path.join("project.yaml").exists(),
        atomic_rename: false,
        case_insensitive: false,
        unicode_names: false,
        write_ms: 0,
        findings: Vec::new(),
        conflicts: Vec::new(),
    };

    if !t.reachable {
        t.findings.push(
            "路径不可达：先挂载网络位置（Finder「连接到服务器」/ Windows 映射网络驱动器），\
             或检查 NAS 是否在线；离线时用工作副本继续记录"
                .to_string(),
        );
        return Ok(t);
    }
    t.readable = fs::read_dir(&path).is_ok();
    if !t.readable {
        t.findings
            .push("看得见但读不了：检查共享权限".to_string());
    }

    // 写入 / 原子改名 / 速度
    let pid = std::process::id();
    let probe = path.join(format!(".cercis-probe-{pid}"));
    let probe_tmp = path.join(format!(".cercis-probe-{pid}.tmp"));
    let started = Instant::now();
    let payload = vec![7u8; 64 * 1024];
    if fs::write(&probe_tmp, &payload).is_ok() {
        t.write_ms = started.elapsed().as_millis();
        if fs::rename(&probe_tmp, &probe).is_ok() {
            t.atomic_rename = true;
            t.writable = true;
            let _ = fs::remove_file(&probe);
        } else {
            let _ = fs::remove_file(&probe_tmp);
            t.findings.push(
                "这个位置不支持原子改名：写入可能产生半写文件，不要直接在上面编辑，改用工作副本"
                    .to_string(),
            );
        }
    } else {
        t.findings.push(
            "写不进去（只读挂载或权限不足）：只能直连浏览/导出，编辑请用工作副本".to_string(),
        );
    }

    // 大小写敏感性
    let lower = path.join(format!(".cercis-case-{pid}.tmp"));
    let upper = path.join(format!(".CERCIS-CASE-{pid}.TMP"));
    if fs::write(&lower, b"x").is_ok() {
        t.case_insensitive = upper.exists();
        let _ = fs::remove_file(&lower);
    }
    if t.case_insensitive {
        t.findings.push(
            "这个位置不区分大小写：两个只差大小写的文件名会被当成同一个（避免 Sample 与 sample 并存）"
                .to_string(),
        );
    }

    let probe_zh = path.join(format!(".cercis-中文试验-{pid}"));
    if fs::write(&probe_zh, b"x").is_ok() {
        t.unicode_names = true;
        let _ = fs::remove_file(&probe_zh);
    } else if t.writable {
        t.findings.push(
            "建不了中文名文件：这个共享的字符集可能是老的（改成 UTF-8，或用英文名）".to_string(),
        );
    }

    if t.write_ms > 2000 {
        t.findings.push(format!(
            "写入偏慢（64 KB 用了 {} ms）：网络不稳时别直连读写大文件，走工作副本",
            t.write_ms
        ));
    }
    t.findings.push(
        "写之前先取写租约（cercis remote claim --who <你@设备>），让别的设备看得见你在写"
            .to_string(),
    );

    // 路径本身就是项目时，顺手做一遍分叉体检
    if t.is_project {
        if let Ok(rep) = crate::sync::check(&path) {
            for f in rep.findings {
                t.conflicts.push(format!("{}（{}）", f.path, f.detail));
            }
        }
    }
    Ok(t)
}

fn guess_kind(path: &str) -> &'static str {
    let p = path.to_ascii_lowercase();
    if p.starts_with("/volumes/") || p.starts_with("\\\\") {
        "smb"
    } else if p.starts_with("http://") || p.starts_with("https://") {
        "webdav"
    } else if p.starts_with("ssh://") || p.starts_with("sftp://") {
        "sftp"
    } else {
        "local"
    }
}

// —————————————————————————————— 写租约（FR-N04） ——————————————————————————————

/// 取一个写租约：`who` 建议写成 `<人>@<设备>`。
///
/// 这是**提醒**不是锁——NAS 上内核挡不住别人写；它的价值是让"两台设备同时写"
/// 这件事看得见（`remote test` 与 `workcopy sync` 都会报出来）。
pub fn claim(root: &Path, who: &str, minutes: i64) -> Result<String> {
    if who.trim().is_empty() {
        return Err(CercisError::Invalid(
            "要写清「谁在哪台设备上写」，如 --who alice@laptop".into(),
        ));
    }
    let dir = root.join("access/leases");
    fs::create_dir_all(&dir)?;
    let until = chrono::Utc::now() + chrono::Duration::minutes(minutes.max(1));
    let until_s = until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let text = format!(
        "who: {}\nsince: {}\nuntil: {until_s}\n",
        who.trim(),
        crate::ids::now_iso8601()
    );
    let name = format!("{}.lease", sanitize(who));
    crate::tx::write_atomic_str(&dir.join(&name), &text)?;
    Ok(format!("已取写租约：{} 写到 {}", who.trim(), until_s))
}

/// 释放写租约：删掉自己的，顺手清掉已过期的（别人的过期租约可以清，没过期的别动）。
pub fn release(root: &Path, who: &str) -> Result<usize> {
    let dir = root.join("access/leases");
    if !dir.exists() {
        return Ok(0);
    }
    let mut n = 0usize;
    let mine = format!("{}.lease", sanitize(who));
    for e in fs::read_dir(&dir)?.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".lease") {
            continue;
        }
        if name == mine || lease_expired(&p) {
            fs::remove_file(&p)?;
            n += 1;
        }
    }
    Ok(n)
}

/// 现在有哪些**还没过期**的写租约。
pub fn active_leases(root: &Path) -> Result<Vec<String>> {
    let dir = root.join("access/leases");
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for e in fs::read_dir(&dir)?.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !name.ends_with(".lease") || lease_expired(&p) {
            continue;
        }
        let text = fs::read_to_string(&p).unwrap_or_default();
        let who = text
            .lines()
            .find_map(|l| l.strip_prefix("who: "))
            .unwrap_or("?")
            .to_string();
        let until = text
            .lines()
            .find_map(|l| l.strip_prefix("until: "))
            .unwrap_or("?")
            .to_string();
        out.push(format!("{who}（到 {until}）"));
    }
    out.sort();
    Ok(out)
}

fn lease_expired(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return true;
    };
    match text
        .lines()
        .find_map(|l| l.strip_prefix("until: "))
        .and_then(|s| crate::ids::parse_iso8601(s.trim()).ok())
    {
        Some(t) => t <= chrono::Utc::now(),
        None => true,
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

// —————————————————————————————— 凭据（钥匙串，FR-N05） ——————————————————————————————

/// 把远程凭据存进系统钥匙串（**绝不写进项目**）。
pub fn credential_set(root: &Path, name: &str, secret: &str) -> Result<String> {
    let mut remotes = Remotes::load(root)?;
    if remotes.get(name).is_none() {
        return Err(CercisError::NotFound(format!(
            "没有登记过远程 {name}（先 remote add）"
        )));
    }
    let account = crate::keychain::account_for(
        &crate::access::project_key(root),
        &format!("remote:{name}"),
    );
    crate::keychain::store(&account, secret)?;
    if let Some(r) = remotes.remotes.iter_mut().find(|r| r.name == name) {
        r.credential_account = account.clone();
    }
    remotes.save(root)?;
    log_access(
        root,
        "update",
        "remote",
        name,
        "凭据已存进系统钥匙串（项目文件里没有口令）",
    )?;
    Ok(account)
}

/// 钥匙串里有没有这个远程的凭据。
pub fn credential_has(root: &Path, name: &str) -> Result<bool> {
    let remotes = Remotes::load(root)?;
    let account = match remotes.get(name) {
        Some(r) if !r.credential_account.is_empty() => r.credential_account.clone(),
        _ => crate::keychain::account_for(
            &crate::access::project_key(root),
            &format!("remote:{name}"),
        ),
    };
    Ok(crate::keychain::load(&account)?.is_some())
}

/// 删掉钥匙串里的凭据。
pub fn credential_delete(root: &Path, name: &str) -> Result<bool> {
    let mut remotes = Remotes::load(root)?;
    let account = match remotes.get(name) {
        Some(r) if !r.credential_account.is_empty() => r.credential_account.clone(),
        _ => crate::keychain::account_for(
            &crate::access::project_key(root),
            &format!("remote:{name}"),
        ),
    };
    let removed = crate::keychain::delete(&account)?;
    if let Some(r) = remotes.remotes.iter_mut().find(|r| r.name == name) {
        r.credential_account.clear();
        remotes.save(root)?;
    }
    Ok(removed)
}

fn log_access(root: &Path, action: &str, kind: &str, id: &str, summary: &str) -> Result<()> {
    let store = crate::store::Store::new(
        root,
        crate::store::Identity {
            user_id: crate::access::current_member(root).unwrap_or_else(|| "local-user".into()),
            device_id: "local-device".to_string(),
            record_device: false,
        },
    );
    store.append_change(&crate::history::ChangeEntry::new(
        &store.identity.user_id,
        "-",
        action,
        kind,
        id,
        summary,
        "",
        "",
    ))
}

// —————————————————————————————— 工作副本（FR-N03 / FR-N07） ——————————————————————————————

/// 从主副本拉一份工作副本到本地。
pub fn workcopy_create(main: &Path, to: &Path, remote_name: &str) -> Result<WorkCopy> {
    if !main.join("project.yaml").exists() {
        return Err(CercisError::NotFound(format!(
            "主副本不是 Cercis 项目（缺少 project.yaml）：{}",
            main.display()
        )));
    }
    if to.exists() && fs::read_dir(to).map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err(CercisError::Conflict(format!(
            "目标目录不为空，不能当工作副本：{}",
            to.display()
        )));
    }
    copy_tree(main, to)?;
    // 先记流水（它本身也是副本里的一条变更），再取水位——否则第一次 status 会多算一条
    log_access(
        to,
        "create",
        "workcopy",
        &fs::canonicalize(main)
            .unwrap_or_else(|_| main.to_path_buf())
            .display()
            .to_string(),
        &format!("从主副本创建本地工作副本（远程 {remote_name}）"),
    )?;
    let wc = WorkCopy {
        format: "cercis-workcopy/1".to_string(),
        remote: remote_name.to_string(),
        main: fs::canonicalize(main)
            .unwrap_or_else(|_| main.to_path_buf())
            .display()
            .to_string(),
        created: crate::ids::now_iso8601(),
        last_sync: crate::ids::now_iso8601(),
        last_sync_main_changes: watermark(main),
        last_sync_copy_changes: watermark(to),
    };
    save_workcopy(to, &wc)?;
    Ok(wc)
}

pub fn load_workcopy(copy: &Path) -> Result<WorkCopy> {
    let path = copy.join(WORKCOPY_REL);
    let text = crate::tx::read_optional(&path)?.ok_or_else(|| {
        CercisError::NotFound(format!(
            "这不是工作副本（缺少 {WORKCOPY_REL}）：{}\n\
             - 建一份：cercis workcopy create <主副本> --to <本地目录>",
            copy.display()
        ))
    })?;
    serde_yaml::from_str(&text)
        .map_err(|e| CercisError::Corrupt(format!("{WORKCOPY_REL} 解析失败: {e}")))
}

fn save_workcopy(copy: &Path, wc: &WorkCopy) -> Result<()> {
    let path = copy.join(WORKCOPY_REL);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::tx::write_atomic_str(&path, &serde_yaml::to_string(wc)?)
}

/// 同步状态（FR-N07）：本地/主副本各有多少新变更、上次同步时间、冲突项。
pub fn workcopy_status(copy: &Path) -> Result<WorkCopyStatus> {
    let wc = load_workcopy(copy)?;
    let main = PathBuf::from(&wc.main);
    let reachable = main.join("project.yaml").exists();
    let local_now = watermark(copy);
    let remote_now = if reachable { watermark(&main) } else { 0 };
    let conflicts = find_conflicts(copy);
    let mut notes = Vec::new();
    if !reachable {
        notes.push(format!(
            "主副本现在不可达（{}）：本地照记，等挂载/联网后再 workcopy sync",
            wc.main
        ));
    }
    if has_zones(copy) && crate::access::unlocked_members(copy).is_empty() {
        notes.push(
            "本地副本有加密区域但还没解锁：同步加密表前先 share unlock（密文之间没法做并集）"
                .to_string(),
        );
    }
    let local_changes = local_now.saturating_sub(wc.last_sync_copy_changes);
    let remote_changes = remote_now.saturating_sub(wc.last_sync_main_changes);
    // 三种状态分清：离线 / 有东西要同步 / **同步完了但冲突副本等人处理**
    let verdict = if !reachable {
        "offline"
    } else if local_changes > 0 || remote_changes > 0 {
        "needs-sync"
    } else if !conflicts.is_empty() {
        "has-conflicts"
    } else {
        "ok"
    };
    if verdict == "has-conflicts" {
        notes.push(format!(
            "两边已经同步完了，但有 {} 处冲突副本等人挑（挑完删掉副本，状态就回到 ok）",
            conflicts.len()
        ));
    }
    Ok(WorkCopyStatus {
        copy: copy.display().to_string(),
        main: wc.main,
        remote: wc.remote,
        reachable,
        last_sync: wc.last_sync,
        local_changes,
        remote_changes,
        conflicts,
        verdict: verdict.to_string(),
        notes,
    })
}

/// 双向同步：先看主副本在不在，再"拉回来 + 推上去"。
pub fn workcopy_sync(copy: &Path, dry_run: bool) -> Result<WorkCopySync> {
    let wc = load_workcopy(copy)?;
    let main = PathBuf::from(&wc.main);
    if !main.join("project.yaml").exists() {
        return Err(CercisError::Invalid(format!(
            "主副本不可达（现在离线）：{}\n\
             - 本地可以继续记录，挂载/联网后再跑一次 workcopy sync\n\
             - 想看两边差多少：cercis workcopy status <本地副本>",
            wc.main
        )));
    }

    let mut notes = Vec::new();
    let leases = active_leases(&main)?;
    if !leases.is_empty() {
        notes.push(format!(
            "注意：主副本上有还没过期的写租约（{}）——如果是另一台设备正在写，等它写完再合并",
            leases.join("、")
        ));
    }

    let stamp = crate::ids::timestamp_for_path(chrono::Utc::now());
    let pid = std::process::id();
    let tmp_root = std::env::temp_dir();

    let pull_dir = tmp_root.join(format!("cercis-wc-pull-{stamp}-{pid}"));
    let pulled = sync_via_pack(&main, copy, &pull_dir, dry_run)?;
    let _ = fs::remove_dir_all(&pull_dir);

    let push_dir = tmp_root.join(format!("cercis-wc-push-{stamp}-{pid}"));
    let pushed = sync_via_pack(copy, &main, &push_dir, dry_run)?;
    let _ = fs::remove_dir_all(&push_dir);

    let conflicts_total = pulled.conflicts.len() + pushed.conflicts.len();
    if !dry_run {
        let mut wc2 = wc.clone();
        wc2.last_sync = crate::ids::now_iso8601();
        wc2.last_sync_main_changes = watermark(&main);
        wc2.last_sync_copy_changes = watermark(copy);
        save_workcopy(copy, &wc2)?;
    }
    if conflicts_total > 0 {
        notes.push(format!(
            "有 {conflicts_total} 处冲突：两边都改过的文件留了 .conflict-… 副本，请人工挑一份（报告见各自的 _labnote/merge-report-*）"
        ));
    }
    Ok(WorkCopySync {
        copy: copy.display().to_string(),
        main: wc.main,
        dry_run,
        pulled,
        pushed,
        conflicts_total,
        notes,
    })
}

/// 一次单向同步：`from` 打包 → `to` 合并（走的是 S12 那一套，不另造算法）。
fn sync_via_pack(
    from: &Path,
    to: &Path,
    tmp: &Path,
    dry_run: bool,
) -> Result<crate::pack::MergeReport> {
    fs::create_dir_all(tmp)?;
    let opts = crate::pack::PackOptions {
        part_size: 0, // 本地/网络直连不需要分卷
        since: None,
        include_history: true,
        with_checkpoints: false,
        dry_run,
    };
    crate::pack::pack(from, tmp, &opts)?;
    crate::pack::merge(
        tmp,
        to,
        &crate::pack::MergeOptions {
            dry_run,
            member: None,
        },
    )
}

/// 流水条数（水位）：优先透明读（表可能在加密区域里）。
fn watermark(root: &Path) -> usize {
    let rel = "tables/change_log.csv";
    if let Ok(Some(bytes)) = crate::access::read_through_zone(root, None, rel) {
        return String::from_utf8_lossy(&bytes).lines().count().saturating_sub(1);
    }
    match fs::read_to_string(root.join(rel)) {
        Ok(text) => text.lines().count().saturating_sub(1),
        Err(_) => 0,
    }
}

fn has_zones(root: &Path) -> bool {
    crate::access::Zones::load(root)
        .map(|z| !z.zones.is_empty())
        .unwrap_or(false)
}

/// 找需要人处理的冲突副本（`.conflict-…`）。
fn find_conflicts(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if matches!(name.as_str(), ".git" | "target" | "_labnote" | "history") {
                    continue;
                }
                stack.push(p);
                continue;
            }
            if name.contains(".conflict-") {
                out.push(
                    p.strip_prefix(root)
                        .map(|x| x.to_string_lossy().to_string())
                        .unwrap_or(name),
                );
            }
        }
    }
    out.sort();
    out
}

/// 递归复制（建工作副本用）：不复制版本库与生成物，也不复制半写的临时文件。
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name_s = name.to_string_lossy().to_string();
        if matches!(name_s.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let dest = to.join(&name);
        if p.is_dir() {
            copy_tree(&p, &dest)?;
        } else if !name_s.ends_with(crate::tx::TMP_SUFFIX) {
            fs::copy(&p, &dest)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Identity;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cercis-remote-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn add_row(p: &crate::Project, id: &str) {
        let mut row = crate::table::Row::new();
        row.insert("record_id".into(), id.into());
        row.insert("value_num".into(), "1".into());
        p.store
            .append_row_extending("records", &[], row, id, "测试写入")
            .unwrap();
    }

    #[test]
    fn remote_add_test_and_remove() {
        let dir = tmp("remote");
        let main = dir.join("main");
        let p = crate::Project::create(&main, "试验", Identity::default()).unwrap();
        add_row(&p, "R-1");

        let rec = add(&main, "nas", "local", main.to_str().unwrap(), "本机当 NAS 自测").unwrap();
        assert_eq!(rec.kind, "local");
        assert_eq!(list(&main).unwrap().len(), 1);

        let t = test(&main, "nas").unwrap();
        assert!(t.reachable && t.writable && t.atomic_rename);
        assert!(t.is_project);

        // 清单里不能有任何口令/密钥字样
        let text = fs::read_to_string(main.join(REMOTES_REL)).unwrap();
        assert!(!text.contains("password") && !text.contains("secret"));

        remove(&main, "nas").unwrap();
        assert!(list(&main).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workcopy_two_way_sync_unions() {
        let dir = tmp("workcopy");
        let main = dir.join("main");
        let copy = dir.join("copy");
        let p = crate::Project::create(&main, "试验", Identity::default()).unwrap();
        add_row(&p, "R-1");

        let wc = workcopy_create(&main, &copy, "nas").unwrap();
        assert_eq!(
            wc.main,
            fs::canonicalize(&main).unwrap().display().to_string()
        );
        assert!(copy.join("tables/records.csv").exists());

        // 本地离线记两条；主副本上也记一条
        let pc = crate::Project::open(&copy, None).unwrap();
        add_row(&pc, "R-2");
        add_row(&pc, "R-3");
        add_row(&p, "R-4");

        let st = workcopy_status(&copy).unwrap();
        assert_eq!(st.verdict, "needs-sync", "{st:?}");
        assert_eq!(st.local_changes, 2);
        assert_eq!(st.remote_changes, 1);

        let rep = workcopy_sync(&copy, false).unwrap();
        assert_eq!(rep.conflicts_total, 0);

        for root in [&main, &copy] {
            let t = crate::table::Table::load(root.join("tables/records.csv")).unwrap();
            let ids: Vec<String> = t
                .rows
                .iter()
                .map(|r| r.get("record_id").cloned().unwrap_or_default())
                .collect();
            for want in ["R-1", "R-2", "R-3", "R-4"] {
                assert!(ids.contains(&want.to_string()), "{root:?} 缺 {want}");
            }
            assert_eq!(t.rows.len(), 4, "不能装重：{root:?}");
        }

        let st2 = workcopy_status(&copy).unwrap();
        assert_eq!(st2.verdict, "ok", "{st2:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn workcopy_reports_offline_clearly() {
        let dir = tmp("offline");
        let main = dir.join("main");
        let copy = dir.join("copy");
        let p = crate::Project::create(&main, "试验", Identity::default()).unwrap();
        add_row(&p, "R-1");
        workcopy_create(&main, &copy, "nas").unwrap();

        // 主副本"掉线"
        fs::rename(&main, dir.join("main-moved")).unwrap();
        let st = workcopy_status(&copy).unwrap();
        assert_eq!(st.verdict, "offline");
        assert!(st.notes.iter().any(|n| n.contains("不可达")));
        let err = workcopy_sync(&copy, false).unwrap_err();
        assert!(err.to_string().contains("离线"), "{err}");

        // 离线时本地照常能写
        let pc = crate::Project::open(&copy, None).unwrap();
        add_row(&pc, "R-2");
        let t = crate::table::Table::load(copy.join("tables/records.csv")).unwrap();
        assert_eq!(t.rows.len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_distinguishes_pending_conflicts_from_pending_sync() {
        let dir = tmp("conflict-status");
        let main = dir.join("main");
        let copy = dir.join("copy");
        let p = crate::Project::create(&main, "试验", Identity::default()).unwrap();
        add_row(&p, "R-1");
        workcopy_create(&main, &copy, "nas").unwrap();
        // 只造一个冲突副本（假装上次同步留下、人还没处理），两边流水都不再动
        std::fs::create_dir_all(copy.join("methods")).unwrap();
        std::fs::write(
            copy.join("methods/方法.md.conflict-2026-01-01T000000Z-abc123"),
            "对方那份",
        )
        .unwrap();

        let st = workcopy_status(&copy).unwrap();
        assert_eq!(
            st.verdict, "has-conflicts",
            "同步已完成、只剩冲突副本 → 不该再报「要同步」：{st:?}"
        );
        assert_eq!(st.local_changes, 0);
        assert_eq!(st.remote_changes, 0);
        assert!(st.notes.iter().any(|n| n.contains("同步完了")), "{:?}", st.notes);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn leases_are_visible_and_expire() {
        let dir = tmp("lease");
        let main = dir.join("main");
        crate::Project::create(&main, "试验", Identity::default()).unwrap();
        claim(&main, "alice@laptop", 30).unwrap();
        let active = active_leases(&main).unwrap();
        assert_eq!(active.len(), 1);
        assert!(active[0].contains("alice@laptop"));
        // 过期的租约不算数
        fs::write(
            main.join("access/leases/old.lease"),
            "who: old\nsince: 2020-01-01T00:00:00Z\nuntil: 2020-01-01T00:30:00Z\n",
        )
        .unwrap();
        assert_eq!(active_leases(&main).unwrap().len(), 1);
        assert!(release(&main, "alice@laptop").unwrap() >= 1);
        assert!(active_leases(&main).unwrap().is_empty());
        let _ = fs::remove_dir_all(&dir);
    }
}
