//! 项目：创建、打开与目录骨架。
//!
//! 目录结构见文档 §6。创建时**不生成任何学科专有表**——表由模板（插件）提供。

use std::path::{Path, PathBuf};

use crate::config::ProjectConfig;
use crate::history::Checkpoints;
use crate::query::Query;
use crate::store::{Identity, Store};
use crate::template::Template;
use crate::tx;
use crate::validate::{validate_project, Report};
use crate::{CercisError, Result};

/// 一个 Cercis 项目。
pub struct Project {
    root: PathBuf,
    pub config: ProjectConfig,
    pub store: Store,
}

/// 标准目录骨架（相对项目根）。
pub const DIRS: &[&str] = &[
    "tables",
    "methods",
    "methods/sop",
    "methods/计算",
    "methods/记录",
    "views",
    "media",
    "import",
    "import/originals",
    "import/scans",
    "templates",
    "exports",
    "history",
    "history/objects",
    "history/checkpoints",
    "access",
    "plugins",
    "_labnote",
    "_labnote/cache",
    "_labnote/thumbs",
    "_labnote/trash",
    "_labnote/runs",
];

impl Project {
    /// 新建项目：创建目录骨架、`project.yaml`、`AGENTS.md`、`README.md`。
    ///
    /// 已存在的目录不会被覆盖（除非为空）；若 `project.yaml` 已存在则报错，
    /// 避免误覆盖已有项目。
    pub fn create(root: impl AsRef<Path>, name: &str, identity: Identity) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        if root.join("project.yaml").exists() {
            return Err(CercisError::Conflict(format!(
                "目录中已存在 project.yaml，不能重复创建项目: {}",
                root.display()
            )));
        }

        for d in DIRS {
            std::fs::create_dir_all(root.join(d))?;
        }

        let mut config = ProjectConfig::new(name);
        config.id = crate::ids::new_machine_id();
        config.identity.record_device = identity.record_device;
        config.identity.display_name = identity.user_id.clone();

        tx::write_atomic_str(&root.join("project.yaml"), &config.to_yaml()?)?;
        tx::write_atomic_str(&root.join("README.md"), &readme_template(name))?;
        tx::write_atomic_str(&root.join("AGENTS.md"), &agents_template(name))?;
        tx::write_atomic_str(
            &root.join("数据字典.csv"),
            "field,label_cn,label_en,type,unit,allowed_values,notes\n",
        )?;
        tx::write_atomic_str(&root.join("数据字典.md"), &dict_md_template(name))?;
        tx::write_atomic_str(&root.join("history/README.md"), HISTORY_README)?;
        tx::write_atomic_str(&root.join("access/README.md"), ACCESS_README)?;
        tx::write_atomic_str(&root.join("plugins/README.md"), PLUGINS_README)?;

        // 装入通用模板（最小可用；用户可换成其他模板）
        let template = Template::generic();
        template.save(root.join("template.yaml"))?;
        let store = Store::new(&root, identity);
        store.ensure_tables_from_template(&template)?;
        store.append_change(&crate::history::ChangeEntry::new(
            &store.identity.user_id,
            &store.identity.device_for_log(),
            "create",
            "project",
            &config.id,
            &format!("创建项目：{name}（已装入通用模板）"),
            "",
            "",
        ))?;

