//! 多人账户与权限分级（FR-N08 / FR-N09 / FR-N11）。
//!
//! 场景：NAS 上多人先后编辑同一个项目。做法是**账户 + 口令**，但：
//! - 账户文件随项目走（`access/accounts.yaml`），**只存加盐哈希**，不存明文口令；
//! - 没有账户文件 = **开放模式**（单人用，一切照旧，不做任何限制）；
//! - 有账户文件 = 必须登录；按角色放行：
//!
//! | 角色 | 能做什么 |
//! |------|----------|
//! | 查看者 viewer | 只读：查询、浏览、导出、打包 |
//! | 编辑者 editor | 以上 + 写数据（录入、更正、导入、合并） |
//! | 管理员 admin | 以上 + 改结构（模板、字段类型、删空表）+ 管插件 + 管加密区域 + 管账户 |
//!
//! 两个细节：
//! 1. **登录态放在用户主目录**（`~/.cercis/share/projects/<项目>/account.json`，0600），
//!    不进项目、不随分享包走；`logout` 即抹掉。
//! 2. **越权要留痕**：被拒绝的写操作会在 `change_log` 里记一条 `denied`，
//!    这样"谁想改但没改成"也查得到。
//!
//! FR-N11：项目配置 `plugins.admin_only_edit: true` 时，插件目录的改动（装、停用、屏蔽、
//! 市场安装）只允许管理员——没有账户却打开这个开关会被明确拒绝（配了也管不住的开关，
//! 不如报错让人把账户配齐）。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::crypto::KdfParams;
use crate::{CercisError, Result};

/// 账户文件（项目内）。
pub const ACCOUNTS_REL: &str = "access/accounts.yaml";

/// 角色（FR-N09）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Viewer,
    Editor,
    Admin,
}

impl Default for Role {
    fn default() -> Self {
        Role::Viewer
    }
}

impl Role {
    pub fn parse(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "viewer" | "read" | "查看者" | "只读" => Ok(Role::Viewer),
            "editor" | "write" | "编辑者" | "编辑" => Ok(Role::Editor),
            "admin" | "管理员" => Ok(Role::Admin),
            other => Err(CercisError::Invalid(format!(
                "角色只能是 viewer / editor / admin，实际: {other}"
            ))),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Viewer => "viewer",
            Role::Editor => "editor",
            Role::Admin => "admin",
        }
    }

    pub fn label_cn(&self) -> &'static str {
        match self {
            Role::Viewer => "查看者",
            Role::Editor => "编辑者",
            Role::Admin => "管理员",
        }
    }

    fn rank(&self) -> u8 {
        match self {
            Role::Viewer => 0,
            Role::Editor => 1,
            Role::Admin => 2,
        }
    }

    /// `self` 是否够得上 `need`。
    pub fn allows(&self, need: Role) -> bool {
        self.rank() >= need.rank()
    }
}

/// 一个账户（**只有加盐哈希，没有明文口令**）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub role: Role,
    /// 盐（十六进制）
    pub salt: String,
    /// Argon2id 原始输出（十六进制）
    pub hash: String,
    pub created: String,
    #[serde(default)]
    pub note: String,
}

/// `access/accounts.yaml`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Accounts {
    pub format: String,
    #[serde(default)]
    pub accounts: Vec<Account>,
}

impl Default for Accounts {
    fn default() -> Self {
        Self {
            format: "cercis-accounts/1".to_string(),
            accounts: Vec::new(),
        }
    }
}

impl Accounts {
    pub fn load(root: &Path) -> Result<Self> {
        match crate::tx::read_optional(&root.join(ACCOUNTS_REL))? {
            None => Ok(Self::default()),
            Some(text) => serde_yaml::from_str(&text)
                .map_err(|e| CercisError::Corrupt(format!("{ACCOUNTS_REL} 解析失败: {e}"))),
        }
    }

    pub fn save(&self, root: &Path) -> Result<()> {
        let path = root.join(ACCOUNTS_REL);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        crate::tx::write_atomic_str(&path, &serde_yaml::to_string(self)?)
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// 有没有启用多人账户（没有 = 开放模式）。
    pub fn enabled(&self) -> bool {
        !self.accounts.is_empty()
    }
}

/// 登录态（存在用户主目录，不进项目）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub role: Role,
    pub display_name: String,
    pub created: String,
    pub expires: String,
}

