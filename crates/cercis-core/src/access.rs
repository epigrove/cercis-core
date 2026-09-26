//! 加密共享与分权访问（S11 / FR-I07–FR-I13）。
//!
//! 目标：**默认不加密**（CSV 保持开放，R / AI 直读），只有被划进"加密区域"
//! 的文件在磁盘上才是密文；成员用自己的账号口令解锁自己有权访问的区域。
//!
//! ## 磁盘上有什么
//! ```text
//! access/
//!   members.yaml            成员目录（只有公钥与元信息，**没有口令、没有私钥明文**）
//!   members/<id>.key        私钥文件：用成员口令（Argon2id）加密后的私钥
//!   zones.yaml              加密区域：区域名、路径规则、成员授权、被封装的内容密钥
//!   keyrings/<zone>.yaml    历史密钥世代（换密钥后旧的留档，用于解旧密文与回滚）
//! tables/xxx.csv.enc        区域内的文件：密文（明文不留在磁盘上）
//! ```
//!
//! ## 密钥怎么走
//! 1. 每个区域一把**内容密钥**（随机 32 字节），文件都用它做 ChaCha20-Poly1305；
//! 2. 内容密钥用每个**成员公钥**分别封装（X25519 + HKDF + AEAD）后写进 `zones.yaml`；
//! 3. 成员私钥用**自己的口令**（Argon2id）加密后存在 `members/<id>.key`；
//! 4. `unlock` 时先解私钥、再解自己那份内容密钥，结果只落在**用户主目录**的会话文件里
//!    （0600 权限，不进项目目录），`lock` 即抹掉。
//!
//! 因此：**拿到项目文件夹 ≠ 拿得到数据**；没有授权成员的私钥，`.enc` 就是一串乱码。
//!
//! ## 边界（诚实说明）
//! - 区域名、路径规则、文件个数与**文件名**在磁盘上是可见的（`.enc` 就在那里）；
//!   隐藏的是**内容**。app 内对未授权成员不显示这些条目。
//! - "移除成员"只能保证他**从这一刻起**读不到新内容；他之前看过的内容无法追回
//!   （这是所有加密方案的共同边界，本工具不假装例外）。
//! - 忘记口令无法找回：只能由管理员重新授权一个新成员身份。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::{self, KdfParams, WrappedKey};
use crate::history::ChangeEntry;
use crate::store::{Identity, Store};
use crate::{CercisError, Result};

/// 声明文件格式版本。
pub const ACCESS_FORMAT: &str = "cercis-access/1";
/// 成员目录（项目内相对路径）。
pub const MEMBERS_REL: &str = "access/members.yaml";
/// 加密区域定义（项目内相对路径）。
pub const ZONES_REL: &str = "access/zones.yaml";
/// 成员私钥目录（项目内相对路径）。
pub const MEMBER_KEYS_REL: &str = "access/members";
/// 历史密钥世代目录（项目内相对路径）。
pub const KEYRINGS_REL: &str = "access/keyrings";
/// 历史对象迁移台账（哪些老对象被加密成了哪个新对象）。
pub const OBJECT_MAP_REL: &str = "access/zone-objects.yaml";
/// 密文文件后缀。
pub const ENC_SUFFIX: &str = ".enc";
/// 默认会话有效期（小时）。
pub const DEFAULT_TTL_HOURS: i64 = 12;

const ENC_MAGIC: &[u8; 8] = b"CERCSENC";
const ENC_VERSION: u8 = 1;
const ENC_ALG_ID: u8 = 1; // 1 = chacha20poly1305
const ENC_HEADER_LEN: usize = 8 + 1 + 1 + 12;
/// 内核自用目录：不参与加密扫描（密钥、历史、导出、日志缓存）。
const INTERNAL_DIRS: &[&str] = &[".git", "access", "history", "exports", "_labnote"];

// —————————————————————————————— 权限 ——————————————————————————————

/// 成员在一个区域里的权限级别（FR-I07 的"只读 / 可写 / 管理"）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// 只读：能看、能解密导出
    Read,
    /// 可写：能改区域内的数据
    Write,
    /// 管理：能改区域成员与规则
    Admin,
}

impl Default for Permission {
    fn default() -> Self {
        Permission::Read
    }
}

impl Permission {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "read" | "r" | "只读" | "读" => Ok(Permission::Read),
            "write" | "w" | "可写" | "写" => Ok(Permission::Write),
            "admin" | "a" | "管理" => Ok(Permission::Admin),
            other => Err(CercisError::Invalid(format!(
                "权限只能是 read / write / admin，实际: {other}"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::Read => "read",
            Permission::Write => "write",
            Permission::Admin => "admin",
        }
    }

    pub fn label_cn(&self) -> &'static str {
        match self {
            Permission::Read => "只读",
            Permission::Write => "可写",
            Permission::Admin => "管理",
        }
    }

    pub fn can_read(&self) -> bool {
        true
    }

    pub fn can_write(&self) -> bool {
        matches!(self, Permission::Write | Permission::Admin)
    }

    pub fn can_admin(&self) -> bool {
        matches!(self, Permission::Admin)
    }
}

// —————————————————————————————— 声明文件 ——————————————————————————————

/// 成员目录里的一条（**不含任何秘密**）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberRecord {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    /// X25519 公钥（十六进制）
    pub public_key: String,
    /// 私钥文件相对路径
    pub key_file: String,
    pub created: String,
    #[serde(default)]
    pub note: String,
}

/// `access/members.yaml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Members {
    pub format: String,
    #[serde(default)]
    pub members: Vec<MemberRecord>,
}

impl Default for Members {
    fn default() -> Self {
        Self {
            format: ACCESS_FORMAT.to_string(),
            members: Vec::new(),
        }
    }
}

