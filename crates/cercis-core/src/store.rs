//! 存储层：项目数据的统一读写入口（含修正机制与变更流水）。
//!
//! 一切写入都必须经过这里——插件不能绕过内核直接改 CSV（架构硬规则）。
//!
//! ## 数据只增不改
//! 原始观测**永不覆盖**：更正写入 `tables/corrections.csv`（记录旧值、新值、原因、
//! 用户、设备、时间、来源），原始值则永久保存在历史对象库里。

use serde::{Deserialize, Serialize};

use crate::history::{ChangeEntry, ObjectStore};
use crate::table::{Row, Table};
use crate::{CercisError, Result};

/// 身份：谁、在哪台设备上操作。
#[derive(Debug, Clone)]
pub struct Identity {
    pub user_id: String,
    pub device_id: String,
    /// 是否把设备写入流水（个人编辑默认关闭）。
    pub record_device: bool,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            user_id: "local-user".to_string(),
            device_id: "local-device".to_string(),
            record_device: false,
        }
    }
}

impl Identity {
    /// 写入流水时使用的设备值（关闭开关时以 `-` 表示未记录）。
    pub fn device_for_log(&self) -> String {
        if self.record_device {
            self.device_id.clone()
        } else {
            "-".to_string()
        }
    }
}

/// 一条更正记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correction {
    pub correction_id: String,
    pub target_table: String,
    pub target_id: String,
    pub field: String,
    pub old_value: String,
    pub new_value: String,
    pub reason: String,
    pub user_id: String,
    pub device_id: String,
    pub ts: String,
    #[serde(default)]
    pub source_ref: String,
    pub status: String,
}

/// 批量写入的结果（导入等场景用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendOutcome {
    /// 本次写入的行数
    pub rows_written: usize,
    /// 写入后表内容的指纹（与流水的 `hash_after` 一致）
    pub hash_after: String,
    /// 批次号（整批共享，记入流水的 `batch_id`）
    pub batch_id: String,
}

/// 存储层：绑定到某个项目目录。
pub struct Store {
    root: std::path::PathBuf,
    objects: ObjectStore,
    pub identity: Identity,
    /// 当前生效的加密共享成员（`share unlock` 后自动带上；None = 没解锁）。
    access_member: Option<String>,
}

impl Store {
    pub fn new(root: impl AsRef<std::path::Path>, identity: Identity) -> Self {
        let root = root.as_ref().to_path_buf();
        Self {
            objects: ObjectStore::new(root.join("history")),
            root,
            identity,
            access_member: None,
        }
    }

    /// 指定当前生效的成员（加密区域用它判断"你是谁、能读能写哪些区域"）。
    pub fn with_access_member(mut self, member: Option<String>) -> Self {
        self.access_member = member;
        self
    }

    /// 同上，用于已经建好的 `Store`。
    pub fn set_access_member(&mut self, member: Option<String>) {
        self.access_member = member;
    }