impl Session {
    fn expired(&self) -> bool {
        match crate::ids::parse_iso8601(&self.expires) {
            Ok(t) => t <= chrono::Utc::now(),
            Err(_) => true,
        }
    }
}

/// 账户文件所在路径（给体检/文档用）。
pub fn accounts_file(root: &Path) -> PathBuf {
    root.join(ACCOUNTS_REL)
}

/// 新增账户（第一个账户必须是管理员，否则谁也管不了）。
pub fn add(
    root: &Path,
    id: &str,
    display_name: &str,
    role: Role,
    password: &str,
    params: KdfParams,
    note: &str,
) -> Result<Account> {
    let id = id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err(CercisError::Invalid(
            "账户 id 只能是 ASCII 字母数字、下划线、短横线（如 alice）".into(),
        ));
    }
    let mut accounts = Accounts::load(root)?;
    if accounts.get(id).is_some() {
        return Err(CercisError::Conflict(format!(
            "账户 {id} 已存在（改口令用 account passwd）"
        )));
    }
    if !accounts.enabled() && role != Role::Admin {
        return Err(CercisError::Invalid(
            "第一个账户必须是 admin（否则以后没人能管账户）".into(),
        ));
    }
    let salt = crate::crypto::random_bytes(crate::crypto::SALT_LEN);
    let hash = crate::crypto::derive_key(password, &salt, params)?;
    let acc = Account {
        id: id.to_string(),
        display_name: display_name.trim().to_string(),
        role,
        salt: hex::encode(&salt),
        hash: hex::encode(&hash[..]),
        created: crate::ids::now_iso8601(),
        note: note.to_string(),
    };
    accounts.accounts.push(acc.clone());
    accounts.save(root)?;
    log(
        root,
        id,
        "create",
        "account",
        id,
        &format!("新建账户 {id}（{}）", role.label_cn()),
    )?;
    Ok(acc)
}

/// 校验口令。
pub fn verify(root: &Path, id: &str, password: &str, params: KdfParams) -> Result<bool> {
    let accounts = Accounts::load(root)?;
    let acc = accounts
        .get(id)
        .ok_or_else(|| CercisError::NotFound(format!("没有账户 {id}")))?;
    let salt = hex::decode(&acc.salt)
        .map_err(|e| CercisError::Corrupt(format!("账户盐不是十六进制: {e}")))?;
    let expect = hex::decode(&acc.hash)
        .map_err(|e| CercisError::Corrupt(format!("账户哈希不是十六进制: {e}")))?;
    let got = crate::crypto::derive_key(password, &salt, params)?;
    Ok(constant_time_eq(&got[..], &expect))
}

/// 登录：口令对了就把登录态写进用户主目录。
pub fn login(
    root: &Path,
    id: &str,
    password: &str,
    params: KdfParams,
    ttl_hours: i64,
) -> Result<Session> {
    let accounts = Accounts::load(root)?;
    if !accounts.enabled() {
        return Err(CercisError::Invalid(
            "这个项目还没有启用账户（开放模式）：需要时先 cercis account add <项目> --id alice --role admin"
                .into(),
        ));
    }
    let acc = accounts
        .get(id)
        .ok_or_else(|| CercisError::NotFound(format!("没有账户 {id}")))?;
    if !verify(root, id, password, params)? {
        log(root, id, "denied", "account", id, "登录失败：口令不对")?;
        return Err(CercisError::Invalid("口令不对".into()));
    }
    let ttl = if ttl_hours <= 0 { 12 } else { ttl_hours };
    let s = Session {
        id: acc.id.clone(),
        role: acc.role,
        display_name: acc.display_name.clone(),
        created: crate::ids::now_iso8601(),
        expires: (chrono::Utc::now() + chrono::Duration::hours(ttl))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    };
    write_session(root, &s)?;
    log(
        root,
        &s.id,
        "read",
        "account",
        &s.id,
        &format!("登录（{}）", s.role.label_cn()),
    )?;
    Ok(s)
}

/// 登出：清掉本机登录态。
pub fn logout(root: &Path) -> Result<bool> {
    let path = session_path(root);
    if path.exists() {
        fs::remove_file(&path)?;
        return Ok(true);
    }
    Ok(false)
}