impl Members {
    pub fn load(root: &Path) -> Result<Self> {
        let path = members_file(root);
        match crate::tx::read_optional(&path)? {
            None => Ok(Self::default()),
            Some(text) => {
                let m: Members = serde_yaml::from_str(&text)
                    .map_err(|e| CercisError::Corrupt(format!("members.yaml 解析失败: {e}")))?;
                m.validate()?;
                Ok(m)
            }
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let text = serde_yaml::to_string(self)?;
        crate::tx::write_atomic_str(&members_file(root), &text)
    }

    pub fn validate(&self) -> Result<()> {
        if self.format != ACCESS_FORMAT {
            return Err(CercisError::Corrupt(format!(
                "members.yaml 格式版本不认识: {}（本内核支持 {ACCESS_FORMAT}）",
                self.format
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for m in &self.members {
            if !is_valid_id(&m.id) {
                return Err(CercisError::Corrupt(format!(
                    "成员 id 需为小写 ASCII 字母数字/下划线/短横线: {:?}",
                    m.id
                )));
            }
            if !seen.insert(m.id.clone()) {
                return Err(CercisError::Corrupt(format!("成员 id 重复: {}", m.id)));
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&MemberRecord> {
        self.members.iter().find(|m| m.id == id)
    }
}

/// 一个区域里的成员授权（含发给他的那份被封装的内容密钥）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneMember {
    pub id: String,
    #[serde(default)]
    pub permission: Permission,
    /// 被该成员公钥封装过的内容密钥（本次世代）
    pub wrapped: WrappedKey,
}

/// 一个加密区域。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zone {
    /// 机器用名（ASCII，唯一）
    pub name: String,
    /// 显示名（可中文）
    #[serde(default)]
    pub label: String,
    /// 加密算法标识
    pub cipher: String,
    /// 路径规则（相对项目根；支持 `*`、`**`、目录前缀）
    pub paths: Vec<String>,
    /// 当前密钥世代（换密钥时 +1）
    #[serde(default = "one")]
    pub generation: u32,
    /// 当前内容密钥的指纹（人工核对用，不是密钥本身）
    pub key_fingerprint: String,
    pub created: String,
    #[serde(default)]
    pub rotated: String,
    #[serde(default)]
    pub members: Vec<ZoneMember>,
}

fn one() -> u32 {
    1
}

impl Zone {
    pub fn display(&self) -> String {
        if self.label.is_empty() {
            self.name.clone()
        } else {
            format!("{}（{}）", self.label, self.name)
        }
    }

    pub fn permission_of(&self, member: &str) -> Option<Permission> {
        self.members
            .iter()
            .find(|m| m.id == member)
            .map(|m| m.permission)
    }

    pub fn member_ids(&self) -> Vec<String> {
        self.members.iter().map(|m| m.id.clone()).collect()
    }

    /// 路径是否落在本区域内。
    pub fn covers(&self, rel: &str) -> bool {
        self.paths.iter().any(|p| matches_path(p, rel))
    }

    fn wrap_info_for(&self, member: &str) -> String {
        crypto::wrap_info(&self.name, member, self.generation)
    }
}

/// `access/zones.yaml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Zones {
    pub format: String,
    #[serde(default)]
    pub zones: Vec<Zone>,
}

impl Default for Zones {
    fn default() -> Self {
        Self {
            format: ACCESS_FORMAT.to_string(),
            zones: Vec::new(),
        }
    }
}

impl Zones {
    pub fn load(root: &Path) -> Result<Self> {
        let path = zones_file(root);
        match crate::tx::read_optional(&path)? {
            None => Ok(Self::default()),
            Some(text) => {
                let z: Zones = serde_yaml::from_str(&text)
                    .map_err(|e| CercisError::Corrupt(format!("zones.yaml 解析失败: {e}")))?;
                z.validate()?;
                Ok(z)
            }
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let text = serde_yaml::to_string(self)?;
        let path = zones_file(root);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::tx::write_atomic_str(&path, &text)
    }

    pub fn validate(&self) -> Result<()> {
        if self.format != ACCESS_FORMAT {
            return Err(CercisError::Corrupt(format!(
                "zones.yaml 格式版本不认识: {}（本内核支持 {ACCESS_FORMAT}）",
                self.format
            )));
        }
        let mut seen = std::collections::BTreeSet::new();
        for z in &self.zones {
            if !is_valid_id(&z.name) {
                return Err(CercisError::Corrupt(format!(
                    "区域名需为小写 ASCII 字母数字/下划线/短横线: {:?}",
                    z.name
                )));
            }
            if !seen.insert(z.name.clone()) {
                return Err(CercisError::Corrupt(format!("区域名重复: {}", z.name)));
            }
            if z.paths.is_empty() {
                return Err(CercisError::Corrupt(format!(
                    "区域 {} 没有路径规则",
                    z.name
                )));
            }
        }
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&Zone> {
        self.zones.iter().find(|z| z.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Zone> {
        self.zones.iter_mut().find(|z| z.name == name)
    }

    /// 找到覆盖某路径的区域（若有多个，取声明顺序里的第一个）。
    pub fn covering(&self, rel: &str) -> Option<&Zone> {
        self.zones.iter().find(|z| z.covers(rel))
    }
}

/// 历史密钥世代（换密钥后旧的留档）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetiredGeneration {
    pub generation: u32,
    pub key_fingerprint: String,
    pub created: String,
    pub retired: String,
    pub members: Vec<ZoneMember>,
}

/// `access/keyrings/<zone>.yaml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyring {
    pub format: String,
    pub zone: String,
    #[serde(default)]
    pub retired: Vec<RetiredGeneration>,
}

impl Keyring {
    pub fn load(root: &Path, zone: &str) -> Result<Self> {
        let path = keyring_file(root, zone);
        match crate::tx::read_optional(&path)? {
            None => Ok(Self {
                format: ACCESS_FORMAT.to_string(),
                zone: zone.to_string(),
                retired: Vec::new(),
            }),
            Some(text) => serde_yaml::from_str(&text)
                .map_err(|e| CercisError::Corrupt(format!("keyrings/{zone}.yaml 解析失败: {e}"))),
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = keyring_file(root, &self.zone);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::tx::write_atomic_str(&path, &serde_yaml::to_string(self)?)
    }
}

/// 历史对象迁移台账。
///
/// 为什么需要：`history/objects/` 里存着"每一次改动前的旧内容"。
/// 一个表在**被划进加密区域之前**存下的快照仍是明文——只把当前文件加密，
/// 老快照就等于留了一份明文备份。所以划区域时要连这些老对象一起加密，
/// 并在这里记下"老指纹 → 新（密文）指纹"，这样旧流水仍然找得回内容。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneObjects {
    pub format: String,
    /// 明文老指纹 → 密文对象指纹
    #[serde(default)]
    pub aliases: BTreeMap<String, String>,
    /// 密文对象指纹 → 它属于哪个区域
    #[serde(default)]
    pub owners: BTreeMap<String, String>,
}

impl Default for ZoneObjects {
    fn default() -> Self {
        Self {
            format: ACCESS_FORMAT.to_string(),
            aliases: BTreeMap::new(),
            owners: BTreeMap::new(),
        }
    }
}

impl ZoneObjects {
    pub fn load(root: &Path) -> Result<Self> {
        match crate::tx::read_optional(&root.join(OBJECT_MAP_REL))? {
            None => Ok(Self::default()),
            Some(text) => serde_yaml::from_str(&text)
                .map_err(|e| CercisError::Corrupt(format!("{OBJECT_MAP_REL} 解析失败: {e}"))),
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        crate::tx::write_atomic_str(
            &root.join(OBJECT_MAP_REL),
            &serde_yaml::to_string(self)?,
        )
    }
}

/// 历史对象加密的结果。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct HistoryEncryptReport {
    pub encrypted: usize,
    pub already: usize,
    pub missing: usize,
}

// —————————————————————————————— 会话 ——————————————————————————————

/// 会话里装着"我已经解开、但没写回项目"的内容密钥。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockedZone {
    pub name: String,
    pub permission: Permission,
    pub key_fingerprint: String,
    /// 内容密钥（十六进制）。**只存在于用户主目录的会话文件里**，不进项目。
    pub content_key: String,
}

/// `unlock` 之后的解锁票据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub format: String,
    pub project: String,
    pub member: String,
    pub created: String,
    pub expires: String,
    #[serde(default)]
    pub zones: Vec<UnlockedZone>,
    /// 成员私钥（十六进制）。放在会话里是为了能解**历史世代**的密钥，
    /// 从而在换密钥后仍能重加密旧密文；会话文件只在本机、0600 权限、不进项目。
    #[serde(default)]
    pub member_secret: String,
}

impl Session {
    pub fn key_for(&self, zone: &str) -> Option<Zeroizing<Vec<u8>>> {
        self.zones
            .iter()
            .find(|z| z.name == zone)
            .and_then(|z| hex::decode(&z.content_key).ok())
            .map(Zeroizing::new)
    }

    pub fn zone(&self, zone: &str) -> Option<&UnlockedZone> {
        self.zones.iter().find(|z| z.name == zone)
    }

    pub fn is_expired(&self) -> bool {
        match crate::ids::parse_iso8601(&self.expires) {
            Ok(exp) => exp <= chrono::Utc::now(),
            Err(_) => true,
        }
    }
}

// —————————————————————————————— 报告 ——————————————————————————————

/// `unlock` 的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnlockReport {
    pub member: String,
    pub zones: Vec<UnlockedZone>,
    pub expires: String,
    pub session_file: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// `apply` 的结果（一个区域一条）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneApplyReport {
    pub zone: String,
    pub dry_run: bool,
    pub encrypted: Vec<String>,
    pub kept_plaintext: Vec<String>,
    pub already: usize,
    pub bytes: u64,
    /// 顺带加密的历史快照个数（老明文本不该留在对象库里）
    #[serde(default)]
    pub objects_encrypted: usize,
}

/// `export` 的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportReport {
    pub zone: String,
    pub member: String,
    pub dest: String,
    pub files: Vec<String>,
    pub bytes: u64,
    pub dry_run: bool,
}

/// `revoke` 的结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeReport {
    pub zone: String,
    pub member: String,
    pub generation: u32,
    pub key_fingerprint: String,
    pub reencrypted: usize,
    pub remaining: Vec<String>,
}

/// 一个区域的当前状态（体检用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneStatus {
    pub zone: String,
    pub label: String,
    pub generation: u32,
    pub key_fingerprint: String,
    pub members: Vec<(String, Permission)>,
    /// 当前成员在该区域的权限（不是成员就是 None）
    pub permission: Option<Permission>,
    pub files: usize,
    pub bytes: u64,
    /// 区域里还没加密的明文文件（**这是要尽快处理的**）
    pub plaintext_left: Vec<String>,
    /// 用你手上的钥匙解不开的密文个数（换密钥后没重加密、或你不是它的授权成员）
    #[serde(default)]
    pub unreadable: usize,
}

/// 整个项目的加密状态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessStatus {
    pub members: Vec<MemberRecord>,
    pub zones: Vec<ZoneStatus>,
    pub session_member: Option<String>,
    pub share_home: String,
    pub keychain_available: bool,
}

// —————————————————————————————— 路径 ——————————————————————————————

pub fn members_file(root: &Path) -> PathBuf {
    root.join(MEMBERS_REL)
}

pub fn zones_file(root: &Path) -> PathBuf {
    root.join(ZONES_REL)
}

pub fn member_key_path(root: &Path, id: &str) -> PathBuf {
    root.join(MEMBER_KEYS_REL).join(format!("{id}.key"))
}

pub fn keyring_file(root: &Path, zone: &str) -> PathBuf {
    root.join(KEYRINGS_REL).join(format!("{zone}.yaml"))
}

/// 用户级共享目录（会话与钥匙串账号都放这儿，**不进项目**）。
pub fn share_home() -> PathBuf {
    if let Ok(dir) = std::env::var("CERCIS_SHARE_HOME") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".cercis").join("share")
}

/// 项目标识：用规范化绝对路径的 SHA-256 前 16 位（换机器/换路径即失效，符合预期）。
pub fn project_key(root: &Path) -> String {
    let canon = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    crypto::sha256_hex(canon.to_string_lossy().as_bytes())[..16].to_string()
}

pub fn project_dir(root: &Path) -> PathBuf {
    share_home().join("projects").join(project_key(root))
}

pub fn session_path(root: &Path, member: &str) -> PathBuf {
    project_dir(root).join(format!("{member}.session.json"))
}

