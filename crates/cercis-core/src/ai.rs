//! AI 友好：生成面向 AI 助手的项目说明与机器可读契约。
//!
//! ## 生成物
//! - `AGENTS.md`：AI 助手的项目说明书（人也能读）
//! - `CLAUDE.md`：其他 AI 工具的入口（指向 AGENTS.md，不重复维护）
//! - `_labnote/ai_contract.json`：机器可读契约（表/列/类型/约束/常用命令）
//! - `_labnote/skill/cercis/`：**Agent 技能包**（装到 Codex / Claude Code 的技能目录用）
//!
//! 前两样是**随项目走**的：AI 打开项目才读到。技能包是**装在 AI 宿主上**的：
//! 它在 AI 碰项目之前就告诉它"这是什么项目、该怎么做、有哪些红线"。
//! 三者的内容都从同一份契约渲染，避免出现互相矛盾的三套说法。
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
    /// 正则模式（text 专用；整格匹配）
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pattern: String,
    /// 小数位数（number 专用；超出只提醒）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precision: Option<u8>,
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
                        pattern: f.pattern.clone(),
                        precision: f.precision,
                    })
                    .collect(),
            })
            .collect();

        let commands = vec![
            cmd("cercis value-types", "查看七种值类型与六种约束（唯一权威；插件与 AI 照用，不得自造类型）", "cercis value-types --json"),
            cmd("cercis infer-types <项目>", "看某列该用什么类型（候选 + 置信度 + 反例；≥95% 才给强建议）", "cercis infer-types ./项目 --table records --json"),
            cmd("cercis set-field-type <项目> --table <表> --field <列> --type <类型>", "改模板里的列类型/约束（写入：先试运行，会拦下会新增错误的改动）", "cercis set-field-type ./项目 --table records --field value_num --type number --precision 2 --dry-run"),
            cmd("cercis template <项目>", "查看表结构与字段定义", "cercis template ./项目 --table observations"),
            cmd("cercis query <项目> --table <表> --filter <列> <运算符> <值>", "组合筛选查询", "cercis query ./项目 --table observations --filter trait_id eq length_mm --sort value_num:desc --json"),
            cmd("cercis validate <项目>", "校验引用完整性与字段规则", "cercis validate ./项目 --json"),
            cmd("cercis log <项目>", "查看变更流水（谁/设备/时间/动作）", "cercis log ./项目 --limit 20 --json"),
            cmd("cercis checkpoint <项目>", "创建检查点（文件级备份）", "cercis checkpoint ./项目 --note '分析前快照'"),
            cmd("cercis manifest <项目>", "生成/刷新机器清单", "cercis manifest ./项目"),
            cmd("cercis append <项目> --table <表> --set 列=值", "写入一行（需用户确认）", "cercis append ./项目 --table records --set record_id=R-0001 --set value_num=12.5"),
            cmd("cercis append-many <项目> --table <表> --from <文件>", "批量写入/导入（先加 --dry-run 试运行）", "cercis append-many ./项目 --table records --from 数据.csv --dry-run --json"),
            cmd("cercis correct <项目> --table <表> --target <编号> --field <列> --old <旧值> --new <新值>", "记录更正（不覆盖原值）", "cercis correct ./项目 --table records --target R-0001 --field value_num --old 12.5 --new 12.7 --reason '笔误'"),
            cmd("cercis note <项目> --action <动作> --summary <说明>", "记一条流水（导出等动作留痕）", "cercis note ./项目 --action export --summary '导出 observations 到 /tmp/out.csv' --source-ref /tmp/out.csv"),
            cmd("cercis plugins <项目>", "看插件：是否生效、为什么没生效、有没有键冲突、有没有撤不回去的登记", "cercis plugins ./项目 --json"),
            cmd("cercis drop-table <项目> --table <表>", "删掉一张空表（换模板后残留的空表清理；有数据的表会被拒绝）", "cercis drop-table ./项目 --table records --dry-run"),
            cmd("cercis plugins <项目> register --plugin <ID> --kind <KIND> --target <目标>", "插件登记自己对项目结构的增改（停用时按倒序回滚）", "cercis plugins ./项目 register --plugin demo_io --kind column --target observations.spad_note"),
            cmd("cercis plugins <项目> disable --id <ID>", "停用插件并回滚它的登记（先加 --dry-run 看会撤什么）", "cercis plugins ./项目 disable --id demo_io --dry-run"),
            cmd("cercis plugins <项目> block --id <ID>", "屏蔽插件（比停用更彻底：不加载、界面不出现、命令不执行；随时 unblock）", "cercis plugins ./项目 block --id demo_io"),
            cmd("cercis market restore --id <ID> [--from <已下载目录>]", "找回被删掉的插件（清墓碑；给目录就顺手装回来）", "cercis market restore --id research_io --from ~/Downloads/cercis-kits-research"),
            cmd("cercis market list", "看默认预置市场（指向公开仓库）与已从市场安装的插件", "cercis market list --json"),
            cmd("cercis market validate --file <市场清单>", "校验一份 marketplace.json（下载由 app 负责，内核不联网）", "cercis market validate --file ./marketplace.json"),
            cmd("cercis market install --from <已下载目录> --plugin <ID>", "从**已经下载好的**仓库目录安装插件（断网可用；记来源、版本与指纹）", "cercis market install --from ~/Downloads/cercis-kits-research --plugin research_io --json"),
            cmd("cercis market uninstall --id <ID>", "删除市场装的插件（删副本 + 记墓碑，升级不会偷偷装回来；随包的基础插件**不可删除**，只能停用/屏蔽）", "cercis market uninstall --id research_io --dry-run"),
            cmd("cercis share list <项目>", "看加密区域：谁在里面、自己有没有权限（未授权成员看不到该区域）", "cercis share list ./项目 --json"),
            cmd("cercis share status <项目>", "看加密状态：有几个区域、每个多少文件、有没有还没加密的明文", "cercis share status ./项目"),
            cmd("cercis share unlock <项目> --member <成员>", "解锁加密区域（**口令必须由用户本人输入**；AI 不得代填、索取或记录口令）", "cercis share unlock ./项目 --member alice --ttl 8"),
            cmd("cercis share export <项目> --zone <区域>", "解密导出到 exports/ 给 R / AI 用（留痕；用完记得清理明文副本）", "cercis share export ./项目 --zone patent-data --dry-run"),
            cmd("cercis share lock <项目>", "上锁：清掉本机会话", "cercis share lock ./项目"),
            cmd("cercis share clean <项目> [--zone <区域>]", "清理解密导出的明文副本（只动 exports/，用完就该删；留一条流水）", "cercis share clean ./项目 --dry-run --json"),
            cmd("cercis pack <项目> --out <目录>", "打包带走（可增量、可分卷；加密区域保持密文；默认每卷 20 MB 适配微信）", "cercis pack ./项目 --out /tmp/我的包 --part-size-mb 20 --json"),
            cmd("cercis merge <包目录> --into <项目>", "合并一个包：先校验指纹，再**并集**合并（两边的记录都留）；两边都改过的文件保留双方副本；本地文件永不被覆盖", "cercis merge /tmp/我的包 --into ./项目 --dry-run --json"),
            cmd("cercis guard <目录>", "分享守卫（红线：发布零数据）：逐文件判定能不能发出去，并列出隐私线索（邮箱/手机号/身份证/经纬度/自定义关键词）", "cercis guard . --keyword 未公开配方 --json"),
            cmd("cercis sync <项目>", "共存体检：项目在云盘/Git 目录里有没有分叉风险，有没有已经分叉的迹象（云盘冲突副本、残留临时文件、iCloud 占位符）", "cercis sync ./项目 --json"),
            cmd("cercis diff <项目> --table <表> --id <编号>", "看一条记录改了什么（字段级差异 + 更正记录）", "cercis diff ./项目 --table records --id R-1 --json"),
            cmd("cercis diff <项目> --from <老指纹> --to <新指纹>", "比两份历史快照（多了/少了哪些行）", "cercis diff ./项目 --from abc123 --to def456 --only-changes"),
            cmd("cercis remote add <项目> --name nas --path <挂载路径>", "登记 NAS/网络位置（填挂载后的本地路径）；口令用 remote credential set 存进系统钥匙串，**不要写进项目**", "cercis remote add ./项目 --name nas --path /Volumes/lab-nas --kind smb"),
            cmd("cercis remote test <项目> --name nas", "网络目录体检（FR-N06）：可达、可写、能否原子改名、大小写、中文名、速度", "cercis remote test ./项目 --name nas --json"),
            cmd("cercis remote claim <项目> --who <人@设备>", "取写租约：告诉别的设备「我在写」（写之前取，写完 release）", "cercis remote claim ./项目 --who alice@laptop --minutes 30"),
            cmd("cercis workcopy create <主副本> --to <本地目录>", "建工作副本（推荐通道）：本地离线记录，联网后双向合并", "cercis workcopy create /Volumes/lab-nas/试验 --to ~/试验-工作副本 --name nas"),
            cmd("cercis workcopy status <本地副本>", "同步状态（FR-N07）：本地/主副本各有多少新变更、上次同步时间、待处理冲突", "cercis workcopy status ~/试验-工作副本 --json"),
            cmd("cercis workcopy sync <本地副本>", "双向同步（拉回来 + 推上去，都是并集；断网会明确报离线）", "cercis workcopy sync ~/试验-工作副本 --dry-run --json"),
            cmd("cercis account add <项目> --id alice --role admin", "启用多人账户（第一个必须是管理员）：只存加盐哈希，没有账户文件就是开放模式", "cercis account add ./项目 --id alice --name 张老师 --role admin --password-env PW"),
            cmd("cercis account login <项目> --id alice", "登录（登录态存本机用户目录，不进项目）；有账户后：查看者只读、编辑者能写数据、管理员才能改结构与插件", "cercis account login ./项目 --id alice --password-env PW"),
            cmd("cercis account list/role/remove", "账户与角色管理（改角色/删账户要管理员；越权会被拒绝并写 denied 流水）", "cercis account list ./项目 --json"),
            cmd("cercis agents <项目>", "刷新 AI 材料与 Agent 技能包", "cercis agents ./项目"),
            cmd("cercis skill <项目>", "生成 Agent 技能包（--install codex|claude 可直接装）", "cercis skill ./项目 --install codex"),
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
                "类型由内核统一定义（七种值域 + 六种约束）：照 `cercis value-types --json` 用，不要自造类型".into(),
                "改类型只改模板、不动数据；改动前先 `cercis infer-types` 看建议与反例，再 `set-field-type --dry-run` 看影响".into(),
                "插件对项目结构的增改必须经内核登记（`install-template --plugin …` 或 `plugins register`）：停用插件时按倒序回滚这些登记".into(),
                "**数据永不回滚**：表里已有数据、列里已有值，回滚就停下来交给人——要纠正数据用 `correct`（更正即补偿）".into(),
                "插件的取舍：基础插件（随 app 交付）**不删除**，只能 `disable`（停用）或 `block`（屏蔽）；市场装的插件可以 `market uninstall`（留墓碑、可 `market restore` 找回）".into(),
                "加密区域（access/zones.yaml）里的数据：磁盘上是密文，**必须先由用户本人 unlock** 才能读；AI 不得代填/索取/记录/转述用户口令，也不得把解密内容写进明文位置（要明文就走 `share export` 并告知用户）".into(),
                "统计、建模与绘图交给 R/Python/SPSS；本工具只做数据整理与导出".into(),
                "涉及真实数据的操作前，先确认用户意图".into(),
            ],
            forbidden: vec![
                "访问项目之外的私人文件夹".into(),
                "把任何真实数据写入分享包或发布物".into(),
                "绕过内核直接修改 tables/*.csv".into(),
                "删除 history/ 与 corrections（历史与更正必须可追溯）".into(),
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

/// Agent 技能包：给 Codex / Claude Code 这类 AI 宿主用的操作手册。
///
/// ## 为什么由内核生成
/// 技能包里的命令速查必须与 [`AiContract`] **同源**。若各自手写，
/// `AGENTS.md`、`ai_contract.json`、技能包三份迟早互相矛盾——而 AI 恰恰
/// 会同时读到它们，矛盾就等于没有规则。
///
/// ## 技能包的定位
/// - **不是插件**：它不跑在 Cercis 里，而是跑在用户的 AI 宿主里；
///   插件管"在 app 里能点什么"，技能包管"AI 知道该怎么干"。
/// - **不含项目数据**：只有操作方法与命令，因此可以随分享包、发布物一起给同行。
/// - **项目细节不写死**：具体表、字段、规则永远以项目内的
///   `AGENTS.md` 与 `ai_contract.json` 为准，技能包只讲"怎么读、怎么做"。
#[derive(Debug, Clone)]
pub struct AgentSkill {
    /// 技能名（同时是安装目录名）
    pub name: String,
    /// 一句话说明（frontmatter 的 description，AI 按它决定何时加载本技能）
    pub description: String,
    /// `SKILL.md`：什么时候用、红线、操作配方
    pub skill_md: String,
    /// `reference.md`：完整命令速查（与契约同源）
    pub reference_md: String,
}

/// 生成技能包时，前端写法（Codex 与 Claude Code 都用 `name` + `description`）。
const SKILL_NAME: &str = "cercis";

/// 技能的一句话说明：AI 宿主靠它判断"什么时候该加载这个技能"。
fn skill_description() -> String {
    "Use when working inside a Cercis project folder (one containing project.yaml and \
tables/*.csv), or when the user asks to query, record, correct, import, export, or health-check \
Cercis data. Cercis 项目的取数、录入、更正、导入导出与体检操作技能。"
        .to_string()
}

impl AgentSkill {
    /// 由契约生成技能包（契约是唯一事实来源）。
    pub fn build(contract: &AiContract) -> Self {
        Self {
            name: SKILL_NAME.to_string(),
            description: skill_description(),
            skill_md: skill_md(contract),
            reference_md: reference_md(contract),
        }
    }

    /// 写入某个目录下的 `<name>/` 子目录（SKILL.md + reference.md）。
    pub fn write(&self, dir: impl AsRef<Path>) -> Result<std::path::PathBuf> {
        let target = dir.as_ref().join(&self.name);
        tx::write_atomic_str(&target.join("SKILL.md"), &self.skill_md)?;
        tx::write_atomic_str(&target.join("reference.md"), &self.reference_md)?;
        Ok(target)
    }

    /// 项目内的默认位置：`_labnote/skill/`。
    pub fn write_into_project(&self, project_root: impl AsRef<Path>) -> Result<std::path::PathBuf> {
        let dir = project_root.as_ref().join("_labnote").join("skill");
        self.write(dir)
    }
}

/// 生成 `SKILL.md`：什么时候用、开工前读什么、红线、配方。
fn skill_md(contract: &AiContract) -> String {
    let version = crate::CORE_VERSION;
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("name: {}\n", SKILL_NAME));
    s.push_str(&format!("description: {}\n", skill_description()));
    s.push_str("---\n\n");
    s.push_str(&format!(
        "# Cercis 项目操作技能（内核 v{version} 生成）\n\n"
    ));
    s.push_str("> 本文件由 Cercis 内核生成，**请勿手工编辑**；改模板后重新运行 `cercis agents` 即可。\n\n");

    s.push_str("## 什么时候用这个技能\n\n");
    s.push_str("- 用户让你在一个 Cercis 项目里查数据、录数据、更正、导入导出或体检；\n");
    s.push_str("- 目录里能看到 `project.yaml`、`tables/*.csv` 或 `_labnote/ai_contract.json`；\n");
    s.push_str("- 用户提到 Cercis、`cercis` 命令、AGENTS.md、机器清单或数据契约。\n\n");

    s.push_str("## 开工前先读三份文件（顺序固定）\n\n");
    s.push_str("| 顺序 | 文件 | 读它做什么 |\n|---|---|---|\n");
    s.push_str("| 1 | `AGENTS.md` | 项目说明书：背景、数据位置、规则、禁止事项 |\n");
    s.push_str("| 2 | `_labnote/manifest.json` | 机器索引：有哪些表、多少行、何时更新 |\n");
    s.push_str("| 3 | `_labnote/ai_contract.json` | 机器契约：表/列/类型/约束/可用命令 |\n\n");
    s.push_str("项目里的这三份文件永远比本技能更权威：本技能只讲**怎么做**，它们讲**这个项目是什么**。\n\n");

    s.push_str("## 环境\n\n");
    s.push_str("```bash\n# 找到内核命令（按顺序尝试）\n$CERCIS_BIN            # 环境变量\ncommand -v cercis      # PATH\ndirname \"$APP\" | xargs -I{} sh -c 'ls {}/cercis'   # 与 app 同级\n```\n\n");
    s.push_str("所有命令都支持 `--json`（带 schema 版本）；AI 应优先用 `--json` 解析结果，而不是猜文本格式。\n\n");

    s.push_str("## 红线（违反则数据不可信）\n\n");
    for rule in &contract.rules {
        s.push_str(&format!("- {rule}\n"));
    }
    for forbidden in &contract.forbidden {
        s.push_str(&format!("- **禁止**：{forbidden}\n"));
    }
    s.push('\n');

    s.push_str("## 六个操作配方\n\n");
    s.push_str("### 1. 查数据（只读）\n\n");
    s.push_str("先看结构，再查内容；筛选结果要能复现（把命令原样给用户）。\n\n");
    s.push_str("```bash\ncercis template <项目> --json\ncercis query <项目> --table <表> --filter <列> <运算符> <值> --sort <列>:desc --json\n```\n\n");
    s.push_str("### 2. 批量导入：必须先试运行\n\n");
    s.push_str("```bash\ncercis append-many <项目> --table <表> --from <文件.csv> --dry-run --json   # 先看会发生什么\ncercis append-many <项目> --table <表> --from <文件.csv> --source-ref <文件.csv> --json  # 用户确认后再写\n```\n\n");
    s.push_str("一次导入 = 一次事务 + 一条流水；模板之外的列默认会被拒绝（不要用 `--allow-new-columns` 绕过，除非用户明确要求）。\n\n");
    s.push_str("### 3. 录入单行 / 更正值\n\n");
    s.push_str("```bash\ncercis append <项目> --table <表> --set 列=值 --target <编号> --json\ncercis correct <项目> --table <表> --target <编号> --field <列> --old <旧值> --new <新值> --reason '<原因>' --json\n```\n\n");
    s.push_str("**永远不要**用改 CSV 的方式修改已录入的值——那会同时破坏留痕与历史。\n\n");
    s.push_str("### 4. 导出（要能回溯）\n\n");
    s.push_str("导出后必须能回答\"这批数字是从哪一版数据来的\"：带上筛选条件、排序与说明书。\n");
    s.push_str("装了基础工具包时优先用 `cercis-plugin-transfer export --readme`；只用内核时用 `cercis query` 取数后自行写出，并 `cercis note` 记一条流水。\n\n");
    s.push_str("### 5. 体检与校验\n\n");
    s.push_str("```bash\ncercis validate <项目> --json     # 引用完整性 + 字段规则\ncercis-plugin-health --project <项目> --json   # 装了基础工具包时：断链/编码/命名/孤儿附件\n```\n\n");
    s.push_str("### 6. 改结构（模板）要先问人\n\n");
    s.push_str("加列、改类型、换模板都会影响既有数据，属于**用户决策**：先说清影响，得到确认再执行。\n\n");

    s.push_str("## 命令速查（与 `ai_contract.json` 同源）\n\n");
    s.push_str("| 命令 | 用途 |\n|---|---|\n");
    for c in &contract.commands {
        s.push_str(&format!("| `{}` | {} |\n", c.name, c.purpose));
    }
    s.push('\n');
    s.push_str("完整示例见同目录 `reference.md`。\n");
    s
}