/// 当前登录态（过期即视为没有）。
pub fn current(root: &Path) -> Result<Option<Session>> {
    let path = session_path(root);
    let Some(text) = crate::tx::read_optional(&path)? else {
        return Ok(None);
    };
    let s: Session = serde_json::from_str(&text)
        .map_err(|e| CercisError::Corrupt(format!("登录态解析失败: {e}")))?;
    if s.expired() {
        let _ = fs::remove_file(&path);
        return Ok(None);
    }
    Ok(Some(s))
}

/// **权限闸门**：开放模式一律放行；有账户就必须登录且角色够。
pub fn require(root: &Path, need: Role) -> Result<()> {
    let accounts = Accounts::load(root)?;
    if !accounts.enabled() {
        return Ok(()); // 开放模式（单人用）
    }
    match current(root)? {
        Some(s) if s.role.allows(need) => Ok(()),
        Some(s) => {
            let _ = log(
                root,
                &s.id,
                "denied",
                "permission",
                need.as_str(),
                &format!(
                    "越权被拒绝：{} 想做需要「{}」的事",
                    s.role.label_cn(),
                    need.label_cn()
                ),
            );
            Err(CercisError::Invalid(format!(
                "权限不足：你以{}身份登录，这个操作需要「{}」。\n\
                 - 换管理员登录：cercis account login <项目> --id <管理员>\n\
                 - 或让管理员改你的角色：cercis account role <项目> --id {} --role {}",
                s.role.label_cn(),
                need.label_cn(),
                s.id,
                need.as_str()
            )))
        }
        None => Err(CercisError::Invalid(format!(
            "这个项目启用了账户（{} 人）：先登录再操作。\n\
             - 登录：cercis account login <项目> --id <你的账户>\n\
             - 看有哪些账户：cercis account list <项目>",
            accounts.accounts.len()
        ))),
    }
}

/// FR-N11：插件目录的改动够不够格（`plugins.admin_only_edit`）。
pub fn require_plugin_edit(root: &Path) -> Result<()> {
    let accounts = Accounts::load(root)?;
    let admin_only = crate::tx::read_optional(&root.join("project.yaml"))?
        .map(|t| {
            crate::config::ProjectConfig::from_yaml(&t)
                .map(|c| c.plugins.admin_only_edit)
                .unwrap_or(false)
        })
        .unwrap_or(false);
    if admin_only && !accounts.enabled() {
        return Err(CercisError::Invalid(
            "项目开了「仅管理员可改插件目录」，但还没启用账户——管不了的开关不如不开：\n\
             - 先建管理员：cercis account add <项目> --id alice --role admin\n\
             - 或把 project.yaml 里的 plugins.admin_only_edit 改回 false"
                .into(),
        ));
    }
    if admin_only {
        return require(root, Role::Admin);
    }
    Ok(())
}

/// 改角色（要管理员）。
pub fn set_role(root: &Path, id: &str, role: Role, actor: Option<&str>) -> Result<Account> {
    require_admin_as(root, actor, "改账户角色")?;
    let mut accounts = Accounts::load(root)?;
    let idx = accounts
        .accounts
        .iter()
        .position(|a| a.id == id)
        .ok_or_else(|| CercisError::NotFound(format!("没有账户 {id}")))?;
    accounts.accounts[idx].role = role;
    if !accounts.accounts.iter().any(|a| a.role == Role::Admin) {
        return Err(CercisError::Invalid(
            "改完就没有管理员了：请先指定另一位管理员".into(),
        ));
    }
    let out = accounts.accounts[idx].clone();
    accounts.save(root)?;
    log(
        root,
        actor.unwrap_or("admin"),
        "update",
        "account",
        id,
        &format!("把 {id} 的角色改为{}", role.label_cn()),
    )?;
    Ok(out)
}

/// 删账户（要管理员；不能删掉最后一个管理员）。
pub fn remove(root: &Path, id: &str, actor: Option<&str>) -> Result<()> {
    require_admin_as(root, actor, "删账户")?;
    let mut accounts = Accounts::load(root)?;
    let before = accounts.accounts.len();
    accounts.accounts.retain(|a| a.id != id);
    if accounts.accounts.len() == before {
        return Err(CercisError::NotFound(format!("没有账户 {id}")));
    }
    if !accounts.accounts.is_empty() && !accounts.accounts.iter().any(|a| a.role == Role::Admin)
    {
        return Err(CercisError::Invalid(
            "删完就没有管理员了：先把另一位设成管理员".into(),
        ));
    }
    accounts.save(root)?;
    log(
        root,
        actor.unwrap_or("admin"),
        "delete",
        "account",
        id,
        "删除账户",
    )?;
    Ok(())
}