/// 当前"生效成员"（`unlock` 时写入；后续命令不带 `--member` 也认得出）。
pub fn current_member(root: &Path) -> Option<String> {
    let p = project_dir(root).join("current");
    fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn set_current_member(root: &Path, member: Option<&str>) -> Result<()> {
    let dir = project_dir(root);
    fs::create_dir_all(&dir)?;
    let p = dir.join("current");
    match member {
        Some(m) => crate::tx::write_atomic_str(&p, &format!("{m}\n")),
        None => {
            if p.exists() {
                fs::remove_file(&p)?;
            }
            Ok(())
        }
    }
}

/// 该项目已解锁的会话成员列表。
pub fn unlocked_members(root: &Path) -> Vec<String> {
    let dir = project_dir(root);
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(&dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(m) = name.strip_suffix(".session.json") {
                out.push(m.to_string());
            }
        }
    }
    out.sort();
    out
}

/// 读取某个成员的会话（过期即视为没有）。
pub fn session(root: &Path, member: &str) -> Result<Option<Session>> {
    let path = session_path(root, member);
    let Some(text) = crate::tx::read_optional(&path)? else {
        return Ok(None);
    };
    let s: Session = serde_json::from_str(&text)
        .map_err(|e| CercisError::Corrupt(format!("会话文件损坏（{}）: {e}", path.display())))?;
    if s.is_expired() {
        let _ = fs::remove_file(&path);
        return Ok(None);
    }
    Ok(Some(s))
}

/// 解析"该用哪个成员的会话"：显式指定优先，其次当前生效成员。
pub fn resolve_session(root: &Path, explicit: Option<&str>) -> Result<Option<Session>> {
    let member = match explicit {
        Some(m) if !m.trim().is_empty() => m.trim().to_string(),
        _ => match current_member(root) {
            Some(m) => m,
            None => return Ok(None),
        },
    };
    session(root, &member)
}

/// 需要已解锁的会话（否则给出可操作的中文提示）。
pub fn require_session(root: &Path, explicit: Option<&str>) -> Result<Session> {
    match resolve_session(root, explicit)? {
        Some(s) => Ok(s),
        None => Err(CercisError::Invalid(
            "这个项目还没有解锁（加密区域需要先解锁）。\n\
             - 解锁：cercis share unlock <项目> --member <成员>\n\
             - 看有哪些成员：cercis share list <项目>"
                .to_string(),
        )),
    }
}

// —————————————————————————————— 成员 ——————————————————————————————

/// 新建成员：生成密钥对，用口令加密私钥后落盘。
pub fn create_member(
    root: &Path,
    id: &str,
    display_name: &str,
    password: &str,
    params: KdfParams,
    note: &str,
) -> Result<MemberRecord> {
    if !is_valid_id(id) {
        return Err(CercisError::Invalid(format!(
            "成员 id 需为小写 ASCII 字母数字/下划线/短横线（如 alice、zhang-2026）: {id:?}"
        )));
    }
    if password.is_empty() {
        return Err(CercisError::Invalid("口令不能为空".into()));
    }
    let mut members = Members::load(root)?;
    if members.get(id).is_some() {
        return Err(CercisError::Conflict(format!(
            "成员 {id} 已存在（改口令用：cercis share passwd <项目> --id {id}）"
        )));
    }

    let kp = crypto::keypair();
    write_member_key(root, id, &kp.secret, &kp.public, password, params)?;

    let record = MemberRecord {
        id: id.to_string(),
        display_name: display_name.to_string(),
        public_key: hex::encode(kp.public),
        key_file: format!("{MEMBER_KEYS_REL}/{id}.key"),
        created: crate::ids::now_iso8601(),
        note: note.to_string(),
    };
    members.members.push(record.clone());
    members.validate()?;
    members.save(root)?;
    log(
        root,
        id,
        "create",
        "member",
        id,
        &format!("新建加密共享成员：{id}（私钥已用其口令加密）"),
    )?;
    Ok(record)
}

/// 改口令：重新加密同一份私钥（不解锁区域、不动数据）。
pub fn change_password(
    root: &Path,
    id: &str,
    old_password: &str,
    new_password: &str,
    params: KdfParams,
) -> Result<()> {
    let (secret, public) = load_member_secret(root, id, old_password)?;
    if new_password.is_empty() {
        return Err(CercisError::Invalid("新口令不能为空".into()));
    }
    write_member_key(root, id, &secret, &public, new_password, params)?;
    log(
        root,
        id,
        "update",
        "member",
        id,
        "更新成员口令（私钥未变，区域密钥无需重发）",
    )
}

/// 载入成员私钥（口令不对就解不开）。
pub fn load_member_secret(
    root: &Path,
    id: &str,
    password: &str,
) -> Result<(Zeroizing<[u8; 32]>, [u8; 32])> {
    let path = member_key_path(root, id);
    let text = crate::tx::read_optional(&path)?.ok_or_else(|| {
        CercisError::NotFound(format!(
            "成员 {id} 的私钥文件不存在（{}）——可能是别人把他的密钥删了，需管理员重新授权",
            path.display()
        ))
    })?;
    let f: MemberKeyFile = serde_yaml::from_str(&text)
        .map_err(|e| CercisError::Corrupt(format!("成员私钥文件解析失败: {e}")))?;
    if f.format != "cercis-member-key/1" {
        return Err(CercisError::Corrupt(format!(
            "成员私钥文件版本不认识: {}",
            f.format
        )));
    }
    if f.member != id {
        return Err(CercisError::Corrupt(format!(
            "成员私钥文件对不上：文件里是 {}，要求的是 {id}",
            f.member
        )));
    }
    let salt = hex::decode(&f.kdf.salt)
        .map_err(|e| CercisError::Corrupt(format!("盐不是十六进制: {e}")))?;
    let params = KdfParams {
        m_cost: f.kdf.m_cost,
        t_cost: f.kdf.t_cost,
        p_cost: f.kdf.p_cost,
    };
    let dk = crypto::derive_key(password, &salt, params)?;
    let nonce = hex::decode(&f.nonce)
        .map_err(|e| CercisError::Corrupt(format!("nonce 不是十六进制: {e}")))?;
    let sealed = hex::decode(&f.sealed)
        .map_err(|e| CercisError::Corrupt(format!("密文不是十六进制: {e}")))?;
    let aad = member_key_aad(id, &params, &salt, &f.public_key);
    let plain = crypto::open(&dk, &nonce, aad.as_bytes(), &sealed).map_err(|_| {
        CercisError::Invalid("口令不对（或私钥文件被改动过）".to_string())
    })?;
    if plain.len() != 32 {
        return Err(CercisError::Corrupt("私钥长度不对".into()));
    }
    let mut secret = [0u8; 32];
    secret.copy_from_slice(&plain);
    let public = hex_decode_32(&f.public_key, "公钥")?;
    // 自检：公钥必须与私钥配得上（防止文件被换过）
    if x25519_public_of(&secret) != public {
        return Err(CercisError::Corrupt(
            "私钥与公钥不匹配（文件可能被篡改）".into(),
        ));
    }
    Ok((Zeroizing::new(secret), public))
}

/// 列出成员。
pub fn list_members(root: &Path) -> Result<Vec<MemberRecord>> {
    Ok(Members::load(root)?.members)
}

fn x25519_public_of(secret: &[u8; 32]) -> [u8; 32] {
    let sk = x25519_dalek::StaticSecret::from(*secret);
    let pk = x25519_dalek::PublicKey::from(&sk);
    pk.to_bytes()
}

