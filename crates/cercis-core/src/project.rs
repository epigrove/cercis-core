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

- `accounts.yaml`：NAS 多人编辑时启用；只存加盐哈希，不存明文口令
- `zones.yaml`：加密区域定义（区域名、路径规则、授权成员、密钥封装）

> 分享包与开源发布默认**不包含**本目录。
"#;

const PLUGINS_README: &str = r#"# 项目内插件（plugins/）

- 每个插件一个目录，含 `plugin.yaml` 与代码
- **默认不加载**：需用户在 app 内明确信任后才运行（信任绑定内容指纹）
- 插件内容改变后信任自动失效，需要重新确认
- 分享包与开源发布默认**不包含**本目录

> 仅管理员可编辑：可在 `project.yaml` 设置 `plugins.admin_only_edit: true`（NAS 多人场景建议开启）。
"#;
