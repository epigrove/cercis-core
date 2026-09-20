//! AI 友好：生成面向 AI 助手的项目说明与机器可读契约。
//!
//! ## 生成物
//! - `AGENTS.md`：AI 助手的项目说明书（人也能读）
//! - `CLAUDE.md`：其他 AI 工具的入口（指向 AGENTS.md，不重复维护）
//! - `_labnote/ai_contract.json`：机器可读契约（表/列/类型/约束/常用命令）
//!
//! ## 原则
//! 1. **只读默认**：AI 不得未经确认写入
//! 2. **写入必走内核通道**：直接编辑 CSV 会导致校验与留痕失效
//! 3. **更正不覆盖**：原值永久保留
//! 4. **统计与绘图交给 R/Python**：本工具只做整理与导出

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::manifest::Manifest;
use crate::template::Template;
use crate::tx;
use crate::{CercisError, Result};

/// 机器可读的 AI 契约。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiContract {
    pub schema: String,
    pub project_name: String,
    pub core_version: String,
    pub plugin_api_version: u32,
    /// 表与列的机器可读描述
    pub tables: Vec<ContractTable>,
    /// 可用命令（供 AI 直接调用）
    pub commands: Vec<ContractCommand>,
    /// 硬性规则
    pub rules: Vec<String>,
    /// 禁止事项
    pub forbidden: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractTable {
    pub name: String,
    pub primary_key: String,
    pub columns: Vec<ContractColumn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractColumn {
    pub name: String,
    pub label_cn: String,
    pub field_type: String,
    pub unit: String,
    pub required: bool,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub allowed_values: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractCommand {
    pub name: String,
    pub purpose: String,
    pub example: String,
}

impl AiContract {
    /// 由模板与清单生成契约。
    pub fn build(template: &Template, manifest: &Manifest) -> Self {
        let tables = template
            .tables
            .iter()
            .map(|t| ContractTable {
                name: t.name.clone(),
                primary_key: t.primary_key.clone(),
                columns: t
                    .fields
                    .iter()
                    .map(|f| ContractColumn {
                        name: f.name.clone(),
                        label_cn: f.label_cn.clone(),
                        field_type: f.field_type.clone(),
                        unit: f.unit.clone(),
                        required: f.required,
                        reference: f.reference.clone().unwrap_or_default(),
                        allowed_values: f.allowed_values.clone(),
                    })
                    .collect(),
            })
            .collect();

        let commands = vec![
            cmd("cercis template <项目>", "查看表结构与字段定义", "cercis template ./项目 --table observations"),
            cmd("cercis query <项目> --table <表> --filter <列> <运算符> <值>", "组合筛选查询", "cercis query ./项目 --table observations --filter trait_id eq length_mm --sort value_num:desc --json"),
            cmd("cercis validate <项目>", "校验引用完整性与字段规则", "cercis validate ./项目 --json"),
            cmd("cercis log <项目>", "查看变更流水（谁/设备/时间/动作）", "cercis log ./项目 --limit 20 --json"),
            cmd("cercis checkpoint <项目>", "创建检查点（文件级备份）", "cercis checkpoint ./项目 --note '分析前快照'"),
            cmd("cercis manifest <项目>", "生成/刷新机器清单", "cercis manifest ./项目"),
            cmd("cercis append <项目> --table <表> --set 列=值", "写入一行（需用户确认）", "cercis append ./项目 --table records --set record_id=R-0001 --set value_num=12.5"),
            cmd("cercis correct <项目> --table <表> --target <编号> --field <列> --old <旧值> --new <新值>", "记录更正（不覆盖原值）", "cercis correct ./项目 --table records --target R-0001 --field value_num --old 12.5 --new 12.7 --reason '笔误'"),
        ];

        Self {
            schema: "cercis.ai_contract/1".into(),
            project_name: manifest.project_name.clone(),
            core_version: crate::CORE_VERSION.to_string(),
            plugin_api_version: crate::plugin::PLUGIN_API_VERSION,
            tables,
            commands,
            rules: vec![
                "只读默认：未经用户确认不得写入".into(),
                "写入必须通过 cercis 命令（内核通道），不得直接编辑 CSV".into(),
                "更正不覆盖原值：原值由 history/objects/ 永久保留".into(),
                "缺失值留空单元格，不要写 NA / - / 0 代替".into(),
                "统计、建模与绘图交给 R/Python/SPSS；本工具只做数据整理与导出".into(),
                "涉及真实数据的操作前，先确认用户意图".into(),
            ],
            forbidden: vec![
                "不访问项目之外的私人文件夹".into(),
                "不把任何真实数据写入分享包或发布物".into(),
                "不绕过内核直接修改 tables/*.csv".into(),
                "不删除 history/ 与 corrections（历史与更正必须可追溯）".into(),
            ],
        }
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// 写入 `_labnote/ai_contract.json`。
    pub fn write(&self, root: impl AsRef<Path>) -> Result<()> {
        let p = root.as_ref().join("_labnote").join("ai_contract.json");
        tx::write_atomic_str(&p, &self.to_json()?)
    }
}

fn cmd(name: &str, purpose: &str, example: &str) -> ContractCommand {
    ContractCommand {
        name: name.into(),
        purpose: purpose.into(),
        example: example.into(),
    }
}

/// 生成 AGENTS.md 内容。
pub fn agents_md(project_name: &str, template: &Template, manifest: &Manifest) -> String {
    let mut s = String::new();
    s.push_str(&format!("# 项目：{project_name}（AGENTS 使用说明）\n\n"));
    s.push_str("> 本文件由 Cercis 内核自动生成（可用 `cercis agents <项目>` 刷新）。\n");
    s.push_str("> 修改前请先读「规则」一节；自定义内容建议写在 `AGENTS.local.md`。\n\n");

    s.push_str("## 数据在哪\n\n");
    s.push_str("- 结构化数据：`tables/*.csv`（UTF-8、无 BOM、LF 换行）\n");
    s.push_str("- 机器清单：`_labnote/manifest.json`（先读它，无需扫描全表）\n");
    s.push_str("- 机器契约：`_labnote/ai_contract.json`（表/列/类型/约束/命令）\n");
    s.push_str("- 字段含义：`数据字典.md`；不要凭猜测解释字段\n");
    s.push_str("- 变更历史：`tables/change_log.csv`；更正记录：`tables/corrections.csv`\n");
    s.push_str("- 历史还原：`history/objects/`（内容寻址）、`history/checkpoints/`（检查点）\n\n");

    s.push_str("## 项目结构\n\n");
    s.push_str(&format!(
        "- 模板：`{}`；术语包：`{}`\n",
        manifest.template, manifest.terms_profile
    ));
    for t in &template.tables {
        s.push_str(&format!(
            "- 表 `{}`（{}）：{} 列，{} 行；主键 `{}`\n",
            t.name,
            if t.label_cn.is_empty() {
                "-"
            } else {
                &t.label_cn
            },
            t.fields.len(),
            manifest
                .tables
                .iter()
                .find(|m| m.name == t.name)
                .map(|m| m.rows)
                .unwrap_or(0),
            t.primary_key
        ));
    }
    s.push('\n');

    s.push_str("## 规则（必须遵守）\n\n");
    for r in AiContract::build(template, manifest).rules {
        s.push_str(&format!("- {r}\n"));
    }
    s.push('\n');

    s.push_str("## 常用命令\n\n");
    s.push_str("```bash\n");
    s.push_str("# 查看结构\ncercis template <项目路径>\n");
    s.push_str("# 查询（组合筛选 + 排序；加 --json 得到机器可读输出）\n");
    s.push_str(
        "cercis query <项目路径> --table <表> --filter <列> eq <值> --sort <列>:asc --json\n",
    );
    s.push_str("# 校验\ncercis validate <项目路径>\n");
    s.push_str("# 流水\ncercis log <项目路径> --limit 20\n");
    s.push_str("```\n\n");

    s.push_str("## 禁止事项\n\n");
    for f in AiContract::build(template, manifest).forbidden {
        s.push_str(&format!("- {f}\n"));
    }
    s.push_str("\n---\n\n愿瑟希斯捍卫你的思想。 / *May Cerces safeguard your thoughts.*\n");
    s
}

/// 生成 CLAUDE.md（入口副本，指向 AGENTS.md）。
pub fn claude_md() -> String {
    "# CLAUDE.md（自动生成）\n\n\
     > 本文件由 Cercis 内核自动生成；**请修改 AGENTS.md，不要修改本文件**。\n\n\
     请阅读同目录下的 `AGENTS.md`，其中包含本项目的数据位置、结构、规则与常用命令。\n\n\
     要点提醒：\n\
     - 只读默认；写入必须通过 `cercis` 命令\n\
     - 先读 `_labnote/manifest.json` 与 `_labnote/ai_contract.json`\n\
     - 绝不直接编辑 `tables/*.csv`\n"
        .to_string()
}

/// 一次生成全套装材料（AGENTS.md、CLAUDE.md、契约、清单）。
pub fn refresh_all(root: impl AsRef<Path>) -> Result<Manifest> {
    let root = root.as_ref();
    if !root.join("project.yaml").exists() {
        return Err(CercisError::NotFound(format!(
            "不是 Cercis 项目（缺少 project.yaml）: {}",
            root.display()
        )));
    }
    let manifest = Manifest::generate(root)?;
    manifest.write(root)?;

    let template = match tx::read_optional(&root.join("template.yaml"))? {
        Some(t) => Template::from_yaml(&t)?,
        None => Template::generic(),
    };

    let contract = AiContract::build(&template, &manifest);
    contract.write(root)?;

    tx::write_atomic_str(
        &root.join("AGENTS.md"),
        &agents_md(&manifest.project_name, &template, &manifest),
    )?;
    tx::write_atomic_str(&root.join("CLAUDE.md"), &claude_md())?;

    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Identity;

    #[test]
    fn generates_agents_and_contract() {
        let dir = std::env::temp_dir().join("cercis-ai-test");
        let _ = std::fs::remove_dir_all(&dir);
        let p = crate::Project::create(&dir, "示例项目", Identity::default()).unwrap();
        let m = refresh_all(p.root()).unwrap();
        assert_eq!(m.project_name, "示例项目");

        let agents = std::fs::read_to_string(dir.join("AGENTS.md")).unwrap();
        assert!(agents.contains("只读默认"));
        assert!(agents.contains("cercis query"));

        let contract = std::fs::read_to_string(dir.join("_labnote/ai_contract.json")).unwrap();
        assert!(contract.contains("cercis.ai_contract/1"));

        assert!(dir.join("CLAUDE.md").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