/// 生成 `reference.md`：完整命令速查 + 示例 + 契约摘要。
fn reference_md(contract: &AiContract) -> String {
    let mut s = String::new();
    s.push_str("# Cercis 命令速查（reference）\n\n");
    s.push_str(&format!(
        "> 由 Cercis 内核 v{} 生成；与 `_labnote/ai_contract.json` 同源，请勿手工编辑。\n\n",
        crate::CORE_VERSION
    ));
    s.push_str(&format!(
        "项目：{} ｜ 契约版本：{} ｜ 插件接口版本：{}\n\n",
        contract.project_name, contract.schema, contract.plugin_api_version
    ));

    s.push_str("## 内核命令\n\n");
    for c in &contract.commands {
        s.push_str(&format!("### {}\n\n", c.name));
        s.push_str(&format!("用途：{}\n\n", c.purpose));
        s.push_str(&format!("```bash\n{}\n```\n\n", c.example));
    }

    s.push_str("## 插件命令（装了基础工具包时可用）\n\n");
    s.push_str("| 命令 | 用途 |\n|---|---|\n");
    s.push_str("| `cercis-plugin-wizard --project <新目录> --name <名称> --template generic\\|example-study --allow-write` | 新建项目 |\n");
    s.push_str("| `cercis-plugin-entry append --project <项目> --table <表> --from <文件> --allow-write` | 批量录入（写入前查列名/必填/主键/引用） |\n");
    s.push_str("| `cercis-plugin-entry attach --project <项目> --table <表> --file <文件> --allow-write` | 附件挂接 |\n");
    s.push_str("| `cercis-plugin-transfer import --project <项目> --table <表> --file <文件> [--dry-run] [--report <md>] --allow-write` | CSV 导入（可出报告） |\n");
    s.push_str("| `cercis-plugin-transfer export --project <项目> --table <表> --out <目录> [--wide <列> --value <列>] --readme --allow-write` | 导出（长表/宽表 + 说明书） |\n");
    s.push_str("| `cercis-plugin-health --project <项目>` | 体检（只读插件） |\n\n");

    s.push_str("## 规则（来自契约）\n\n");
    for rule in &contract.rules {
        s.push_str(&format!("- {rule}\n"));
    }
    s.push('\n');
    s.push_str("## 禁止事项（来自契约）\n\n");
    for f in &contract.forbidden {
        s.push_str(&format!("- {f}\n"));
    }
    s
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

/// 一次生成全套装材料（AGENTS.md、CLAUDE.md、契约、清单、Agent 技能包）。
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

    let mut template = match tx::read_optional(&root.join("template.yaml"))? {
        Some(t) => Template::from_yaml(&t)?,
        None => Template::generic(),
    };
    // 术语包只改标签：AI 材料与契约显示领域用词（小区/处理），列名仍是 unit_id/treatment_id
    if let Some(terms) = crate::Terms::load(root)? {
        terms.apply_to(&mut template);
    }

    let contract = AiContract::build(&template, &manifest);
    contract.write(root)?;

    tx::write_atomic_str(
        &root.join("AGENTS.md"),
        &agents_md(&manifest.project_name, &template, &manifest),
    )?;
    tx::write_atomic_str(&root.join("CLAUDE.md"), &claude_md())?;

    // Agent 技能包：随项目生成一份，安装到 Codex / Claude Code 用（见 AgentSkill）
    AgentSkill::build(&contract).write_into_project(root)?;

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

        // 技能包随 AGENTS 一起生成，且能被 AI 宿主识别
        let skill = std::fs::read_to_string(dir.join("_labnote/skill/cercis/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: cercis\n"));
        assert!(skill.contains("description:"));
        assert!(skill.contains("禁止"));
        assert!(dir.join("_labnote/skill/cercis/reference.md").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 技能包里的命令速查必须与 `ai_contract.json` **完全同源**。
    ///
    /// 这条测试盯的是"三份文案互相矛盾"这个具体风险：
    /// 契约改了命令、技能包没跟上，AI 就会照着过期手册干活。
    #[test]
    fn skill_commands_match_contract_exactly() {
        let dir = std::env::temp_dir().join("cercis-skill-drift-test");
        let _ = std::fs::remove_dir_all(&dir);
        let p = crate::Project::create(&dir, "技能包测试", Identity::default()).unwrap();
        refresh_all(p.root()).unwrap();

        let contract_text = std::fs::read_to_string(dir.join("_labnote/ai_contract.json")).unwrap();
        let contract: AiContract = serde_json::from_str(&contract_text).unwrap();
        let skill = std::fs::read_to_string(dir.join("_labnote/skill/cercis/SKILL.md")).unwrap();
        let reference =
            std::fs::read_to_string(dir.join("_labnote/skill/cercis/reference.md")).unwrap();

        // 1) 每条契约命令都出现在两份文件里
        for c in &contract.commands {
            assert!(
                skill.contains(&c.name),
                "SKILL.md 缺少契约里的命令：{}",
                c.name
            );
            assert!(
                reference.contains(&c.name),
                "reference.md 缺少契约里的命令：{}",
                c.name
            );
            assert!(
                reference.contains(&c.example),
                "reference.md 缺少示例：{}",
                c.example
            );
        }

        // 2) 数量也要对得上（多了说明手写掺进来，少了说明漏渲染）
        let skill_rows = skill
            .lines()
            .filter(|l| l.starts_with("| `cercis ") && l.ends_with(" |"))
            .count();
        assert_eq!(
            skill_rows,
            contract.commands.len(),
            "技能包命令表行数与契约不一致"
        );

        // 3) 红线与禁止事项同样来自契约
        for rule in &contract.rules {
            assert!(skill.contains(rule), "SKILL.md 缺少规则：{rule}");
        }
        for f in &contract.forbidden {
            assert!(skill.contains(f), "SKILL.md 缺少禁止事项：{f}");
        }

        let _ = std::fs::remove_dir_all(&dir);
    }
}