/// 改口令（本人知道旧口令即可，不需要管理员）。
pub fn change_password(
    root: &Path,
    id: &str,
    old: &str,
    new: &str,
    params: KdfParams,
) -> Result<()> {
    if !verify(root, id, old, params)? {
        log(root, id, "denied", "account", id, "改口令失败：旧口令不对")?;
        return Err(CercisError::Invalid("旧口令不对".into()));
    }
    if new.trim().is_empty() {
        return Err(CercisError::Invalid("新口令不能为空".into()));
    }
    let mut accounts = Accounts::load(root)?;
    let salt = crate::crypto::random_bytes(crate::crypto::SALT_LEN);
    let hash = crate::crypto::derive_key(new, &salt, params)?;
    let acc = accounts
        .accounts
        .iter_mut()
        .find(|a| a.id == id)
        .ok_or_else(|| CercisError::NotFound(format!("没有账户 {id}")))?;
    acc.salt = hex::encode(&salt);
    acc.hash = hex::encode(&hash[..]);
    accounts.save(root)?;
    log(root, id, "update", "account", id, "改口令（只存哈希）")?;
    Ok(())
}

pub fn list(root: &Path) -> Result<Vec<Account>> {
    Ok(Accounts::load(root)?.accounts)
}

fn require_admin_as(root: &Path, actor: Option<&str>, what: &str) -> Result<()> {
    let accounts = Accounts::load(root)?;
    if !accounts.enabled() {
        return Ok(()); // 开放模式
    }
    let s = current(root)?;
    if let Some(s) = &s {
        if s.role.allows(Role::Admin) {
            return Ok(());
        }
        let _ = log(
            root,
            &s.id,
            "denied",
            "permission",
            "account",
            &format!("越权被拒绝：{} 想{what}", s.role.label_cn()),
        );
        return Err(CercisError::Invalid(format!(
            "权限不足：{what}需要管理员（你是{}）",
            s.role.label_cn()
        )));
    }
    // 没有登录态时允许用 --as 指定（脚本场景），但必须确认那人真是管理员
    if let Some(id) = actor {
        if let Some(a) = accounts.get(id) {
            if a.role.allows(Role::Admin) {
                return Ok(());
            }
        }
        return Err(CercisError::Invalid(format!(
            "权限不足：{what}需要管理员（{id} 不是管理员）"
        )));
    }
    Err(CercisError::Invalid(format!(
        "请先登录管理员再{what}：cercis account login <项目> --id <管理员>"
    )))
}

fn session_path(root: &Path) -> PathBuf {
    crate::access::project_dir(root).join("account.json")
}

fn write_session(root: &Path, s: &Session) -> Result<()> {
    let path = session_path(root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::tx::write_atomic(&path, serde_json::to_string_pretty(s)?.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path)?.permissions();
        perms.set_mode(0o600);
        fs::set_permissions(&path, perms)?;
    }
    Ok(())
}