        Ok(Self {
            root,
            config,
            store,
        })
    }

    /// 打开已有项目。
    pub fn open(root: impl AsRef<Path>, identity: Option<Identity>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let text = tx::read_optional(&root.join("project.yaml"))?.ok_or_else(|| {
            CercisError::NotFound(format!(
                "不是 Cercis 项目（缺少 project.yaml）: {}",
                root.display()
            ))
        })?;
        let config = ProjectConfig::from_yaml(&text)?;

        let mut id = identity.unwrap_or_default();
        if id.user_id == "local-user" && !config.identity.display_name.is_empty() {
            id.user_id = config.identity.display_name.clone();
        }
        id.record_device = config.identity.record_device;

        let store = Store::new(&root, id);
        let mut store = store;
        // 加密区域：若这个项目已经解锁过，会话里的成员自动生效（读密文时透明解密）
        store.set_access_member(crate::access::current_member(&root));
        Ok(Self {
            root,
            config,
            store,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 项目体检（S1 版本）：临时文件残留、目录缺失、配置可读性。
    pub fn doctor(&self) -> Result<Vec<String>> {
        let mut issues = Vec::new();
        for d in DIRS {
            if !self.root.join(d).exists() {
                issues.push(format!("缺少目录: {d}"));
            }
        }
        let removed = tx::clean_stale_tmp(&self.root.join("tables"))?;
        for r in removed {
            issues.push(format!("清理残留临时文件: {}", r.display()));
        }
        // 加密区域：明文残留、未解锁提示
        issues.extend(crate::access::doctor_issues(&self.root)?);
        Ok(issues)
    }

    /// 载入项目模板（缺省返回通用模板）。
    pub fn template(&self) -> Result<Template> {
        match self.store.load_template()? {
            Some(t) => Ok(t),
            None => Ok(Template::generic()),
        }
    }

    /// 载入模板，并**把术语包应用到标签上**（数据与列名不受影响）。
    ///
    /// 界面、说明书、AI 材料都该用这一版；要"原样的模板"用 [`Project::template`]。
    pub fn template_with_terms(&self) -> Result<(Template, Option<crate::Terms>)> {
        let mut t = self.template()?;
        let terms = crate::Terms::load(self.root())?;
        if let Some(pack) = &terms {
            pack.apply_to(&mut t);
        }
        Ok((t, terms))
    }

    /// 当前生效的术语包（没装过就是 None）。
    pub fn terms(&self) -> Result<Option<crate::Terms>> {
        crate::Terms::load(self.root())
    }

    /// 安装/替换术语包：写 `<项目>/terms.yaml`，**不动任何数据**。
    pub fn install_terms(&self, terms: &crate::Terms) -> Result<()> {
        crate::accounts::require(self.root(), crate::accounts::Role::Admin)?;
        terms.save(self.root())?;
        self.store.append_change(&crate::history::ChangeEntry::new(
            &self.store.identity.user_id,
            &self.store.identity.device_for_log(),
            "update",
            "terms",
            &terms.profile,
            &format!(
                "切换术语包：{}（覆盖 {} 处标签，数据未改动）",
                terms.profile,
                terms.override_count()
            ),
            "",
            "",
        ))?;
        Ok(())
    }

    /// 装入模板：保存到项目并建表（已有的表与数据不动）。
    ///
    /// 返回新建的表名列表。
    pub fn install_template(&self, t: &Template) -> Result<Vec<String>> {
        crate::accounts::require(self.root(), crate::accounts::Role::Admin)?;
        t.validate()?;
        self.store.save_template(t)?;
        let created = self.store.ensure_tables_from_template(t)?;
        self.store.append_change(&crate::history::ChangeEntry::new(
            &self.store.identity.user_id,
            &self.store.identity.device_for_log(),
            "update",
            "template",
            &t.name,
            &format!("装入模板 {}（新建表: {}）", t.name, created.join(", ")),
            "",
            "",
        ))?;
        Ok(created)
    }

    /// 按模板执行查询。
    pub fn query(&self, table: &str, mut q: Query) -> Result<Vec<crate::table::Row>> {
        let t = self.template()?;
        let table_obj = self.store.load_table_with_template(table, &t)?;
        // 用模板里的类型信息补全查询（数值/日期排序需要）
        if q.types.is_empty() {
            if let Some(td) = t.table(table) {
                let mut m = std::collections::BTreeMap::new();
                for f in &td.fields {
                    m.insert(f.name.clone(), f.field_type.clone());
                }
                q.types = m;
            }
        }
        q.execute(&table_obj)
    }

    /// 按模板做整体校验（引用完整性 + 字段规则）。
    pub fn validate(&self) -> Result<Report> {
        let t = self.template()?;
        validate_project(&self.store, &t)
    }

    /// 按**指定的**模板做整体校验（不改项目文件）。
    ///
    /// 用途：`set-field-type` 这类"先看看改了会怎么样"的场景——拿一份还没落盘的
    /// 新模板去校验现有数据，就能预测出"这一改会新增几处错误、在哪几行"。
    pub fn validate_with(&self, t: &Template) -> Result<Report> {
        validate_project(&self.store, t)
    }

    /// 改模板里的一个字段（**写入**：保存模板并留痕）。
    ///
    /// 改类型不会动任何数据文件——类型只存在于模板里。返回新模板与变更摘要。
    pub fn apply_field_patch(
        &self,
        table: &str,
        field: &str,
        patch: &crate::template::FieldPatch,
        note: &str,
    ) -> Result<(Template, String)> {
        let current = self.template()?;
        let next = current.with_field_patch(table, field, patch)?;
        self.store.save_template(&next)?;
        let describe = patch.describe();
        let extra = if note.trim().is_empty() {
            String::new()
        } else {
            format!("；说明：{}", note.trim())
        };
        self.store.append_change(&crate::history::ChangeEntry::new(
            &self.store.identity.user_id,
            &self.store.identity.device_for_log(),
            "update",
            "template",
            &format!("{table}.{field}"),
            &format!("修改字段 {table}.{field}：{describe}{extra}（数据文件未改动）"),
            "",
            "",
        ))?;
        Ok((next, describe))
    }

    /// 删掉一张**空表**（F-05：换模板后残留的空表要有个干净的清理通道）。
    ///
    /// 规矩：**只删空表**——表里只要有数据就拒绝（数据只增不改，删数据得走更正/回滚）。
    /// 顺带把它从模板里去掉（模板里没有这张表时不动模板）。
    /// 返回（删掉的表名，是否也从模板里去掉）。
    pub fn drop_empty_table(&self, table: &str) -> Result<(String, bool)> {
        let path = self.root.join("tables").join(format!("{table}.csv"));
        if !path.exists() {
            return Err(CercisError::NotFound(format!("表不存在：{table}")));
        }
        let t = crate::table::Table::load(&path)?;
        if !t.rows.is_empty() {
            return Err(CercisError::Invalid(format!(
                "表 {table} 里已有 {} 行数据——只删空表；要处理数据请用更正或检查点回滚",
                t.rows.len()
            )));
        }
        std::fs::remove_file(&path)?;
        // 模板里若声明了这张表，一并去掉（否则体检会报"模板要求的列在表中缺失"）
        let mut removed_from_template = false;
        if let Ok(Some(tpl)) = self.store.load_template() {
            if tpl.table(table).is_some() {
                let mut next = tpl.clone();
                next.tables.retain(|x| x.name != table);
                if next.validate().is_ok() {
                    self.store.save_template(&next)?;
                    removed_from_template = true;
                }
            }
        }
        self.store.append_change(&crate::history::ChangeEntry::new(
            &self.store.identity.user_id,
            &self.store.identity.device_for_log(),
            "drop",
            table,
            table,
            &format!(
                "删掉空表 {table}{}（表里没有数据，符合「只删空表」的规矩）",
                if removed_from_template {
                    "，并已从模板中移除"
                } else {
                    ""
                }
            ),
            "",
            "",
        ))?;
        Ok((table.to_string(), removed_from_template))
    }

    /// 创建检查点（文件级备份）。
    pub fn checkpoint(&self, note: &str) -> Result<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(self.root.join("tables"))? {
            let e = entry?;
            if e.file_type()?.is_file() {
                files.push(e.path());
            }
        }
        let cps = Checkpoints::new(
            self.root.join("history"),
            self.config.history.checkpoints_keep,
        );
        cps.create(&self.root, &files, note)
    }
}

fn readme_template(name: &str) -> String {
    format!(
        r#"# {name}

由 **Cercis** 创建的项目（文件夹即数据库）。

- 结构化数据在 `tables/`（CSV，任何软件可读）
- 方法与文档在 `methods/`
- 历史与还原在 `history/`（逐次存档 + 检查点）
- AI 助手请先读 `AGENTS.md`

> 愿瑟希斯捍卫你的思想。 / *May Cerces safeguard your thoughts.*
"#
    )
}

fn agents_template(name: &str) -> String {
    format!(
        r#"# 项目：{name}（AGENTS 使用说明）

## 数据在哪
- 全部结构化数据在 `tables/*.csv`；表结构见 `数据字典.md`
- 历史改动在 `tables/change_log.csv`；更正在 `tables/corrections.csv`

## 规则（必须遵守）
- 只读默认：未经用户确认不得写入
- 绝不直接编辑 `tables/*.csv`；写入必须通过 `cercis` 命令（内核通道）
- 更正不覆盖原值：使用 corrections；原值由 `history/objects/` 永久保留
- 统计与绘图交给 R/Python；本工具只做数据整理与导出

## 禁止事项
- 不访问项目之外的私人文件夹
- 不把任何真实数据写入分享包或发布物
"#
    )
}

fn dict_md_template(name: &str) -> String {
    format!(
        r#"# 数据字典：{name}

| 字段 | 中文名 | 英文名 | 类型 | 单位 | 允许值 | 说明 |
|------|--------|--------|------|------|--------|------|
| （由模板/插件写入） | | | | | | |

> 字段的语义由模板（插件）提供；内核只负责结构与格式规则。
"#
    )
}

const HISTORY_README: &str = r#"# 历史（history/）

- `objects/`：逐次改动存档（按内容指纹去重，任一时刻的文件内容都在这里）
- `checkpoints/`：检查点（文件级备份；默认保留最近 50 个）

还原任意一次改动 = 把对应指纹的内容写回文件（回滚本身也记一条流水，不销毁现状）。
"#;

const ACCESS_README: &str = r#"# 身份与鉴权（access/）

- `members.yaml`：加密共享的成员名单（**只有公钥**，没有口令、没有私钥明文）
- `members/<成员>.key`：成员私钥文件——用他自己的口令（Argon2id）加密过
- `zones.yaml`：加密区域定义（区域名、路径规则、成员授权、被封装的内容密钥）
- `keyrings/<区域>.yaml`：历史密钥世代（换密钥后留档，用于解旧密文与回滚）
- `accounts.yaml`：NAS 多人编辑时启用；只存加盐哈希，不存明文口令

## 加密区域怎么用（命令行）

```bash
cercis share member-add <项目> --id alice --name 张老师      # 建成员（会让你输入口令）
cercis share zone-create <项目> --name patent-data --paths 'methods/配方/**' --member alice:admin
cercis share unlock <项目> --member alice                   # 解锁（口令可交给系统钥匙串记住）
cercis share status <项目>                                  # 看有哪些区域、谁在里面、有没有明文残留
cercis share export <项目> --zone patent-data               # 解密导出到 exports/ 给 R / AI 用
cercis share lock <项目>                                    # 上锁（清掉本机会话）
```

要点：

- **默认不加密**：只有被划进加密区域的文件在磁盘上才是密文（`.enc`），其余保持 CSV 开放。
- 区域名、路径规则与**文件名**在磁盘上可见，**内容**不可见；app 内对未授权成员不显示这些条目。
- 忘记口令**无法找回**：只能由管理员重新授权一个新的成员身份。
- 移除成员会自动**换密钥并重加密**；被移除者从此刻起读不到新内容（他之前看过的无法追回）。

> 分享包与开源发布默认**不包含**本目录。
"#;

const PLUGINS_README: &str = r#"# 项目内插件（plugins/）

- 每个插件一个目录，含 `plugin.yaml` 与代码
- **默认不加载**：需用户在 app 内明确信任后才运行（信任绑定内容指纹）
- 插件内容改变后信任自动失效，需要重新确认
- 分享包与开源发布默认**不包含**本目录

> 仅管理员可编辑：可在 `project.yaml` 设置 `plugins.admin_only_edit: true`（NAS 多人场景建议开启）。
"#;