fn write_member_key(
    root: &Path,
    id: &str,
    secret: &[u8; 32],
    public: &[u8; 32],
    password: &str,
    params: KdfParams,
) -> Result<()> {
    let salt = crypto::random_bytes(crypto::SALT_LEN);
    let dk = crypto::derive_key(password, &salt, params)?;
    let nonce = crypto::random_bytes(crypto::NONCE_LEN);
    let public_hex = hex::encode(public);
    let aad = member_key_aad(id, &params, &salt, &public_hex);
    let sealed = crypto::seal(&dk, &nonce, aad.as_bytes(), secret)?;
    let file = MemberKeyFile {
        format: "cercis-member-key/1".to_string(),
        member: id.to_string(),
        kdf: KdfBlock {
            algorithm: crypto::KDF_ALG.to_string(),
            m_cost: params.m_cost,
            t_cost: params.t_cost,
            p_cost: params.p_cost,
            salt: hex::encode(&salt),
        },
        cipher: crypto::AEAD_ALG.to_string(),
        nonce: hex::encode(nonce),
        public_key: public_hex,
        sealed: hex::encode(sealed),
    };
    let path = member_key_path(root, id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_private(&path, serde_yaml::to_string(&file)?.as_bytes())?;
    Ok(())
}

fn member_key_aad(id: &str, params: &KdfParams, salt: &[u8], public_hex: &str) -> String {
    format!(
        "cercis-member-key/1|{id}|{}|{}|{}|{}|{}|{}",
        crypto::KDF_ALG,
        params.m_cost,
        params.t_cost,
        params.p_cost,
        hex::encode(salt),
        public_hex
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemberKeyFile {
    format: String,
    member: String,
    kdf: KdfBlock,
    cipher: String,
    nonce: String,
    public_key: String,
    sealed: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KdfBlock {
    algorithm: String,
    m_cost: u32,
    t_cost: u32,
    p_cost: u32,
    salt: String,
}

// —————————————————————————————— 区域 ——————————————————————————————

/// 新建区域：生成内容密钥、按成员公钥封装、写入声明；可选**立刻加密**命中的文件。
pub fn create_zone(
    root: &Path,
    name: &str,
    label: &str,
    paths: &[String],
    grants: &[(String, Permission)],
    encrypt_now: bool,
    keep_plaintext: bool,
) -> Result<(Zone, Option<ZoneApplyReport>)> {
    if !is_valid_id(name) {
        return Err(CercisError::Invalid(format!(
            "区域名需为小写 ASCII 字母数字/下划线/短横线（如 patent-data）: {name:?}"
        )));
    }
    if paths.is_empty() {
        return Err(CercisError::Invalid(
            "至少要给一条路径规则（如 tables/patent_*.csv 或 methods/配方/**）".into(),
        ));
    }
    if grants.is_empty() {
        return Err(CercisError::Invalid(
            "加密区域至少要有一个成员（否则没人能打开它）".into(),
        ));
    }
    if !grants.iter().any(|(_, p)| p.can_admin()) {
        return Err(CercisError::Invalid(
            "加密区域至少要有一个 admin 成员（否则以后没人能改区域设置）".into(),
        ));
    }
    let mut zones = Zones::load(root)?;
    if zones.get(name).is_some() {
        return Err(CercisError::Conflict(format!("区域已存在: {name}")));
    }
    let members = Members::load(root)?;

    let content_key = crypto::random_bytes(crypto::KEY_LEN);
    let mut zone_members = Vec::new();
    for (id, perm) in grants {
        let rec = members.get(id).ok_or_else(|| {
            CercisError::NotFound(format!("成员不存在: {id}（先 cercis share member-add）"))
        })?;
        let public = hex_decode_32(&rec.public_key, "公钥")?;
        let wrapped = crypto::wrap_content_key(
            &content_key,
            &public,
            &crypto::wrap_info(name, id, 1),
        )?;
        zone_members.push(ZoneMember {
            id: id.clone(),
            permission: *perm,
            wrapped,
        });
    }

    let zone = Zone {
        name: name.to_string(),
        label: label.to_string(),
        cipher: crypto::AEAD_ALG.to_string(),
        paths: paths.to_vec(),
        generation: 1,
        key_fingerprint: crypto::fingerprint(&content_key),
        created: crate::ids::now_iso8601(),
        rotated: String::new(),
        members: zone_members,
    };
    let report = if encrypt_now {
        Some(apply_with_key(
            root,
            &zone,
            &content_key,
            false,
            keep_plaintext,
        )?)
    } else {
        None
    };

    zones.zones.push(zone.clone());
    zones.validate()?;
    zones.save(root)?;
    log(
        root,
        &current_member(root).unwrap_or_else(|| "local-user".into()),
        "create",
        "zone",
        name,
        &format!(
            "新建加密区域 {}（{} 条路径规则，成员 {}，密钥指纹 {}）",
            zone.display(),
            paths.len(),
            zone.member_ids().join("/"),
            zone.key_fingerprint
        ),
    )?;
    Ok((zone, report))
}

/// 给区域加成员 / 改权限。新增成员只拿到**当前世代**的密钥。
pub fn grant(
    root: &Path,
    zone_name: &str,
    member: &str,
    permission: Permission,
    actor: Option<&str>,
) -> Result<Zone> {
    let session = require_session(root, actor)?;
    let mut zones = Zones::load(root)?;
    let zone = zones
        .get(zone_name)
        .ok_or_else(|| CercisError::NotFound(format!("区域不存在: {zone_name}")))?
        .clone();
    require_admin(&session, &zone)?;

    let members = Members::load(root)?;
    let rec = members
        .get(member)
        .ok_or_else(|| CercisError::NotFound(format!("成员不存在: {member}")))?;
    let public = hex_decode_32(&rec.public_key, "公钥")?;

    let current_key = session.key_for(zone_name).ok_or_else(|| {
        CercisError::Invalid(format!(
            "当前会话里没有区域 {zone_name} 的密钥（你可能不是它的成员）"
        ))
    })?;

    let z = zones.get_mut(zone_name).unwrap();
    match z.members.iter_mut().find(|m| m.id == member) {
        Some(existing) => {
            existing.permission = permission;
        }
        None => {
            let wrapped = crypto::wrap_content_key(
                &current_key,
                &public,
                &crypto::wrap_info(zone_name, member, z.generation),
            )?;
            z.members.push(ZoneMember {
                id: member.to_string(),
                permission,
                wrapped,
            });
        }
    }
    let out = z.clone();
    zones.validate()?;
    zones.save(root)?;
    log(
        root,
        &session.member,
        "update",
        "zone",
        zone_name,
        &format!(
            "区域 {} 授权 {member}（{}）",
            zone.display(),
            permission.label_cn()
        ),
    )?;
    Ok(out)
}

/// 移除成员：换一把新内容密钥并用新密钥**重新加密**区域内所有文件。
///
/// - 旧世代密钥留档（用于解旧密文、回滚），只有当时的成员能解；
/// - 被移除者从此刻起读不到新内容（他之前看过的内容无法追回）。
pub fn revoke(
    root: &Path,
    zone_name: &str,
    member: &str,
    actor: Option<&str>,
) -> Result<RevokeReport> {
    let session = require_session(root, actor)?;
    let mut zones = Zones::load(root)?;
    let zone = zones
        .get(zone_name)
        .ok_or_else(|| CercisError::NotFound(format!("区域不存在: {zone_name}")))?
        .clone();
    require_admin(&session, &zone)?;
    if zone.permission_of(member).is_none() {
        return Err(CercisError::NotFound(format!(
            "成员 {member} 本来就不在区域 {} 里",
            zone.display()
        )));
    }
    if session.key_for(zone_name).is_none() {
        return Err(CercisError::Invalid(format!(
            "会话里没有区域 {zone_name} 的钥匙，无法重新加密"
        )));
    }

    let remaining: Vec<ZoneMember> = zone
        .members
        .iter()
        .filter(|m| m.id != member)
        .cloned()
        .collect();
    if remaining.is_empty() {
        return Err(CercisError::Invalid(format!(
            "移除 {member} 后区域 {} 就没有成员了（先授权别的成员，或整块删掉该区域）",
            zone.display()
        )));
    }
    if !remaining.iter().any(|m| m.permission.can_admin()) {
        return Err(CercisError::Invalid(
            "移除后就没有 admin 了：先把另一位成员设为 admin".into(),
        ));
    }

    // 先把旧世代留档，再用新密钥重加密
    let mut keyring = Keyring::load(root, zone_name)?;
    keyring.retired.push(RetiredGeneration {
        generation: zone.generation,
        key_fingerprint: zone.key_fingerprint.clone(),
        created: zone.created.clone(),
        retired: crate::ids::now_iso8601(),
        members: zone.members.clone(),
    });
    keyring.save(root)?;

    let new_key = crypto::random_bytes(crypto::KEY_LEN);
    let new_generation = zone.generation + 1;
    let members_dir = Members::load(root)?;
    let mut new_members = Vec::new();
    for m in &remaining {
        let rec = members_dir
            .get(&m.id)
            .ok_or_else(|| CercisError::NotFound(format!("成员不存在: {}", m.id)))?;
        let public = hex_decode_32(&rec.public_key, "公钥")?;
        let wrapped = crypto::wrap_content_key(
            &new_key,
            &public,
            &crypto::wrap_info(zone_name, &m.id, new_generation),
        )?;
        new_members.push(ZoneMember {
            id: m.id.clone(),
            permission: m.permission,
            wrapped,
        });
    }

    let mut new_zone = zone.clone();
    new_zone.generation = new_generation;
    new_zone.key_fingerprint = crypto::fingerprint(&new_key);
    new_zone.rotated = crate::ids::now_iso8601();
    new_zone.members = new_members;

    // 用新密钥重加密（旧密钥从会话 / 历史世代里取）
    let old_keys = unwrap_available_keys(root, &zone, &session)?;
    let files = zone_files(root, &zone);
    let mut reencrypted = 0usize;
    for rel in &files {
        let enc = enc_path(&root.join(rel));
        if !enc.exists() {
            continue;
        }
        let blob = fs::read(&enc)?;
        let plain = decrypt_with_keys(&old_keys, &blob)?;
        let new_blob = encrypt_bytes(&new_key, &plain);
        crate::tx::write_atomic(&enc, &new_blob)?;
        reencrypted += 1;
    }

    *zones.get_mut(zone_name).unwrap() = new_zone.clone();
    zones.validate()?;
    zones.save(root)?;

    // 会话里的密钥也换成新的（当前操作者继续可用）
    if let Some(mut s) = resolve_session(root, Some(&session.member))? {
        if let Some(z) = s.zones.iter_mut().find(|z| z.name == zone_name) {
            z.content_key = hex::encode(&new_key);
            z.key_fingerprint = new_zone.key_fingerprint.clone();
        }
        write_session(root, &s)?;
    }

    log(
        root,
        &session.member,
        "update",
        "zone",
        zone_name,
        &format!(
            "区域 {} 移除成员 {member}：已换密钥（第 {} 代，指纹 {}）并重加密 {reencrypted} 个文件；\
             被移除者从此刻起读不到新内容（他之前看过的无法追回）",
            zone.display(),
            new_generation,
            new_zone.key_fingerprint
        ),
    )?;

    Ok(RevokeReport {
        zone: zone_name.to_string(),
        member: member.to_string(),
        generation: new_generation,
        key_fingerprint: new_zone.key_fingerprint.clone(),
        reencrypted,
        remaining: new_zone.member_ids(),
    })
}

/// 只改权限（不发新密钥）。
pub fn set_permission(
    root: &Path,
    zone_name: &str,
    member: &str,
    permission: Permission,
    actor: Option<&str>,
) -> Result<Zone> {
    grant(root, zone_name, member, permission, actor)
}

/// 列出区域（`visible_to` 给了成员时，只列他有权访问的）。
pub fn list_zones(root: &Path, visible_to: Option<&str>) -> Result<Vec<Zone>> {
    let zones = Zones::load(root)?;
    Ok(match visible_to {
        None => zones.zones,
        Some(m) => zones
            .zones
            .into_iter()
            .filter(|z| z.permission_of(m).is_some())
            .collect(),
    })
}

// —————————————————————————————— 解锁 / 上锁 ——————————————————————————————

/// 解锁：私钥 + 自己那份内容密钥解出来，写进**用户主目录**的会话文件。
pub fn unlock(
    root: &Path,
    member: &str,
    password: &str,
    ttl_hours: i64,
    remember: bool,
) -> Result<UnlockReport> {
    let members = Members::load(root)?;
    let rec = members
        .get(member)
        .ok_or_else(|| CercisError::NotFound(format!("成员不存在: {member}")))?;
    let (secret, _public) = load_member_secret(root, member, password)?;

    let zones = Zones::load(root)?;
    let mut unlocked = Vec::new();
    let mut notes = Vec::new();
    for z in &zones.zones {
        let Some(zm) = z.members.iter().find(|m| m.id == member) else {
            continue; // 没授权的区域连名字都不返回
        };
        match crypto::unwrap_content_key(&zm.wrapped, &secret, &z.wrap_info_for(member)) {
            Ok(key) => {
                let fp = crypto::fingerprint(&key);
                if fp != z.key_fingerprint {
                    notes.push(format!(
                        "区域 {} 的密钥指纹对不上（声明 {} / 实际 {}）",
                        z.name, z.key_fingerprint, fp
                    ));
                    continue;
                }
                unlocked.push(UnlockedZone {
                    name: z.name.clone(),
                    permission: zm.permission,
                    key_fingerprint: fp,
                    content_key: hex::encode(&key),
                });
            }
            Err(e) => notes.push(format!("区域 {} 解不开：{e}", z.name)),
        }
    }

    let ttl = if ttl_hours <= 0 {
        DEFAULT_TTL_HOURS
    } else {
        ttl_hours
    };
    let expires = (chrono::Utc::now() + chrono::Duration::hours(ttl))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let s = Session {
        format: "cercis-session/1".to_string(),
        project: fs::canonicalize(root)
            .unwrap_or_else(|_| root.to_path_buf())
            .to_string_lossy()
            .to_string(),
        member: member.to_string(),
        created: crate::ids::now_iso8601(),
        expires: expires.clone(),
        zones: unlocked.clone(),
        member_secret: hex::encode(&secret[..]),
    };
    write_session(root, &s)?;
    set_current_member(root, Some(member))?;

    if remember {
        let account = crate::keychain::account_for(&project_key(root), member);
        match crate::keychain::store(&account, password) {
            Ok(()) => notes.push("口令已交给系统钥匙串保管".to_string()),
            Err(e) => notes.push(format!("钥匙串没存上（不影响这次解锁）：{e}")),
        }
    }

    log(
        root,
        member,
        "read",
        "access",
        member,
        &format!(
            "解锁 {} 个加密区域：{}",
            unlocked.len(),
            unlocked
                .iter()
                .map(|z| format!("{}（{}）", z.name, z.permission.label_cn()))
                .collect::<Vec<_>>()
                .join(" / ")
        ),
    )?;

    let _ = rec;
    Ok(UnlockReport {
        member: member.to_string(),
        zones: unlocked,
        expires,
        session_file: session_path(root, member).display().to_string(),
        notes,
    })
}

/// 上锁：清掉会话（`forget` 时连钥匙串里的口令一起删）。
pub fn lock(root: &Path, member: Option<&str>, forget: bool) -> Result<Vec<String>> {
    let targets = match member {
        Some(m) => vec![m.to_string()],
        None => unlocked_members(root),
    };
    let mut out = Vec::new();
    for m in targets {
        let p = session_path(root, &m);
        if p.exists() {
            fs::remove_file(&p)?;
        }
        if forget {
            let account = crate::keychain::account_for(&project_key(root), &m);
            let _ = crate::keychain::delete(&account);
        }
        out.push(m);
    }
    if member.is_none() {
        set_current_member(root, None)?;
    } else if let Some(m) = member {
        if current_member(root).as_deref() == Some(m) {
            set_current_member(root, None)?;
        }
    }
    Ok(out)
}

fn write_session(root: &Path, s: &Session) -> Result<()> {
    let path = session_path(root, &s.member);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_private(&path, serde_json::to_string_pretty(s)?.as_bytes())
}

/// 用钥匙串里存着的口令自动解锁（`unlock --keychain`）。
pub fn unlock_with_keychain(
    root: &Path,
    member: &str,
    ttl_hours: i64,
) -> Result<Option<UnlockReport>> {
    let account = crate::keychain::account_for(&project_key(root), member);
    match crate::keychain::load(&account)? {
        Some(pw) => Ok(Some(unlock(root, member, &pw, ttl_hours, false)?)),
        None => Ok(None),
    }
}

// —————————————————————————————— 加密 / 解密 ——————————————————————————————

/// 加密一段内容（含文件头）。
pub fn encrypt_bytes(content_key: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let nonce = crypto::content_nonce(content_key, plaintext);
    let aad = enc_aad();
    let ct = crypto::seal(content_key, &nonce, &aad, plaintext).unwrap_or_default();
    let mut out = Vec::with_capacity(ENC_HEADER_LEN + ct.len());
    out.extend_from_slice(ENC_MAGIC);
    out.push(ENC_VERSION);
    out.push(ENC_ALG_ID);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    out
}

/// 解密一段内容。
pub fn decrypt_bytes(content_key: &[u8], blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() < ENC_HEADER_LEN || &blob[..8] != ENC_MAGIC {
        return Err(CercisError::Corrupt(
            "不是 Cercis 密文文件（文件头不对）".into(),
        ));
    }
    if blob[8] != ENC_VERSION {
        return Err(CercisError::Corrupt(format!(
            "密文版本 {} 不认识（本内核支持 {ENC_VERSION}）",
            blob[8]
        )));
    }
    if blob[9] != ENC_ALG_ID {
        return Err(CercisError::Corrupt(format!(
            "密文算法 {} 不认识",
            blob[9]
        )));
    }
    let nonce = &blob[10..22];
    crypto::open(content_key, nonce, &enc_aad(), &blob[ENC_HEADER_LEN..])
}

fn enc_aad() -> Vec<u8> {
    let mut aad = Vec::with_capacity(12);
    aad.extend_from_slice(ENC_MAGIC);
    aad.push(ENC_VERSION);
    aad.push(ENC_ALG_ID);
    aad
}

/// 密文文件路径（`tables/a.csv` → `tables/a.csv.enc`）。
pub fn enc_path(plain: &Path) -> PathBuf {
    let mut s = plain.as_os_str().to_string_lossy().to_string();
    s.push_str(ENC_SUFFIX);
    PathBuf::from(s)
}

/// 把区域里还没加密的文件加密（明文只在 `keep_plaintext` 时保留）。
pub fn apply(
    root: &Path,
    zone_filter: Option<&str>,
    member: Option<&str>,
    dry_run: bool,
    keep_plaintext: bool,
) -> Result<Vec<ZoneApplyReport>> {
    let session = require_session(root, member)?;
    let zones = Zones::load(root)?;
    let mut out = Vec::new();
    for z in &zones.zones {
        if let Some(f) = zone_filter {
            if z.name != f {
                continue;
            }
        }
        let Some(uz) = session.zone(&z.name) else {
            continue;
        };
        if !uz.permission.can_admin() {
            return Err(CercisError::Invalid(format!(
                "你在区域 {} 里是{}，加密落盘需要管理权限",
                z.display(),
                uz.permission.label_cn()
            )));
        }
        let key = session.key_for(&z.name).ok_or_else(|| {
            CercisError::Invalid(format!("会话里没有区域 {} 的密钥", z.name))
        })?;
        out.push(apply_with_key(root, z, &key, dry_run, keep_plaintext)?);
    }
    if let Some(f) = zone_filter {
        if out.is_empty() {
            return Err(CercisError::NotFound(format!(
                "没有可加密的区域 {f}（不存在，或你不在它的成员名单里）"
            )));
        }
    }
    Ok(out)
}

fn apply_with_key(
    root: &Path,
    zone: &Zone,
    key: &[u8],
    dry_run: bool,
    keep_plaintext: bool,
) -> Result<ZoneApplyReport> {
    let mut report = ZoneApplyReport {
        zone: zone.name.clone(),
        dry_run,
        encrypted: Vec::new(),
        kept_plaintext: Vec::new(),
        already: 0,
        bytes: 0,
        objects_encrypted: 0,
    };
    for rel in zone_files(root, zone) {
        let plain = root.join(&rel);
        let enc = enc_path(&plain);
        if enc.exists() {
            report.already += 1;
            continue;
        }
        let bytes = fs::read(&plain)?;
        let blob = encrypt_bytes(key, &bytes);
        if dry_run {
            report.encrypted.push(rel.clone());
            report.bytes += bytes.len() as u64;
            continue;
        }
        crate::tx::write_atomic(&enc, &blob)?;
        // 回读验证：解开必须与原文逐字节一致，才允许删明文
        let check = decrypt_bytes(key, &blob)?;
        if check != bytes {
            return Err(CercisError::Corrupt(format!(
                "加密后回读校验失败：{rel}（已保留明文，未删除任何东西）"
            )));
        }
        report.encrypted.push(rel.clone());
        report.bytes += bytes.len() as u64;
        if keep_plaintext {
            report.kept_plaintext.push(rel.clone());
        } else {
            fs::remove_file(&plain)?;
        }
    }
    // 顺带把"加密之前存下的历史快照"也换成密文（否则对象库就是一份明文备份）
    let hist = encrypt_zone_history(root, zone, key, dry_run)?;
    report.objects_encrypted = hist.encrypted;
    if !dry_run && !report.encrypted.is_empty() {
        log(
            root,
            &current_member(root).unwrap_or_else(|| "local-user".into()),
            "update",
            "zone",
            &zone.name,
            &format!(
                "加密区域 {}：加密 {} 个文件（{}{}）",
                zone.display(),
                report.encrypted.len(),
                if report.kept_plaintext.is_empty() {
                    "明文已移除"
                } else {
                    "按要求保留了明文副本"
                },
                if report.already > 0 {
                    format!("，另有 {} 个已是密文", report.already)
                } else {
                    String::new()
                }
            ),
        )?;
    }
    if !dry_run && hist.encrypted > 0 {
        log(
            root,
            &current_member(root).unwrap_or_else(|| "local-user".into()),
            "update",
            "zone",
            &zone.name,
            &format!(
                "加密区域 {}：把 {} 个历史快照从明文换成密文（老指纹记在 {}）",
                zone.display(),
                hist.encrypted,
                OBJECT_MAP_REL
            ),
        )?;
    }
    Ok(report)
}

/// 把与区域相关的历史对象加密，并登记"老指纹 → 新指纹"。
///
/// 只处理**流水里引用到的**对象（`hash_before` / `hash_after`），
/// 而且只处理属于本区域内那些表的行——别的表的历史不动。
pub fn encrypt_zone_history(
    root: &Path,
    zone: &Zone,
    key: &[u8],
    dry_run: bool,
) -> Result<HistoryEncryptReport> {
    let mut report = HistoryEncryptReport::default();
    let objects_dir = root.join("history").join("objects");
    if !objects_dir.exists() {
        return Ok(report);
    }
    let Some(log_text) = crate::tx::read_optional(&root.join("tables/change_log.csv"))? else {
        return Ok(report);
    };
    let table = crate::table::Table::from_csv_text(&root.join("tables/change_log.csv"), &log_text)?;

    let mut hashes: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for row in &table.rows {
        let target = row.get("target_table").cloned().unwrap_or_default();
        if target.is_empty() {
            continue;
        }
        let rel = format!("tables/{target}.csv");
        if !zone.covers(&rel) {
            continue;
        }
        for field in ["hash_before", "hash_after"] {
            if let Some(h) = row.get(field) {
                if h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) {
                    hashes.insert(h.to_lowercase());
                }
            }
        }
    }
    if hashes.is_empty() {
        return Ok(report);
    }

    let mut map = ZoneObjects::load(root)?;
    for (idx, old) in hashes.into_iter().enumerate() {
        // 已经迁过的对象：跳过（幂等）
        if map.aliases.contains_key(&old) {
            report.already += 1;
            continue;
        }
        let path = objects_dir.join(&old[..2]).join(&old);
        let Ok(bytes) = fs::read(&path) else {
            report.missing += 1;
            continue;
        };
        if bytes.starts_with(ENC_MAGIC) {
            report.already += 1;
            map.owners.insert(old.clone(), zone.name.clone());
            continue;
        }
        report.encrypted += 1;
        if dry_run {
            continue;
        }
        let blob = encrypt_bytes(key, &bytes);
        let new_hash = crypto::sha256_hex(&blob);
        let new_path = objects_dir.join(&new_hash[..2]).join(&new_hash);
        crate::tx::write_atomic(&new_path, &blob)?;
        // 回读校验通过后才删老对象
        let check = decrypt_bytes(key, &blob)?;
        if check != bytes {
            return Err(CercisError::Corrupt(format!(
                "历史对象加密后回读校验失败：{old}（老对象保留未删）"
            )));
        }
        fs::remove_file(&path)?;
        map.aliases.insert(old, new_hash.clone());
        map.owners.insert(new_hash, zone.name.clone());
        let _ = idx;
    }
    if !dry_run {
        map.save(root)?;
    }
    Ok(report)
}

/// 取某个区域的内容密钥（从已解锁的会话里取）。
pub fn key_for_zone(
    root: &Path,
    zone: &str,
    member: Option<&str>,
) -> Result<Zeroizing<Vec<u8>>> {
    let session = require_session(root, member)?;
    session.key_for(zone).ok_or_else(|| {
        CercisError::Invalid(format!(
            "成员 {} 没有区域 {zone} 的钥匙（他不在成员名单里，或还没解锁）",
            session.member
        ))
    })
}

/// 历史对象是否已加密，以及它属于哪个区域（供内核透明还原用）。
pub fn object_zone(root: &Path, hash: &str) -> Result<Option<(String, String)>> {
    let map = ZoneObjects::load(root)?;
    let real = map.aliases.get(hash).cloned().unwrap_or_else(|| hash.to_string());
    Ok(map.owners.get(&real).map(|z| (real, z.clone())))
}

/// 把历史对象里的"老指纹"换成"现在的密文指纹"。
pub fn resolve_object_hash(root: &Path, hash: &str) -> Result<String> {
    let map = ZoneObjects::load(root)?;
    Ok(map
        .aliases
        .get(hash)
        .cloned()
        .unwrap_or_else(|| hash.to_string()))
}

/// 解一份历史对象：不是密文就原样返回；是密文就用当前会话里的区域钥匙试。
pub fn decrypt_object(root: &Path, member: Option<&str>, blob: &[u8]) -> Result<Vec<u8>> {
    if !blob.starts_with(ENC_MAGIC) {
        return Ok(blob.to_vec());
    }
    let session = require_session(root, member)?;
    let mut tried = 0usize;
    for z in &session.zones {
        if let Ok(key) = hex::decode(&z.content_key) {
            tried += 1;
            if let Ok(plain) = decrypt_bytes(&key, blob) {
                return Ok(plain);
            }
        }
    }
    Err(CercisError::Invalid(format!(
        "这份历史快照是密文，当前会话（{}，试过 {tried} 个区域）解不开。\n\
         - 解锁对应的加密区域后再试：cercis share unlock <项目> --member <成员>",
        session.member
    )))
}

/// 解密导出：把区域内容解密到 `exports/`（给 R / AI / 投稿用的明文副本）。
pub fn export_zone(
    root: &Path,
    zone_name: &str,
    member: Option<&str>,
    dest_rel: Option<&str>,
    dry_run: bool,
) -> Result<ExportReport> {
    let session = require_session(root, member)?;
    let zones = Zones::load(root)?;
    let zone = zones
        .get(zone_name)
        .ok_or_else(|| CercisError::NotFound(format!("区域不存在: {zone_name}")))?;
    let uz = session
        .zone(zone_name)
        .ok_or_else(|| CercisError::Invalid(format!("你不在区域 {} 的成员名单里", zone.display())))?;
    if !uz.permission.can_read() {
        return Err(CercisError::Invalid("你没有读这个区域的权限".into()));
    }
    let key = session
        .key_for(zone_name)
        .ok_or_else(|| CercisError::Invalid("会话里没有该区域的密钥".to_string()))?;

    let dest = PathBuf::from(
        dest_rel
            .map(|d| d.to_string())
            .unwrap_or_else(|| format!("exports/{}", zone.name)),
    );
    let mut report = ExportReport {
        zone: zone.name.clone(),
        member: session.member.clone(),
        dest: dest.display().to_string(),
        files: Vec::new(),
        bytes: 0,
        dry_run,
    };
    for rel in zone_files(root, zone) {
        let enc = enc_path(&root.join(rel.clone()));
        if !enc.exists() {
            continue;
        }
        let blob = fs::read(&enc)?;
        let plain = decrypt_bytes(&key, &blob)?;
        report.bytes += plain.len() as u64;
        report.files.push(rel.clone());
        if dry_run {
            continue;
        }
        let out = root.join(&dest).join(&rel);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::tx::write_atomic(&out, &plain)?;
    }
    if !dry_run {
        log(
            root,
            &session.member,
            "export",
            "zone",
            &zone.name,
            &format!(
                "解密导出：{} → {}（{} 个文件，{} 字节；明文副本只放在项目 exports/ 下，写完记得清理）",
                zone.display(),
                dest.display(),
                report.files.len(),
                report.bytes
            ),
        )?;
    }
    Ok(report)
}

// —————————————————————————————— 内核透明读写 ——————————————————————————————

/// 某个相对路径是否归某个加密区域管（返回区域）。
pub fn zone_covering(root: &Path, rel: &str) -> Result<Option<Zone>> {
    Ok(Zones::load(root)?.covering(rel).cloned())
}

/// 透明读：`Ok(None)` 表示"不归加密区域管，按普通文件读"。
///
/// - 区域内的密文：解密后返回；
/// - 区域内还有明文（尚未 `apply`）：**报错并给出命令**，不让明文假装没事；
/// - 没解锁：报错并提示解锁命令。
pub fn read_through_zone(root: &Path, member: Option<&str>, rel: &str) -> Result<Option<Vec<u8>>> {
    let Some(zone) = zone_covering(root, rel)? else {
        return Ok(None);
    };
    let enc = enc_path(&root.join(rel));
    if !enc.exists() {
        return Err(CercisError::Invalid(format!(
            "「{rel}」在加密区域 {} 里，但磁盘上还是明文（没加密）。\n\
             - 处理：cercis share apply <项目> --zone {}（管理员执行；会把明文换成密文）",
            zone.display(),
            zone.name
        )));
    }
    let session = resolve_session(root, member)?;
    let Some(s) = session else {
        return Err(CercisError::Invalid(format!(
            "「{rel}」在加密区域 {} 里，当前未解锁。\n\
             - 解锁：cercis share unlock <项目> --member <成员>",
            zone.display()
        )));
    };
    let Some(uz) = s.zone(&zone.name) else {
        return Err(CercisError::Invalid(format!(
            "成员 {} 没有区域 {} 的权限",
            s.member,
            zone.display()
        )));
    };
    let key = s
        .key_for(&zone.name)
        .ok_or_else(|| CercisError::Invalid("会话里没有该区域的密钥".to_string()))?;
    let blob = fs::read(&enc)?;
    let plain = decrypt_bytes(&key, &blob).map_err(|_| {
        CercisError::Invalid(format!(
            "「{rel}」解不开：你手上的是旧钥匙，而区域 {} 已经是第 {} 代（指纹 {}）。\n\
             - 重新解锁：cercis share unlock <项目> --member {}\n\
             - 如果你已被移出这个区域，请联系管理员重新授权",
            zone.display(),
            zone.generation,
            zone.key_fingerprint,
            s.member
        ))
    })?;
    let _ = uz;
    Ok(Some(plain))
}

/// 透明写：把内容写进区域（密文落盘，明文不留）。
///
/// - `Ok(None)` 表示"不归加密区域管，按普通文件写"；
/// - `Ok(Some(bytes))` 返回实际落盘的密文字节（流水指纹用）；
/// - 权限不足或未解锁都会给出可操作的中文提示。
pub fn write_through_zone(
    root: &Path,
    member: Option<&str>,
    rel: &str,
    bytes: &[u8],
) -> Result<Option<Vec<u8>>> {
    let Some(zone) = zone_covering(root, rel)? else {
        return Ok(None);
    };
    let s = require_session(root, member)?;
    let uz = s.zone(&zone.name).ok_or_else(|| {
        CercisError::Invalid(format!(
            "成员 {} 没有区域 {} 的权限",
            s.member,
            zone.display()
        ))
    })?;
    if !uz.permission.can_write() {
        return Err(CercisError::Invalid(format!(
            "你在区域 {} 里是{}，不能写入。\n\
             - 要写就请管理员改权限：cercis share grant <项目> --zone {} --member {} --permission write",
            zone.display(),
            uz.permission.label_cn(),
            zone.name,
            s.member
        )));
    }
    let key = s
        .key_for(&zone.name)
        .ok_or_else(|| CercisError::Invalid("会话里没有该区域的密钥".to_string()))?;
    let blob = encrypt_bytes(&key, bytes);
    let plain = root.join(rel);
    let enc = enc_path(&plain);
    crate::tx::write_atomic(&enc, &blob)?;
    if plain.exists() {
        fs::remove_file(&plain)?;
    }
    Ok(Some(blob))
}

// —————————————————————————————— 状态 ——————————————————————————————

/// 项目加密状态（体检 / app 面板用）。
pub fn status(root: &Path, member: Option<&str>) -> Result<AccessStatus> {
    let members = Members::load(root)?.members;
    let zones = Zones::load(root)?;
    let session = resolve_session(root, member)?;
    // 谁在看：显式指定的成员优先，其次"当前解锁的成员"
    let viewer: Option<String> = member
        .map(|m| m.to_string())
        .or_else(|| session.as_ref().map(|s| s.member.clone()));
    let mut out = Vec::new();
    for z in &zones.zones {
        let permission = session
            .as_ref()
            .and_then(|s| s.zone(&z.name))
            .map(|uz| uz.permission)
            .or_else(|| viewer.as_deref().and_then(|m| z.permission_of(m)));
        // 未授权成员连区域的存在都不该知道（没指定人 = 站在"整个项目"的角度看）
        let visible = permission.is_some() || viewer.is_none();
        if !visible {
            continue;
        }
        let mut files = 0usize;
        let mut bytes = 0u64;
        let mut plaintext_left = Vec::new();
        let mut unreadable = 0usize;
        let keys = match session.as_ref() {
            Some(s) if s.zone(&z.name).is_some() => {
                unwrap_available_keys(root, z, s).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        for rel in zone_files(root, z) {
            let plain = root.join(&rel);
            let enc = enc_path(&plain);
            if enc.exists() {
                files += 1;
                bytes += fs::metadata(&enc).map(|m| m.len()).unwrap_or(0);
                if !keys.is_empty() {
                    if let Ok(blob) = fs::read(&enc) {
                        if decrypt_with_keys(&keys, &blob).is_err() {
                            unreadable += 1;
                        }
                    }
                }
            } else if plain.exists() {
                plaintext_left.push(rel.clone());
            }
        }
        out.push(ZoneStatus {
            zone: z.name.clone(),
            label: z.label.clone(),
            generation: z.generation,
            key_fingerprint: z.key_fingerprint.clone(),
            members: z
                .members
                .iter()
                .map(|m| (m.id.clone(), m.permission))
                .collect(),
            permission,
            files,
            bytes,
            plaintext_left,
            unreadable,
        });
    }
    Ok(AccessStatus {
        members,
        zones: out,
        session_member: session.map(|s| s.member),
        share_home: share_home().display().to_string(),
        keychain_available: crate::keychain::available(),
    })
}

/// 体检：加密相关的问题清单（`cercis doctor` / app 用）。
pub fn doctor_issues(root: &Path) -> Result<Vec<String>> {
    let mut issues = Vec::new();
    let zones = Zones::load(root)?;
    if zones.zones.is_empty() {
        return Ok(issues);
    }
    for z in &zones.zones {
        for rel in zone_files(root, z) {
            let plain = root.join(&rel);
            if plain.exists() && !enc_path(&plain).exists() {
                issues.push(format!(
                    "加密区域 {} 里有明文文件：{rel}（处理：cercis share apply <项目> --zone {}）",
                    z.display(),
                    z.name
                ));
            }
        }
        if z.key_fingerprint.len() != 8 {
            issues.push(format!("区域 {} 的密钥指纹异常（可能被手改过）", z.name));
        }
    }
    if resolve_session(root, None)?.is_none() {
        issues.push(format!(
            "本项目有 {} 个加密区域，当前未解锁（解锁：cercis share unlock <项目> --member <成员>）",
            zones.zones.len()
        ));
    }
    Ok(issues)
}

// —————————————————————————————— 内部工具 ——————————————————————————————

fn require_admin(session: &Session, zone: &Zone) -> Result<()> {
    let uz = session.zone(&zone.name).ok_or_else(|| {
        CercisError::Invalid(format!(
            "成员 {} 不在区域 {} 的成员名单里",
            session.member,
            zone.display()
        ))
    })?;
    if !uz.permission.can_admin() {
        return Err(CercisError::Invalid(format!(
            "成员 {} 在区域 {} 里是{}，改成员名单需要 admin",
            session.member,
            zone.display(),
            uz.permission.label_cn()
        )));
    }
    Ok(())
}

/// 当前成员手上**能解开这个区域的所有钥匙**：当前世代 + 历史世代里发给他的那些。
///
/// 用途：换密钥/修复对账时，旧密文要先用旧钥匙解开、再用新钥匙写回去。
fn unwrap_available_keys(
    root: &Path,
    zone: &Zone,
    session: &Session,
) -> Result<Vec<Zeroizing<Vec<u8>>>> {
    let mut keys = Vec::new();
    if let Some(k) = session.key_for(&zone.name) {
        keys.push(k);
    }
    // 历史世代：用会话里的私钥把当年发给自己的那些钥匙解出来
    // （换密钥之后，老密文就是靠它们才能重加密到新密钥）
    if let Ok(secret_bytes) = hex::decode(&session.member_secret) {
        if secret_bytes.len() == 32 {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&secret_bytes);
            let keyring = Keyring::load(root, &zone.name)?;
            for gen in &keyring.retired {
                let Some(m) = gen.members.iter().find(|m| m.id == session.member) else {
                    continue;
                };
                let info = crypto::wrap_info(&zone.name, &session.member, gen.generation);
                if let Ok(k) = crypto::unwrap_content_key(&m.wrapped, &arr, &info) {
                    keys.push(k);
                }
            }
        }
    }
    if keys.is_empty() {
        return Err(CercisError::Invalid(format!(
            "会话里没有区域 {} 的钥匙（先 unlock，或你不在成员名单里）",
            zone.name
        )));
    }
    Ok(keys)
}

/// 依次用候选钥匙解密，第一把成功的算数。
fn decrypt_with_keys(keys: &[Zeroizing<Vec<u8>>], blob: &[u8]) -> Result<Vec<u8>> {
    let mut last_err = None;
    for k in keys {
        match decrypt_bytes(k, blob) {
            Ok(p) => return Ok(p),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| CercisError::Corrupt("没有可用的钥匙".into())))
}

/// 用**当前世代的密钥**把区域内所有密文重新写一遍（修复"密钥对不上"的密文）。
pub fn rewrap(
    root: &Path,
    zone_name: &str,
    actor: Option<&str>,
) -> Result<(usize, usize, String)> {
    let session = require_session(root, actor)?;
    let zones = Zones::load(root)?;
    let zone = zones
        .get(zone_name)
        .ok_or_else(|| CercisError::NotFound(format!("区域不存在: {zone_name}")))?
        .clone();
    require_admin(&session, &zone)?;
    let key = session.key_for(zone_name).ok_or_else(|| {
        CercisError::Invalid(format!("会话里没有区域 {zone_name} 的钥匙"))
    })?;
    let keys = unwrap_available_keys(root, &zone, &session)?;
    let mut rewritten = 0usize;
    let mut failed = 0usize;
    for rel in zone_files(root, &zone) {
        let enc = enc_path(&root.join(&rel));
        if !enc.exists() {
            continue;
        }
        let blob = fs::read(&enc)?;
        match decrypt_with_keys(&keys, &blob) {
            Ok(plain) => {
                let new_blob = encrypt_bytes(&key, &plain);
                if new_blob != blob {
                    crate::tx::write_atomic(&enc, &new_blob)?;
                    rewritten += 1;
                }
            }
            Err(_) => failed += 1,
        }
    }
    if failed > 0 {
        return Err(CercisError::Invalid(format!(
            "{failed} 个文件用你手上的钥匙解不开：你不是它们的授权成员，或它们来自更早的世代。\
             请让仍持有旧钥匙的成员执行一次 share rewrap"
        )));
    }
    log(
        root,
        &session.member,
        "update",
        "zone",
        zone_name,
        &format!(
            "重新加密区域 {}：{rewritten} 个文件统一到第 {} 代密钥（指纹 {}）",
            zone.display(),
            zone.generation,
            zone.key_fingerprint
        ),
    )?;
    Ok((rewritten, failed, zone.key_fingerprint))
}

fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

fn hex_decode_32(s: &str, what: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(s).map_err(|e| CercisError::Corrupt(format!("{what}不是十六进制: {e}")))?;
    if bytes.len() != 32 {
        return Err(CercisError::Corrupt(format!(
            "{what}长度应为 32 字节，实际 {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// 写文件并设为仅本人可读（0600）。
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    crate::tx::write_atomic(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// 写一条流水（动作落在 change_log 里，与其他操作同一本账）。
fn log(root: &Path, user: &str, action: &str, kind: &str, id: &str, summary: &str) -> Result<()> {
    let store = Store::new(
        root,
        Identity {
            user_id: user.to_string(),
            device_id: "local-device".to_string(),
            record_device: false,
        },
    );
    store.append_change(&ChangeEntry::new(
        user,
        "-",
        action,
        kind,
        id,
        summary,
        "",
        "",
    ))
}

/// 列出区域里当前存在的文件（相对路径，已排除内核自用目录与密文本身）。
pub fn zone_files(root: &Path, zone: &Zone) -> Vec<String> {
    let mut all = Vec::new();
    walk(root, root, &mut all);
    let mut set = std::collections::BTreeSet::new();
    for rel in all {
        // 密文文件 `x.csv.enc` 还原成它代表的那份明文 `x.csv`
        let plain = match rel.strip_suffix(ENC_SUFFIX) {
            Some(stem) => stem.to_string(),
            None => rel.clone(),
        };
        if is_internal(&plain) || !zone.covers(&plain) {
            continue;
        }
        if root.join(&plain).exists() || enc_path(&root.join(&plain)).exists() {
            set.insert(plain);
        }
    }
    set.into_iter().collect()
}

fn is_internal(rel: &str) -> bool {
    let first = rel.split('/').next().unwrap_or("");
    INTERNAL_DIRS.contains(&first)
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if path.is_dir() {
            if INTERNAL_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk(root, &path, out);
        } else if !name.ends_with(crate::tx::TMP_SUFFIX) {
            out.push(rel);
        }
    }
}

/// 路径规则匹配：`*` 匹配一段内的任意字符，`**` 跨目录，`dir/` 表示整棵子树。
pub fn matches_path(pattern: &str, rel: &str) -> bool {
    let raw = pattern.trim().trim_start_matches("./");
    let rel = rel.trim().trim_start_matches("./");
    // 以 `/` 结尾表示"整棵子树"（如 methods/ 覆盖 methods 下所有层级）
    let subtree = raw.ends_with('/');
    let pat = raw.trim_end_matches('/');
    if pat.is_empty() {
        return false;
    }
    if subtree {
        return rel == pat || rel.starts_with(&format!("{pat}/"));
    }
    if pat == rel {
        return true;
    }
    let p: Vec<&str> = pat.split('/').collect();
    let r: Vec<&str> = rel.split('/').collect();
    segments_match(&p, &r)
}

fn segments_match(p: &[&str], r: &[&str]) -> bool {
    if p.is_empty() {
        return r.is_empty();
    }
    if p[0] == "**" {
        for i in 0..=r.len() {
            if segments_match(&p[1..], &r[i..]) {
                return true;
            }
        }
        return false;
    }
    if r.is_empty() {
        return false;
    }
    if !segment_match(p[0], r[0]) {
        return false;
    }
    segments_match(&p[1..], &r[1..])
}

fn segment_match(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut star, mut mark) = (None, 0usize);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(si) = star {
            i = si + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

/// 项目里的成员与区域一览（app 面板用）。
pub fn summary_map(root: &Path) -> Result<BTreeMap<String, usize>> {
    let zones = Zones::load(root)?;
    let mut out = BTreeMap::new();
    for z in &zones.zones {
        out.insert(z.name.clone(), z.members.len());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_rules_cover_common_shapes() {
        assert!(matches_path("tables/patent_*.csv", "tables/patent_2026.csv"));
        assert!(!matches_path("tables/patent_*.csv", "tables/other.csv"));
        assert!(!matches_path("tables/*.csv", "tables/sub/a.csv"));
        assert!(matches_path("tables/**", "tables/sub/a.csv"));
        assert!(matches_path("methods/", "methods/sop/a.md"));
        assert!(matches_path("media/**/raw/*.png", "media/2026/raw/x.png"));
        assert!(!matches_path("media/**/raw/*.png", "media/2026/raw/x.jpg"));
        assert!(matches_path("**/*.docx", "a/b/c.docx"));
    }

    #[test]
    fn permission_levels() {
        assert!(Permission::Read.can_read());
        assert!(!Permission::Read.can_write());
        assert!(Permission::Write.can_write());
        assert!(!Permission::Write.can_admin());
        assert!(Permission::Admin.can_admin());
        assert_eq!(Permission::parse("只读").unwrap(), Permission::Read);
    }

    #[test]
    fn enc_file_roundtrip() {
        let key = crypto::random_bytes(32);
        let plain = "sample_id,height\nS1,12.5\n".as_bytes();
        let blob = encrypt_bytes(&key, plain);
        assert_eq!(&blob[..8], ENC_MAGIC);
        assert_eq!(decrypt_bytes(&key, &blob).unwrap(), plain);
        // 换把钥匙解不开
        let other = crypto::random_bytes(32);
        assert!(decrypt_bytes(&other, &blob).is_err());
        // 同一明文 → 同一密文（可对账）
        assert_eq!(blob, encrypt_bytes(&key, plain));
    }
}