/// 常数时间比较（避免按字节提前返回）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn log(root: &Path, user: &str, action: &str, kind: &str, id: &str, summary: &str) -> Result<()> {
    let store = crate::store::Store::new(
        root,
        crate::store::Identity {
            user_id: user.to_string(),
            device_id: "local-device".to_string(),
            record_device: false,
        },
    );
    store.append_change(&crate::history::ChangeEntry::new(
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

/// 口令散列（给测试用：确认项目里存的不是明文）。
pub fn hash_preview(root: &Path, id: &str) -> Result<String> {
    let accounts = Accounts::load(root)?;
    Ok(accounts
        .get(id)
        .map(|a| format!("{}:{}", a.salt, &a.hash[..8.min(a.hash.len())]))
        .unwrap_or_default())
}

/// 把口令派生成一个临时值（仅用于测试断言哈希不等于口令本身）。
pub fn derive_for_test(password: &str) -> Zeroizing<Vec<u8>> {
    crate::crypto::derive_key(password, b"0123456789abcdef", KdfParams::light()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Identity;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("cercis-accounts-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn accounts_store_hash_not_password() {
        let dir = tmp("hash");
        crate::Project::create(&dir, "试验", Identity::default()).unwrap();
        add(
            &dir,
            "alice",
            "张老师",
            Role::Admin,
            "alice-pw",
            KdfParams::light(),
            "",
        )
        .unwrap();
        let text = fs::read_to_string(dir.join(ACCOUNTS_REL)).unwrap();
        assert!(!text.contains("alice-pw"), "不能存明文口令：{text}");
        assert!(text.contains("salt") && text.contains("hash"));
        assert!(verify(&dir, "alice", "alice-pw", KdfParams::light()).unwrap());
        assert!(!verify(&dir, "alice", "wrong", KdfParams::light()).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_account_must_be_admin() {
        let dir = tmp("first");
        crate::Project::create(&dir, "试验", Identity::default()).unwrap();
        let err = add(
            &dir,
            "bob",
            "李同学",
            Role::Editor,
            "pw",
            KdfParams::light(),
            "",
        )
        .unwrap_err();
        assert!(err.to_string().contains("admin"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_mode_allows_everything() {
        let dir = tmp("open");
        crate::Project::create(&dir, "试验", Identity::default()).unwrap();
        // 没有账户文件 → 任何权限要求都放行
        require(&dir, Role::Admin).unwrap();
        require_plugin_edit(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn roles_gate_writes_and_admin_ops() {
        let dir = tmp("roles");
        crate::testsupport::share_home();
        crate::testsupport::keychain_off();
        let p = crate::Project::create(&dir, "试验", Identity::default()).unwrap();
        add(
            &dir,
            "alice",
            "张老师",
            Role::Admin,
            "alice-pw",
            KdfParams::light(),
            "",
        )
        .unwrap();
        add(
            &dir,
            "carol",
            "王同学",
            Role::Viewer,
            "carol-pw",
            KdfParams::light(),
            "",
        )
        .unwrap();

        // 没登录：写入被拒
        let mut row = crate::table::Row::new();
        row.insert("record_id".into(), "R-1".into());
        let err = p
            .store
            .append_row_extending("records", &[], row.clone(), "R-1", "试写")
            .unwrap_err();
        assert!(err.to_string().contains("先登录"), "{err}");

        // 查看者：还是不能写
        login(&dir, "carol", "carol-pw", KdfParams::light(), 8).unwrap();
        let err = p
            .store
            .append_row_extending("records", &[], row.clone(), "R-1", "试写")
            .unwrap_err();
        assert!(err.to_string().contains("查看者"), "{err}");

        // 管理员：能写
        login(&dir, "alice", "alice-pw", KdfParams::light(), 8).unwrap();
        p.store
            .append_row_extending("records", &[], row, "R-1", "管理员写入")
            .unwrap();

        // 越权要留痕
        let log_text = fs::read_to_string(dir.join("tables/change_log.csv")).unwrap();
        assert!(log_text.contains("denied"), "越权应有流水：{log_text}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn admin_only_plugin_edit_requires_accounts_and_admin() {
        let dir = tmp("plugin-admin");
        crate::testsupport::share_home();
        crate::testsupport::keychain_off();
        crate::Project::create(&dir, "试验", Identity::default()).unwrap();
        // 打开开关但没账户 → 明确拒绝
        let mut cfg = crate::config::ProjectConfig::from_yaml(
            &fs::read_to_string(dir.join("project.yaml")).unwrap(),
        )
        .unwrap();
        cfg.plugins.admin_only_edit = true;
        crate::tx::write_atomic_str(&dir.join("project.yaml"), &cfg.to_yaml().unwrap()).unwrap();
        let err = require_plugin_edit(&dir).unwrap_err();
        assert!(err.to_string().contains("账户"), "{err}");

        // 建账户 + 编辑者：插件目录改动被拒；管理员放行
        add(
            &dir,
            "alice",
            "张老师",
            Role::Admin,
            "a",
            KdfParams::light(),
            "",
        )
        .unwrap();
        add(
            &dir,
            "bob",
            "李同学",
            Role::Editor,
            "b",
            KdfParams::light(),
            "",
        )
        .unwrap();
        login(&dir, "bob", "b", KdfParams::light(), 8).unwrap();
        assert!(require_plugin_edit(&dir).is_err());
        login(&dir, "alice", "a", KdfParams::light(), 8).unwrap();
        require_plugin_edit(&dir).unwrap();
        let _ = fs::remove_dir_all(&dir);
    }
}