    pub fn access_member(&self) -> Option<&str> {
        self.access_member.as_deref()
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// 载入一张表（不存在则按给定列创建）。
    ///
    /// 若表不存在且未提供列定义，返回可读错误（而不是创建一张无列名的表）。
    pub fn load_table(&self, name: &str, columns: &[&str]) -> Result<Table> {
        // 插件上下文：清单里限了表就只允许读那几张（Cordis §6.3 的权限降级）
        if let Some(ctx) = crate::plugin::PluginContext::from_env() {
            ctx.ensure_table(name, &self.root)?;
        }
        let rel = format!("tables/{name}.csv");
        let path = self.root.join(&rel);

        // 加密区域：磁盘上是密文，读取时透明解密（未解锁会给出可操作提示）。
        match crate::access::read_through_zone(&self.root, self.access_member.as_deref(), &rel)? {
            Some(bytes) => {
                let text = String::from_utf8(bytes).map_err(|e| {
                    CercisError::Corrupt(format!("加密表解密后不是 UTF-8：{rel}（{e}）"))
                })?;
                return Table::from_csv_text(&path, &text);
            }
            None => {}
        }

        if !path.exists() && columns.is_empty() {
            return Err(CercisError::NotFound(format!(
                "表不存在: {name}（可先写入一行以创建，或由模板提供列定义）"
            )));
        }
        // 区域内还没落盘的新表：只在内存里建（明文不许落到磁盘上）
        if !path.exists()
            && crate::access::zone_covering(&self.root, &rel)?.is_some()
        {
            let t = Table::in_memory(&path, columns);
            t.validate_structure()?;
            return Ok(t);
        }
        Table::load_or_create(path, columns)
    }

    /// 把整张表写入（含历史存档与流水）。
    ///
    /// - `action`：create / update / restore ……
    /// - `summary`：一句话说明（写入流水）
    pub fn save_table(&self, table: &Table, action: &str, summary: &str) -> Result<String> {
        if let Some(ctx) = crate::plugin::PluginContext::from_env() {
            ctx.ensure_write(&self.root)?;
        }
        crate::accounts::require(&self.root, crate::accounts::Role::Editor)?;
        let before = self.read_stored(table.path())?;
        let hash_before = before
            .as_ref()
            .map(|b| ObjectStore::hash_bytes(b))
            .unwrap_or_default();

        if let Some(prev) = before.as_ref() {
            self.objects.put(prev)?; // 保留旧状态（加密区域内保留的是密文）
        }

        let after_text = table.to_csv_text();
        self.write_table_text(table, &after_text)?;
        let after = self.read_stored(table.path())?.ok_or_else(|| {
            CercisError::Corrupt(format!("写入后读不到表内容：{}", table.path().display()))
        })?;
        let hash_after = self.objects.put(&after)?;

        let table_name = table
            .path()
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        self.append_change(&ChangeEntry::new(
            &self.identity.user_id,
            &self.identity.device_for_log(),
            action,
            &table_name,
            "",
            summary,
            &hash_before,
            &hash_after,
        ))?;

        Ok(hash_after)
    }

    /// 读取"文件当前实际存着的东西"：加密区域里的表就是密文。
    fn read_stored(&self, path: &std::path::Path) -> Result<Option<Vec<u8>>> {
        if let Some(text) = crate::tx::read_optional(path)? {
            return Ok(Some(text.into_bytes()));
        }
        let enc = crate::access::enc_path(path);
        if enc.exists() {
            return Ok(Some(std::fs::read(&enc)?));
        }
        Ok(None)
    }

    /// 写表：区域内的表加密落盘（明文不留），区域外就是普通 CSV。
    fn write_table_text(&self, table: &Table, text: &str) -> Result<()> {
        let rel = self.rel_of(table.path());
        match crate::access::write_through_zone(
            &self.root,
            self.access_member.as_deref(),
            &rel,
            text.as_bytes(),
        )? {
            Some(_blob) => Ok(()),
            None => crate::tx::write_atomic_str(table.path(), text),
        }
    }

    /// 项目内的相对路径（统一用 `/` 分隔）。
    fn rel_of(&self, path: &std::path::Path) -> String {
        path.strip_prefix(&self.root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().to_string())
    }

    /// 追加一行到某张表，并留痕（最常用的写入路径）。
    pub fn append_row(
        &self,
        table_name: &str,
        columns: &[&str],
        row: Row,
        target_id: &str,
        summary: &str,
    ) -> Result<()> {
        self.append_row_action(table_name, columns, row, target_id, summary, "create")
    }

    /// 同 `append_row`，但可指定流水中的动作类型（如 correct / import）。
    pub fn append_row_action(
        &self,
        table_name: &str,
        columns: &[&str],
        row: Row,
        target_id: &str,
        summary: &str,
        action: &str,
    ) -> Result<()> {
        self.append_row_inner(table_name, columns, row, target_id, summary, action, false)
    }

    /// 追加一行，并允许**按这一行的列扩展表结构**（新增列追加到表末尾）。
    ///
    /// 用于"表已经存在、但用户要写一个新列"的场景（S6 发现 F-03：
    /// `append-many` 有 `--allow-new-columns`，`append` 却没有）。
    pub fn append_row_extending(
        &self,
        table_name: &str,
        columns: &[&str],
        row: Row,
        target_id: &str,
        summary: &str,
    ) -> Result<()> {
        self.append_row_inner(table_name, columns, row, target_id, summary, "create", true)
    }

    #[allow(clippy::too_many_arguments)]
    fn append_row_inner(
        &self,
        table_name: &str,
        columns: &[&str],
        row: Row,
        target_id: &str,
        summary: &str,
        action: &str,
        allow_new_columns: bool,
    ) -> Result<()> {
        let mut table = self.load_table(table_name, columns)?;
        // 结构校验：列名必须已存在；显式允许时按这一行扩展出新列
        for k in row.keys() {
            if !table.columns.contains(k) {
                if allow_new_columns {
                    table.columns.push(k.clone());
                    continue;
                }
                return Err(CercisError::Invalid(format!(
                    "表 {table_name} 中不存在列 {k}（请先扩展表结构，或加 --allow-new-columns）"
                )));
            }
        }
        table.push(row);
        self.persist_with_log(&table, action, target_id, summary, "", "")?;
        Ok(())
    }

    /// 批量追加多行（**一次事务、一条流水**）。
    ///
    /// 与逐行追加的区别，正是导入场景需要的两点：
    /// - **一次事务**：先把整批行校验完再落盘，中途失败不会留下"写了一半"的数据；
    /// - **一条流水**：整批共享同一个 `batch_id` 与 `source_ref`，上万行导入
    ///   不会把 `change_log.csv` 冲爆，也便于"这批数据从哪来"的追溯。
    ///
    /// `source_ref` 建议写导入源文件路径；`action` 建议用 `import`。
    #[allow(clippy::too_many_arguments)]
    pub fn append_rows(
        &self,
        table_name: &str,
        columns: &[&str],
        rows: Vec<Row>,
        action: &str,
        summary: &str,
        source_ref: &str,
    ) -> Result<AppendOutcome> {
        if rows.is_empty() {
            return Err(CercisError::Invalid("批量写入的行数为 0".into()));
        }
        let mut table = self.load_table(table_name, columns)?;
        // 结构校验：整批行都必须只用已存在的列（先全查，再落盘）
        for (i, row) in rows.iter().enumerate() {
            for k in row.keys() {
                if !table.columns.contains(k) {
                    return Err(CercisError::Invalid(format!(
                        "第 {} 行含有表 {table_name} 中不存在的列 {k}（请先扩展表结构）",
                        i + 1
                    )));
                }
            }
        }

        let rows_written = rows.len();
        for row in rows {
            table.push(row);
        }
        let batch_id = crate::ids::new_machine_id();
        let hash_after =
            self.persist_with_log(&table, action, "", summary, source_ref, &batch_id)?;
        Ok(AppendOutcome {
            rows_written,
            hash_after,
            batch_id,
        })
    }

    /// 内部：把改好的表落盘，并写一条流水（单行与批量共用）。
    ///
    /// 落盘前把旧内容存入对象库（逐次存档），因此任何一次写入都可回溯。
    fn persist_with_log(
        &self,
        table: &Table,
        action: &str,
        target_id: &str,
        summary: &str,
        source_ref: &str,
        batch_id: &str,
    ) -> Result<String> {
        // 插件上下文：清单里 permissions.write 不为真就一律写不动
        if let Some(ctx) = crate::plugin::PluginContext::from_env() {
            ctx.ensure_write(&self.root)?;
        }
        // 多人项目（有账户）里，写数据至少要是"编辑者"；开放模式一律放行
        crate::accounts::require(&self.root, crate::accounts::Role::Editor)?;
        let table_name = table
            .path()
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let before = self.read_stored(table.path())?;
        let hash_before = before
            .as_ref()
            .map(|b| ObjectStore::hash_bytes(b))
            .unwrap_or_default();
        if let Some(prev) = before.as_ref() {
            self.objects.put(prev)?;
        }
        let after_text = table.to_csv_text();
        self.write_table_text(table, &after_text)?;
        let after = self.read_stored(table.path())?.ok_or_else(|| {
            CercisError::Corrupt(format!("写入后读不到表内容：{}", table.path().display()))
        })?;
        let hash_after = self.objects.put(&after)?;

        let mut entry = ChangeEntry::new(
            &self.identity.user_id,
            &self.identity.device_for_log(),
            action,
            &table_name,
            target_id,
            summary,
            &hash_before,
            &hash_after,
        );
        entry.batch_id = batch_id.to_string();
        entry.source_ref = source_ref.to_string();
        self.append_change(&entry)?;
        Ok(hash_after)
    }

    /// 记录一条更正（不覆盖原值）。
    ///
    /// 参数较多是有意为之：更正必须写全“旧值/新值/原因/来源”，缺一不可。
    #[allow(clippy::too_many_arguments)]
    pub fn correct(
        &self,
        target_table: &str,
        target_id: &str,
        field: &str,
        old_value: &str,
        new_value: &str,
        reason: &str,
        source_ref: &str,
    ) -> Result<String> {
        // 确保更正表存在（首次使用时按标准列创建）
        let _ = self.load_table(
            "corrections",
            &[
                "correction_id",
                "target_table",
                "target_id",
                "field",
                "old_value",
                "new_value",
                "reason",
                "user_id",
                "device_id",
                "ts",
                "source_ref",
                "status",
            ],
        )?;

        let c = Correction {
            correction_id: format!("C-{}", crate::ids::new_machine_id()),
            target_table: target_table.to_string(),
            target_id: target_id.to_string(),
            field: field.to_string(),
            old_value: old_value.to_string(),
            new_value: new_value.to_string(),
            reason: reason.to_string(),
            user_id: self.identity.user_id.clone(),
            device_id: self.identity.device_for_log(),
            ts: crate::ids::now_iso8601(),
            source_ref: source_ref.to_string(),
            status: "生效".to_string(),
        };

        let mut row = Row::new();
        row.insert("correction_id".into(), c.correction_id.clone());
        row.insert("target_table".into(), c.target_table.clone());
        row.insert("target_id".into(), c.target_id.clone());
        row.insert("field".into(), c.field.clone());
        row.insert("old_value".into(), c.old_value.clone());
        row.insert("new_value".into(), c.new_value.clone());
        row.insert("reason".into(), c.reason.clone());
        row.insert("user_id".into(), c.user_id.clone());
        row.insert("device_id".into(), c.device_id.clone());
        row.insert("ts".into(), c.ts.clone());
        row.insert("source_ref".into(), c.source_ref.clone());
        row.insert("status".into(), c.status.clone());

        self.append_row_action(
            "corrections",
            &[
                "correction_id",
                "target_table",
                "target_id",
                "field",
                "old_value",
                "new_value",
                "reason",
                "user_id",
                "device_id",
                "ts",
                "source_ref",
                "status",
            ],
            row,
            &c.correction_id,
            &format!("更正 {target_table}.{target_id}.{field}: {old_value} → {new_value}"),
            "correct",
        )?;
        Ok(c.correction_id)
    }

    /// 追加一条流水到 `tables/change_log.csv`。
    ///
    /// 说明：写流水本身**不再生成流水、也不进历史对象库**（否则会无限递归）。
    /// 流水表的内容变化可通过 `history/checkpoints/` 的检查点回溯。
    pub fn append_change(&self, entry: &ChangeEntry) -> Result<()> {
        let table = self.load_table(
            "change_log",
            &[
                "change_id",
                "ts",
                "user_id",
                "device_id",
                "action",
                "target_table",
                "target_id",
                "summary",
                "batch_id",
                "tool_version",
                "source_ref",
                "hash_before",
                "hash_after",
            ],
        )?;
        let mut row = Row::new();
        row.insert("change_id".into(), entry.change_id.clone());
        row.insert("ts".into(), entry.ts.clone());
        row.insert("user_id".into(), entry.user_id.clone());
        row.insert("device_id".into(), entry.device_id.clone());
        row.insert("action".into(), entry.action.clone());
        row.insert("target_table".into(), entry.target_table.clone());
        row.insert("target_id".into(), entry.target_id.clone());
        row.insert("summary".into(), entry.summary.clone());
        row.insert("batch_id".into(), entry.batch_id.clone());
        row.insert("tool_version".into(), entry.tool_version.clone());
        row.insert("source_ref".into(), entry.source_ref.clone());
        row.insert("hash_before".into(), entry.hash_before.clone());
        row.insert("hash_after".into(), entry.hash_after.clone());
        let mut table = table;
        table.push(row);
        table.save()?;
        Ok(())
    }

    /// 读取变更流水。
    pub fn read_change_log(&self) -> Result<Vec<ChangeEntry>> {
        let table = self.load_table(
            "change_log",
            &[
                "change_id",
                "ts",
                "user_id",
                "device_id",
                "action",
                "target_table",
                "target_id",
                "summary",
                "batch_id",
                "tool_version",
                "source_ref",
                "hash_before",
                "hash_after",
            ],
        )?;
        let mut out = Vec::new();
        for r in &table.rows {
            out.push(ChangeEntry {
                change_id: r.get("change_id").cloned().unwrap_or_default(),
                ts: r.get("ts").cloned().unwrap_or_default(),
                user_id: r.get("user_id").cloned().unwrap_or_default(),
                device_id: r.get("device_id").cloned().unwrap_or_default(),
                action: r.get("action").cloned().unwrap_or_default(),
                target_table: r.get("target_table").cloned().unwrap_or_default(),
                target_id: r.get("target_id").cloned().unwrap_or_default(),
                summary: r.get("summary").cloned().unwrap_or_default(),
                batch_id: r.get("batch_id").cloned().unwrap_or_default(),
                tool_version: r.get("tool_version").cloned().unwrap_or_default(),
                source_ref: r.get("source_ref").cloned().unwrap_or_default(),
                hash_before: r.get("hash_before").cloned().unwrap_or_default(),
                hash_after: r.get("hash_after").cloned().unwrap_or_default(),
            });
        }
        Ok(out)
    }

    /// 还原某个历史对象到指定文件（回滚 = 新增一次变更）。
    pub fn restore_object_to_file(
        &self,
        hash: &str,
        target: impl AsRef<std::path::Path>,
        summary: &str,
    ) -> Result<()> {
        // 回滚也是写入（回滚 = 新增一次变更）→ 至少要是编辑者
        crate::accounts::require(&self.root, crate::accounts::Role::Editor)?;
        let bytes = self.read_object(hash)?;
        let text = String::from_utf8(bytes).map_err(|e| {
            CercisError::Corrupt(format!("历史对象不是合法 UTF-8: {hash}（{e}）"))
        })?;
        crate::tx::write_atomic_str(target.as_ref(), &text)?;
        let name = target
            .as_ref()
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        self.append_change(&ChangeEntry::new(
            &self.identity.user_id,
            &self.identity.device_for_log(),
            "rollback",
            &name,
            "",
            summary,
            "",
            hash,
        ))?;
        Ok(())
    }

    /// 读取历史对象内容（加密区域里存的是密文，这里透明解密）。
    ///
    /// - 老指纹会先按 `access/zone-objects.yaml` 的台账换成密文对象指纹；
    /// - 密文对象用当前会话里的区域钥匙解开（未解锁会给出可操作提示）。
    pub fn read_object(&self, hash: &str) -> Result<Vec<u8>> {
        let real = crate::access::resolve_object_hash(&self.root, hash)?;
        let blob = self.objects.get(&real)?;
        crate::access::decrypt_object(&self.root, self.access_member.as_deref(), &blob)
    }
}
