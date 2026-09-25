//! Cercis 命令行（`cercis`）：内核能力的第一个使用者。
//!
//! 设计目标：**人能用、脚本能用、AI 能用**。
//! - 所有命令支持 `--json`（稳定输出，带 schema 版本）
//! - 只读默认；写入命令明确列出，且全部经由内核通道留痕

use std::io::Read;
use std::path::{Path, PathBuf};

use clap::{Args, Parser, Subcommand};

use cercis_core::query::{Op, Query, SortDir};
use cercis_core::store::Identity;
use cercis_core::table::Row;
use cercis_core::{CercisError, Project};
use cercis_core::history::ChangeEntry;

/// Cercis —— 愿瑟希斯捍卫你的思想。
#[derive(Parser, Debug)]
#[command(
    name = "cercis",
    version,
    about = "Cercis 内核命令行（数据存取 · AI 友好）"
)]
struct Cli {
    /// 以 JSON 输出（机器/AI 读；schema 版本随输出携带）
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 新建项目
    Init(InitArgs),
    /// 打开项目并体检
    Doctor(ProjectArg),
    /// 列出某张表的内容
    Show(ShowArgs),
    /// 追加一行（写入）
    Append(AppendArgs),
    /// 批量追加多行（一次事务、一条流水；导入用）
    AppendMany(AppendManyArgs),
    /// 记录一条更正（写入；不覆盖原值）
    Correct(CorrectArgs),
    /// 查看变更流水
    Log(LogArgs),
    /// 记一条流水（插件记录导出等动作；不改动数据文件）
    Note(NoteArgs),
    /// 创建检查点（文件级备份）
    Checkpoint(CheckpointArgs),
    /// 还原某个历史对象到文件（回滚 = 新变更）
    Restore(RestoreArgs),
    /// 查看项目模板（表结构与字段定义）
    Template(TemplateArgs),
    /// 装入模板（示例研究 / 通用）
    InstallTemplate(InstallTemplateArgs),
    /// 查看或切换术语包（只改显示名，不动数据）
    Terms(TermsArgs),
    /// 按条件查询某张表（组合筛选 + 排序）
    Query(QueryArgs),
    /// 校验项目（引用完整性 + 字段规则）
    Validate(ProjectArg),
    /// 列出值类型与约束（内核唯一定义；插件与 AI 照用）
    ValueTypes,
    /// 看某列该用什么类型（候选 + 置信度 + 反例；只建议，不改模板）
    InferTypes(InferTypesArgs),
    /// 改模板里的列类型/约束（写入；默认先给影响预览）
    SetFieldType(SetFieldTypeArgs),
    /// 刷新机器清单与 AI 材料（AGENTS.md / CLAUDE.md / ai_contract.json / 技能包）
    Agents(ProjectArg),
    /// 生成 AI Agent 技能包（装到 Codex / Claude Code 用）
    Skill(SkillArgs),
    /// 生成机器清单
    ManifestCmd(ProjectArg),
    /// 列出插件（全局与项目内；项目内默认未信任）
    Plugins(PluginsArgs),
    /// 信任项目内插件（绑定内容指纹）
    Trust(TrustArgs),
    /// 内核信息
    Version,
}

#[derive(Args, Debug)]
struct TemplateArgs {
    path: PathBuf,
    /// 只显示某张表
    #[arg(long)]
    table: Option<String>,
    /// 显示**未经术语包覆盖**的原始模板
    #[arg(long)]
    raw: bool,
}

#[derive(Args, Debug)]
struct InstallTemplateArgs {
    path: PathBuf,
    /// 模板名：generic / example-study
    #[arg(long, conflicts_with = "file")]
    name: Option<String>,
    /// 从模板文件装入（科研工具包提供的模板走这条路）
    #[arg(long, value_name = "FILE")]
    file: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct TermsArgs {
    /// 项目目录
    path: PathBuf,
    /// 装入术语包文件（写入 <项目>/terms.yaml；不动数据）
    #[arg(long, value_name = "FILE")]
    file: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct InferTypesArgs {
    /// 项目目录
    path: PathBuf,
    /// 只看某张表（默认扫描模板里所有已存在的表）
    #[arg(long)]
    table: Option<String>,
    /// 只看某一列
    #[arg(long)]
    column: Option<String>,
    /// 强建议门槛（默认 0.95，即 95% 的格子匹配才给建议）
    #[arg(long, default_value_t = 0.95)]
    min_confidence: f64,
    /// 只列出"建议改类型"的列
    #[arg(long)]
    only_change: bool,
}

#[derive(Args, Debug)]
struct SetFieldTypeArgs {
    /// 项目目录
    path: PathBuf,
    /// 表名
    #[arg(long)]
    table: String,
    /// 列名
    #[arg(long)]
    field: String,
    /// 改成哪个值域：text / integer / number / bool / date / datetime / time
    #[arg(long = "type", value_name = "TYPE")]
    field_type: Option<String>,
    /// 设为必填
    #[arg(long, conflicts_with = "optional")]
    required: bool,
    /// 设为可选（取消必填）
    #[arg(long)]
    optional: bool,
    /// 计量单位（给空串即清空）
    #[arg(long)]
    unit: Option<String>,
    /// 建议范围（数值列）：如 `0,250`；`0,` 表示只设下限，`,250` 表示只设上限
    #[arg(long, value_name = "MIN,MAX", conflicts_with = "clear_range")]
    range: Option<String>,
    /// 清掉建议范围
    #[arg(long)]
    clear_range: bool,
    /// 允许值（分号分隔；给空串即清空）
    #[arg(long)]
    allowed_values: Option<String>,
    /// 正则模式（text 列；整格匹配）
    #[arg(long)]
    pattern: Option<String>,
    /// 小数位数（number 列；超出只提醒）
    #[arg(long, conflicts_with = "clear_precision")]
    precision: Option<u8>,
    /// 清掉小数位数
    #[arg(long)]
    clear_precision: bool,
    /// 引用 `表名.列名`（给空串即清空）
    #[arg(long)]
    reference: Option<String>,
    /// 更新这一列的说明
    #[arg(long)]
    notes: Option<String>,
    /// 只预览影响、不写模板
    #[arg(long)]
    dry_run: bool,
    /// 明知会新增校验错误也照改（数据可以随后补）
    #[arg(long)]
    force: bool,
    /// 写进变更流水的一句话（留空自动生成）
    #[arg(long, default_value = "")]
    summary: String,
}

#[derive(Args, Debug)]
struct QueryArgs {
    path: PathBuf,
    #[arg(long)]
    table: String,
    /// 筛选：col op value（如 --filter value_num ge 12）；可重复
    #[arg(long = "filter", value_name = "COL OP VAL", num_args = 3)]
    filters: Vec<String>,
    /// 排序：col 或 col:desc
    #[arg(long = "sort")]
    sort: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
    #[arg(long, default_value_t = 0)]
    offset: usize,
}

#[derive(Args, Debug)]
struct InitArgs {
    /// 项目目录（不存在则创建）
    path: PathBuf,
    /// 项目名称
    #[arg(long)]
    name: String,
    /// 用户名（默认取系统环境；仅记录用）
    #[arg(long, default_value = "local-user")]
    user: String,
    /// 是否记录设备信息
    #[arg(long)]
    record_device: bool,
}

#[derive(Args, Debug)]
struct ProjectArg {
    /// 项目目录
    path: PathBuf,
}

#[derive(Args, Debug)]
struct PluginsArgs {
    path: PathBuf,
    /// 全局插件目录（可选）
    #[arg(long)]
    global_dir: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct SkillArgs {
    /// 项目目录（技能包按它的模板与契约生成）
    path: PathBuf,
    /// 写到指定目录（默认写进项目 `_labnote/skill/`）
    #[arg(long)]
    out: Option<PathBuf>,
    /// 直接装到 AI 宿主的技能目录：codex（~/.codex/skills）或 claude（~/.claude/skills）
    #[arg(long)]
    install: Option<String>,
}

#[derive(Args, Debug)]
struct TrustArgs {
    path: PathBuf,
    /// 插件 ID
    #[arg(long)]
    id: String,
    /// 撤销信任（默认是授予信任）
    #[arg(long)]
    revoke: bool,
}

#[derive(Args, Debug)]
struct ShowArgs {
    path: PathBuf,
    /// 表名（不含 .csv）
    #[arg(long)]
    table: String,
    /// 最多显示多少行
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args, Debug)]
struct AppendArgs {
    path: PathBuf,
    #[arg(long)]
    table: String,
    /// 目标记录编号（写进流水，便于追溯）
    #[arg(long, default_value = "")]
    target: String,
    /// 一句话说明
    #[arg(long, default_value = "")]
    summary: String,
    /// 单元格：允许重复，形如 --set col=value
    #[arg(long = "set", value_name = "COL=VALUE")]
    sets: Vec<String>,
    /// 允许写入表里还不存在的列（按这一行自动扩展表结构）
    #[arg(long)]
    allow_new_columns: bool,
    /// 明知有类型错误也照写（默认拦下；数据可以随后补）
    #[arg(long)]
    force: bool,
}

#[derive(Args, Debug)]
struct AppendManyArgs {
    /// 项目目录
    path: PathBuf,
    #[arg(long)]
    table: String,
    /// 数据来源：CSV/TSV 文件、JSON 数组文件、或 `-`（从标准输入读 JSON）
    #[arg(long, value_name = "FILE|-")]
    from: String,
    /// 流水中记录的动作（create / import / …）
    #[arg(long, default_value = "import")]
    action: String,
    /// 一句话说明（留空自动生成）
    #[arg(long, default_value = "")]
    summary: String,
    /// 来源说明（导入文件路径等，便于追溯）
    #[arg(long, default_value = "")]
    source_ref: String,
    /// 允许写入模板未定义的列（默认拒绝）
    #[arg(long)]
    allow_new_columns: bool,
    /// 明知有类型错误也照写（默认拦下；数据可以随后补）
    #[arg(long)]
    force: bool,
    /// 只校验不写入（试运行）
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct CorrectArgs {
    path: PathBuf,
    /// 目标表
    #[arg(long)]
    table: String,
    /// 目标记录编号
    #[arg(long)]
    target: String,
    /// 字段名
    #[arg(long)]
    field: String,
    #[arg(long)]
    old: String,
    #[arg(long)]
    new: String,
    #[arg(long)]
    reason: String,
    #[arg(long, default_value = "")]
    source: String,
}

#[derive(Args, Debug)]
struct LogArgs {
    path: PathBuf,
    #[arg(long, default_value_t = 20)]
    limit: usize,
}

#[derive(Args, Debug)]
struct NoteArgs {
    /// 项目目录
    path: PathBuf,
    /// 动作类型：export / import / checkpoint / …
    #[arg(long)]
    action: String,
    /// 影响对象所在的表（可空）
    #[arg(long, default_value = "")]
    table: String,
    /// 影响对象的编号（可空）
    #[arg(long, default_value = "")]
    target: String,
    /// 一句话说明（谁做了什么、结果如何）
    #[arg(long)]
    summary: String,
    /// 来源说明（导出文件路径等）
    #[arg(long, default_value = "")]
    source_ref: String,
}

#[derive(Args, Debug)]
struct CheckpointArgs {
    path: PathBuf,
    #[arg(long, default_value = "手动检查点")]
    note: String,
}

#[derive(Args, Debug)]
struct RestoreArgs {
    path: PathBuf,
    /// 历史对象指纹（sha256）
    #[arg(long)]
    hash: String,
    /// 还原到哪个文件（项目内相对路径或绝对路径）
    #[arg(long)]
    to: PathBuf,
    #[arg(long, default_value = "回滚到历史版本")]
    summary: String,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        // 错误也要能机器读
        if cli.json {
            let payload = serde_json::json!({
                "schema": "cercis.cli/1",
                "ok": false,
                "error": e.to_string(),
            });
            println!("{}", serde_json::to_string_pretty(&payload).unwrap());
        } else {
            eprintln!("错误: {e}");
        }
        std::process::exit(1);
    }
}

fn run(cli: &Cli) -> Result<(), CercisError> {
    match &cli.command {
        Command::Template(a) => {
            let p = Project::open(&a.path, None)?;
            // 默认显示"术语包生效后"的标签；--raw 看原始模板
            let (t, terms) = if a.raw {
                (p.template()?, None)
            } else {
                p.template_with_terms()?
            };
            let profile = terms
                .as_ref()
                .map(|x| x.profile.clone())
                .unwrap_or_else(|| t.terms_profile.clone());
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "terms_profile": profile,
                    "terms_applied": terms.is_some(),
                    "terms_overrides": terms.as_ref().map(|x| x.override_count()).unwrap_or(0),
                    "template": t,
                }));
            } else {
                println!("模板：{}（{}）", t.name, t.label_cn);
                if let Some(pack) = &terms {
                    println!(
                        "术语包：{}（{}，覆盖 {} 处标签；列名不变）",
                        pack.profile,
                        pack.label_cn,
                        pack.override_count()
                    );
                }
                for td in &t.tables {
                    if let Some(only) = &a.table {
                        if only != &td.name {
                            continue;
                        }
                    }
                    println!(
                        "\n表 {}（{}） 主键: {}",
                        td.name, td.label_cn, td.primary_key
                    );
                    for f in &td.fields {
                        let unit = if f.unit.is_empty() {
                            String::new()
                        } else {
                            format!(" 单位={}", f.unit)
                        };
                        let req = if f.required { " 必填" } else { "" };
                        let rng = match &f.range {
                            Some(r) if r.len() == 2 => format!(
                                " 范围=[{}, {}]",
                                r[0].map(|v| v.to_string()).unwrap_or_default(),
                                r[1].map(|v| v.to_string()).unwrap_or_default()
                            ),
                            _ => String::new(),
                        };
                        let reff = f
                            .reference
                            .as_ref()
                            .map(|r| format!(" 引用={r}"))
                            .unwrap_or_default();
                        println!(
                            "  {:<20} {:<12} {}{}{}{}",
                            f.name, f.label_cn, f.field_type, unit, req, rng
                        );
                        if !reff.is_empty() {
                            println!("  {:<20} ↳{}", "", reff);
                        }
                    }
                }
            }
        }

        Command::InstallTemplate(a) => {
            let p = Project::open(&a.path, None)?;
            let t = match (&a.file, &a.name) {
                // 从文件装入：科研工具包等外部提供模板走这条通道
                // （内核保持行业普适——它不认识"科研"，只认识"模板文件"）
                (Some(file), _) => {
                    let text = std::fs::read_to_string(file).map_err(|e| {
                        CercisError::NotFound(format!("读不到模板文件 {}：{e}", file.display()))
                    })?;
                    cercis_core::Template::from_yaml(&text)?
                }
                (None, Some(name)) => match name.as_str() {
                    "generic" => cercis_core::Template::generic(),
                    "example-study" => cercis_core::Template::example_study(),
                    other => {
                        return Err(CercisError::Invalid(format!(
                            "未知模板 {other}（可用：generic / example-study，或用 --file 指定模板文件）"
                        )))
                    }
                },
                (None, None) => {
                    return Err(CercisError::Invalid(
                        "请给 --name <模板名> 或 --file <模板文件>".into(),
                    ))
                }
            };
            let created = p.install_template(&t)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "template": t.name,
                    "created_tables": created,
                }));
            } else {
                println!("已装入模板：{}", t.name);
                if let Some(f) = &a.file {
                    println!("来源：{}", f.display());
                }
                if created.is_empty() {
                    println!("（表已存在，未新建）");
                } else {
                    println!("新建表：{}", created.join(", "));
                }
            }
        }

        Command::Terms(a) => {
            let p = Project::open(&a.path, None)?;
            if let Some(file) = &a.file {
                let text = std::fs::read_to_string(file).map_err(|e| {
                    CercisError::NotFound(format!("读不到术语包 {}：{e}", file.display()))
                })?;
                let terms = cercis_core::Terms::from_yaml(&text)?;
                p.install_terms(&terms)?;
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "profile": terms.profile,
                        "label_cn": terms.label_cn,
                        "overrides": terms.override_count(),
                        "source": file.display().to_string(),
                    }));
                } else {
                    println!(
                        "已切换术语包：{}（{}）——覆盖 {} 处标签，**数据文件未改动**",
                        terms.profile,
                        terms.label_cn,
                        terms.override_count()
                    );
                }
                return Ok(());
            }

            match p.terms()? {
                Some(t) => {
                    if cli.json {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "profile": t.profile,
                            "label_cn": t.label_cn,
                            "overrides": t.override_count(),
                        }));
                    } else {
                        println!(
                            "当前术语包：{}（{}）｜覆盖 {} 处标签",
                            t.profile,
                            t.label_cn,
                            t.override_count()
                        );
                    }
                }
                None => {
                    if cli.json {
                        print_json(&serde_json::json!({
                            "schema": "cercis.cli/1",
                            "ok": true,
                            "profile": "（未装术语包）",
                        }));
                    } else {
                        println!("未装术语包：界面按模板原本的中英文标签显示。");
                        println!("装入方式：cercis terms <项目> --file <术语包.yaml>");
                    }
                }
            }
        }

        Command::Query(a) => {
            let p = Project::open(&a.path, None)?;
            let mut q = Query::new();

            // 解析 --filter col op value（三连）
            if a.filters.len() % 3 != 0 {
                return Err(CercisError::Invalid(
                    "--filter 需要三个值：列名 运算符 值".into(),
                ));
            }
            for trio in a.filters.chunks(3) {
                let (col, op, val) = (&trio[0], &trio[1], &trio[2]);
                let op = match op.as_str() {
                    "eq" => Op::Eq,
                    "ne" => Op::Ne,
                    "lt" => Op::Lt,
                    "le" => Op::Le,
                    "gt" => Op::Gt,
                    "ge" => Op::Ge,
                    "contains" => Op::Contains,
                    "startswith" => Op::StartsWith,
                    "in" => Op::In,
                    "isempty" => Op::IsEmpty,
                    "notempty" => Op::NotEmpty,
                    other => {
                        return Err(CercisError::Invalid(format!(
                            "未知运算符 {other}（可用: eq ne lt le gt ge contains startswith in isempty notempty）"
                        )))
                    }
                };
                q = q.filter(col, op, val);
            }

            if let Some(sort) = &a.sort {
                let (col, dir) = match sort.split_once(':') {
                    Some((c, "desc")) => (c, SortDir::Desc),
                    Some((c, "asc")) => (c, SortDir::Asc),
                    Some((_, other)) => {
                        return Err(CercisError::Invalid(format!(
                            "排序方向只能是 asc 或 desc，实际: {other}"
                        )))
                    }
                    None => (sort.as_str(), SortDir::Asc),
                };
                q = q.sort(col, dir);
            }
            q = q.page(a.offset, a.limit);

            let rows = p.query(&a.table, q)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "table": a.table,
                    "count": rows.len(),
                    "rows": rows,
                }));
            } else {
                println!("表 {}：命中 {} 行", a.table, rows.len());
                for r in &rows {
                    let line = r
                        .iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join("  ");
                    println!("  {line}");
                }
            }
        }

        Command::Agents(a) => {
            let m = cercis_core::ai::refresh_all(&a.path)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "project": m.project_name,
                    "tables": m.tables.len(),
                }));
            } else {
                println!("已刷新 AI 材料：");
                println!("  AGENTS.md（AI 说明书）");
                println!("  CLAUDE.md（其他 AI 工具入口）");
                println!("  _labnote/manifest.json（机器清单）");
                println!("  _labnote/ai_contract.json（机器契约）");
                println!("  _labnote/skill/cercis/（Agent 技能包，装到 Codex / Claude Code 用）");
            }
        }

        Command::Skill(a) => {
            if a.out.is_some() && a.install.is_some() {
                return Err(CercisError::Invalid(
                    "--out 与 --install 只能用一个（前者写到你指定的目录，后者装进 AI 宿主）".into(),
                ));
            }
            let p = Project::open(&a.path, None)?;
            let manifest = cercis_core::Manifest::generate(&a.path)?;
            let contract = cercis_core::AiContract::build(&p.template()?, &manifest);
            let skill = cercis_core::AgentSkill::build(&contract);

            let (target, host) = if let Some(host) = a.install.as_deref() {
                (skill.write(skill_host_dir(host)?)?, Some(host.to_string()))
            } else if let Some(out) = a.out.as_ref() {
                (skill.write(out)?, None)
            } else {
                (skill.write_into_project(&a.path)?, None)
            };

            let files = vec![
                target.join("SKILL.md").display().to_string(),
                target.join("reference.md").display().to_string(),
            ];
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "skill": skill.name,
                    "target": target.display().to_string(),
                    "host": host,
                    "files": files,
                    "commands": contract.commands.len(),
                }));
            } else {
                println!("已生成 Agent 技能包：{}", target.display());
                for f in &files {
                    println!("  {f}");
                }
                println!(
                    "说明：技能包与 _labnote/ai_contract.json 同源（{} 条命令）；改模板后重新生成即可。",
                    contract.commands.len()
                );
                if host.is_none() {
                    println!("安装：cercis skill <项目> --install codex（或 claude）");
                }
            }
        }

        Command::ManifestCmd(a) => {
            let m = cercis_core::Manifest::generate(&a.path)?;
            m.write(&a.path)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "manifest": m,
                }));
            } else {
                println!(
                    "已生成清单：{} 张表，变更 {} 条",
                    m.tables.len(),
                    m.change_count
                );
                for t in &m.tables {
                    println!("  {:<16} {:>6} 行  {} 列", t.name, t.rows, t.columns.len());
                }
            }
        }

        Command::Plugins(a) => {
            let found = cercis_core::plugin::discover(&a.path, a.global_dir.as_deref())?;
            let trust = cercis_core::TrustRecord::load(&a.path)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "plugins": found.iter().map(|p| serde_json::json!({
                        "id": p.manifest.id,
                        "name": p.manifest.name,
                        "tier": p.manifest.tier,
                        "origin": p.origin,
                        "fingerprint": p.fingerprint,
                        "trusted": trust.is_trusted(&p.manifest.id, &p.fingerprint),
                        "permissions": p.manifest.permission_summary(),
                        "ui": p.manifest.ui.len(),
                    })).collect::<Vec<_>>(),
                }));
            } else if found.is_empty() {
                println!("未发现插件。");
            } else {
                for p in &found {
                    let trusted = trust.is_trusted(&p.manifest.id, &p.fingerprint);
                    let status = if p.origin == "project" {
                        if trusted {
                            "已信任"
                        } else {
                            "未信任（默认不加载）"
                        }
                    } else {
                        "全局"
                    };
                    println!(
                        "{}  {}  [{}]  {}  权限：{}  界面扩展：{}",
                        p.manifest.id,
                        p.manifest.name,
                        p.origin,
                        status,
                        p.manifest.permission_summary(),
                        p.manifest.ui.len()
                    );
                }
            }
        }

        Command::Trust(a) => {
            let found = cercis_core::plugin::discover(&a.path, None)?;
            let target = found
                .iter()
                .find(|p| p.manifest.id == a.id)
                .ok_or_else(|| CercisError::NotFound(format!("未找到插件：{}", a.id)))?;
            let mut tr = cercis_core::TrustRecord::load(&a.path)?;
            if a.revoke {
                tr.untrust(&a.id);
                tr.save(&a.path)?;
                println!("已撤销信任：{}", a.id);
            } else {
                tr.trust(&a.id, &target.fingerprint);
                tr.save(&a.path)?;
                println!("已信任插件：{}", a.id);
                println!("  权限：{}", target.manifest.permission_summary());
                println!("  指纹：{}", target.fingerprint);
                println!("（内容一旦变化，信任将自动失效）");
            }
        }

        Command::Validate(a) => {
            let p = Project::open(&a.path, None)?;
            let report = p.validate()?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": report.is_ok(),
                    "errors": report.errors,
                    "warnings": report.warnings,
                }));
            } else {
                print!("{}", cercis_core::validate::report_to_text(&report));
            }
            if !report.is_ok() {
                std::process::exit(2);
            }
        }

        Command::ValueTypes => {
            let c = cercis_core::value::contract();
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "value_types": c,
                }));
            } else {
                println!("值类型（内核唯一定义；插件与 AI 照用，不得自造）\n");
                for d in &c.domains {
                    println!(
                        "  - {:<9} {:<6}｜违反时：{}",
                        d.name,
                        d.label_cn,
                        severity_cn(d.severity_on_violation)
                    );
                    println!("      怎么写：{}", d.csv_form);
                    if !d.examples_ok.is_empty() {
                        println!("      合法例子：{}", d.examples_ok.join("、"));
                    }
                    if !d.examples_bad.is_empty() {
                        println!("      必须报告：{}", d.examples_bad.join("、"));
                    }
                    if !d.inferable {
                        println!("      注：只校验、不参与类型推断");
                    }
                }
                println!("\n约束（与类型正交，可组合）");
                for k in &c.constraints {
                    println!(
                        "  - {:<15} {:<6}｜适用于 {}｜{}",
                        k.name,
                        k.label_cn,
                        k.applies_to,
                        severity_cn(k.severity)
                    );
                    println!("      {}", k.notes);
                }
                println!("\n{}", c.missing_value_rule);
                println!("\n校验时点：");
                for p in &c.validation_points {
                    println!("  - {p}");
                }
                println!("\n{}", c.severity_policy);
            }
        }

        Command::InferTypes(a) => {
            let p = Project::open(&a.path, None)?;
            let template = p.template()?;
            let tables: Vec<String> = match &a.table {
                Some(t) => vec![t.clone()],
                None => template.tables.iter().map(|t| t.name.clone()).collect(),
            };
            let mut columns: Vec<cercis_core::infer::ColumnInference> = Vec::new();
            let mut scanned_tables: Vec<String> = Vec::new();
            for name in &tables {
                let path = p.root().join("tables").join(format!("{name}.csv"));
                if !path.exists() {
                    continue;
                }
                let table = p.store.load_table_with_template(name, &template)?;
                let declared: Vec<(String, String)> = template
                    .table(name)
                    .map(|td| {
                        td.fields
                            .iter()
                            .map(|f| (f.name.clone(), f.field_type.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut found = cercis_core::infer::infer_table(&table, &declared);
                for c in found.iter_mut() {
                    c.table = name.clone();
                }
                columns.extend(found);
                scanned_tables.push(name.clone());
            }
            if let Some(only) = &a.column {
                columns.retain(|c| &c.column == only);
            }
            if a.only_change {
                columns.retain(|c| c.suggest_change);
            }

            let suggest_count = columns.iter().filter(|c| c.suggest_change).count();
            let declared_bad = columns.iter().filter(|c| c.declared_violations > 0).count();

            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "threshold": a.min_confidence,
                    "tables_scanned": scanned_tables,
                    "columns": columns,
                    "summary": {
                        "columns": columns.len(),
                        "suggest_change": suggest_count,
                        "columns_with_violations": declared_bad,
                    },
                }));
            } else if columns.is_empty() {
                println!("没有可推断的列（表不存在，或没有数据）。");
            } else {
                for c in &columns {
                    let s = &c.profile.stats;
                    let head = format!("{}.{}（声明 {}）", c.table, c.column, c.declared_type);
                    match &c.profile.recommended {
                        Some(r) if c.suggest_change => {
                            println!("{head} → 建议改成 {r}（{}）", confidence_text(&c.profile));
                        }
                        Some(r) => {
                            println!("{head} → 与建议一致（{r}，{}）", confidence_text(&c.profile));
                        }
                        None => {
                            println!("{head} → 无强建议（{}）", confidence_text(&c.profile));
                        }
                    }
                    println!(
                        "    非空 {}／共 {} 格（空 {}，占位符写法 {}）",
                        s.non_empty, s.total, s.empty, s.missing_markers
                    );
                    if c.declared_violations > 0 {
                        println!(
                            "    ⚠ 有 {} 格不符合声明的 {} 类型：{}",
                            c.declared_violations,
                            c.declared_type,
                            c.declared_counterexamples.join("、")
                        );
                    }
                    let counter: Vec<String> = c
                        .profile
                        .candidates
                        .iter()
                        .filter(|cand| !cand.counterexamples.is_empty())
                        .take(1)
                        .flat_map(|cand| {
                            cand.counterexamples
                                .iter()
                                .map(|v| format!("{v}（不符 {}）", cand.domain))
                                .collect::<Vec<_>>()
                        })
                        .collect();
                    if !counter.is_empty() && c.suggest_change {
                        println!("    反例：{}", counter.join("、"));
                    }
                    for n in &c.profile.notes {
                        println!("    注：{n}");
                    }
                }
                println!(
                    "\n共 {} 列（表格 {}）：{} 列建议改类型，{} 列有不符合当前类型的格子。",
                    columns.len(),
                    scanned_tables.join("、"),
                    suggest_count,
                    declared_bad
                );
                println!("提示：这只是建议，不会自动改模板；确认后用 set-field-type 改（先加 --dry-run 看影响）。");
            }
        }

        Command::SetFieldType(a) => {
            let p = Project::open(&a.path, None)?;
            let patch = build_field_patch(a)?;
            if patch.is_empty() {
                return Err(CercisError::Invalid(
                    "没有要改的内容（至少给一项：--type / --required / --optional / --range / --pattern / --precision / --allowed-values / --reference / --unit / --notes）"
                        .into(),
                ));
            }
            let current = p.template()?;
            let next = current.with_field_patch(&a.table, &a.field, &patch)?;

            // 影响预览：拿新模板去校验现有数据，只算"这一次改动**新增**的问题"
            let before = p.validate()?;
            let after = p.validate_with(&next)?;
            let mut new_errors: Vec<&cercis_core::Issue> = Vec::new();
            let mut new_warnings: Vec<&cercis_core::Issue> = Vec::new();
            for issue in after.errors.iter() {
                if !before.errors.iter().any(|b| same_issue(b, issue)) {
                    new_errors.push(issue);
                }
            }
            for issue in after.warnings.iter() {
                if !before.warnings.iter().any(|b| same_issue(b, issue)) {
                    new_warnings.push(issue);
                }
            }

            let describe = patch.describe();
            if !new_errors.is_empty() && !a.force {
                let mut msg = format!(
                    "这次改动会让 {} 处格子不符合校验（示例：{}）",
                    new_errors.len(),
                    new_errors
                        .iter()
                        .take(3)
                        .map(|e| format!("{}:{} {}.{}", e.table, e.row, e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("；")
                );
                msg.push_str("。已存在的原值不会被改动：直接写会留下这些错误，体检会报出来。确认要改就加 --force（数据随后补），或先清理数据。");
                return Err(CercisError::Invalid(msg));
            }

            if a.dry_run {
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "dry_run": true,
                        "table": a.table,
                        "field": a.field,
                        "change": describe,
                        "new_errors": new_errors.len(),
                        "new_warnings": new_warnings.len(),
                        "errors": new_errors,
                        "warnings": new_warnings,
                    }));
                } else {
                    println!("试运行：{}.{} 将改为 {}", a.table, a.field, describe);
                    println!(
                        "影响：新增错误 {} 处，新增提醒 {} 处（未写模板）",
                        new_errors.len(),
                        new_warnings.len()
                    );
                    for e in new_errors.iter().take(5) {
                        println!("  错误 {}:{} {}：{}", e.table, e.row, e.field, e.message);
                    }
                    for w in new_warnings.iter().take(5) {
                        println!("  提醒 {}:{} {}：{}", w.table, w.row, w.field, w.message);
                    }
                    println!("（去掉 --dry-run 即正式写入模板；数据文件不会被改动）");
                }
                return Ok(());
            }

            let (_, describe) = p.apply_field_patch(&a.table, &a.field, &patch, &a.summary)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": false,
                    "table": a.table,
                    "field": a.field,
                    "change": describe,
                    "new_errors": new_errors.len(),
                    "new_warnings": new_warnings.len(),
                }));
            } else {
                println!(
                    "已修改模板：{}.{}（{}）",
                    a.table, a.field, describe
                );
                println!(
                    "数据文件未改动；新增错误 {} 处、新增提醒 {} 处（cercis validate 可复核）",
                    new_errors.len(),
                    new_warnings.len()
                );
                if !a.summary.is_empty() {
                    println!("说明：{}", a.summary);
                }
            }
        }

        Command::Version => {
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "core_version": cercis_core::CORE_VERSION,
                    "schema_version": cercis_core::SCHEMA_VERSION,
                    "dirs": cercis_core::project::DIRS.len(),
                }));
            } else {
                println!(
                    "Cercis 内核 {}（项目结构版本 {}）",
                    cercis_core::CORE_VERSION,
                    cercis_core::SCHEMA_VERSION
                );
                println!("愿瑟希斯捍卫你的思想。 / May Cerces safeguard your thoughts.");
            }
        }

        Command::Init(a) => {
            let identity = Identity {
                user_id: a.user.clone(),
                device_id: hostname_or("local-device"),
                record_device: a.record_device,
            };
            let p = Project::create(&a.path, &a.name, identity)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "project": p.root().display().to_string(),
                    "name": p.config.name,
                }));
            } else {
                println!("已创建项目：{}", p.root().display());
                println!("项目名称：{}", p.config.name);
            }
        }

        Command::Doctor(a) => {
            let p = Project::open(&a.path, None)?;
            let issues = p.doctor()?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "issues": issues,
                }));
            } else if issues.is_empty() {
                println!("体检通过，未发现问题。");
            } else {
                println!("发现 {} 项：", issues.len());
                for i in issues {
                    println!("  - {i}");
                }
            }
        }

        Command::Show(a) => {
            let p = Project::open(&a.path, None)?;
            let t = p.store.load_table(&a.table, &[])?;
            println!(
                "表 {}（{} 列，{} 行；显示前 {} 行）",
                a.table,
                t.columns.len(),
                t.rows.len(),
                a.limit
            );
            for r in t.rows.iter().take(a.limit) {
                let cells = t
                    .columns
                    .iter()
                    .map(|c| format!("{c}={}", r.get(c).cloned().unwrap_or_default()))
                    .collect::<Vec<_>>()
                    .join("  ");
                println!("  {cells}");
            }
        }

        Command::Append(a) => {
            let p = Project::open(&a.path, None)?;
            let mut row = Row::new();
            for kv in &a.sets {
                let (k, v) = kv.split_once('=').ok_or_else(|| {
                    CercisError::Invalid(format!("--set 需要 col=value 形式，实际: {kv}"))
                })?;
                row.insert(k.trim().to_string(), v.trim().to_string());
            }
            if row.is_empty() {
                return Err(CercisError::Invalid("至少提供一个 --set col=value".into()));
            }
            // 写入前的那道校验（§5.9.3）：结构错误拦下，提醒放行
            let template = p.template()?;
            let (errors, warnings) = check_cells_against_template(&template, &a.table, &[row.clone()]);
            if !errors.is_empty() && !a.force {
                return Err(CercisError::Invalid(format!(
                    "{} 处格子不符合模板的类型规则（{}）。要么改值，要么加 --force 照写（会留下已知错误）",
                    errors.len(),
                    errors
                        .iter()
                        .take(3)
                        .map(|e| format!("{}：{}", e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("；")
                )));
            }
            for w in warnings.iter().take(3) {
                eprintln!("提醒 [{}] {}：{}", w.row, w.field, w.message);
            }
            // 若表不存在，按本次提供的列创建（列名按字母序稳定）
            let owned: Vec<String> = row.keys().cloned().collect();
            let cols: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            if a.allow_new_columns {
                p.store
                    .append_row_extending(&a.table, &cols, row, &a.target, &a.summary)?;
            } else {
                p.store
                    .append_row(&a.table, &cols, row, &a.target, &a.summary)?;
            }
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "table": a.table,
                    "target": a.target,
                    "extended_columns": a.allow_new_columns,
                }));
            } else if a.allow_new_columns {
                println!("已写入表 {}（必要时自动扩展了列；已留痕）", a.table);
            } else {
                println!("已写入表 {}（已留痕）", a.table);
            }
        }

        Command::AppendMany(a) => {
            let rows = read_rows_input(&a.from)?;
            if rows.is_empty() {
                return Err(CercisError::Invalid(format!("{} 中没有数据行", a.from)));
            }

            let p = Project::open(&a.path, None)?;
            let template = p.template()?;
            let template_cols: Vec<String> = template
                .table(&a.table)
                .map(|td| td.fields.iter().map(|f| f.name.clone()).collect())
                .unwrap_or_default();

            // 输入涉及的列：模板列在前，输入里新增的列在后（顺序稳定，便于复核）
            let mut columns = template_cols.clone();
            for r in &rows {
                for k in r.keys() {
                    if !columns.contains(k) {
                        columns.push(k.clone());
                    }
                }
            }

            // 模板已定义该表时，默认拒绝写入模板之外的列（防止手滑多写一列脏数据）
            if !template_cols.is_empty() && !a.allow_new_columns {
                let extra: Vec<&str> = columns[template_cols.len()..]
                    .iter()
                    .map(|s| s.as_str())
                    .collect();
                if !extra.is_empty() {
                    return Err(CercisError::Invalid(format!(
                        "输入含有模板 {} 未定义的列：{}（请先扩展模板，或显式加 --allow-new-columns）",
                        template.name,
                        extra.join(", ")
                    )));
                }
            }

            let summary = if a.summary.is_empty() {
                format!("导入 {} 行到表 {}（来源：{}）", rows.len(), a.table, a.from)
            } else {
                a.summary.clone()
            };

            // 写入前的类型校验（§5.9.3 的第一个时点）：结构错误整批拦下，提醒放行。
            let (cell_errors, cell_warnings) =
                check_cells_against_template(&template, &a.table, &rows);

            if a.dry_run {
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "dry_run": true,
                        "table": a.table,
                        "rows": rows.len(),
                        "columns": columns,
                        "errors": cell_errors,
                        "warnings": cell_warnings,
                        "sample": rows.iter().take(3).collect::<Vec<_>>(),
                    }));
                } else {
                    println!("试运行：表 {} 将新增 {} 行", a.table, rows.len());
                    println!("列：{}", columns.join(", "));
                    for r in rows.iter().take(3) {
                        let line = r
                            .iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join("  ");
                        println!("  {line}");
                    }
                    if cell_errors.is_empty() {
                        println!("校验：没有结构错误（{} 处提醒）", cell_warnings.len());
                    } else {
                        println!("校验：{} 处错误（这些会让整批写不进去）", cell_errors.len());
                        for e in cell_errors.iter().take(8) {
                            println!("  错误 第 {} 行 {}：{}", e.row, e.field, e.message);
                        }
                        if cell_errors.len() > 8 {
                            println!("  ……还有 {} 处", cell_errors.len() - 8);
                        }
                    }
                    for w in cell_warnings.iter().take(8) {
                        println!("  提醒 第 {} 行 {}：{}", w.row, w.field, w.message);
                    }
                    println!("（未写入任何数据；去掉 --dry-run 即正式导入）");
                }
                return Ok(());
            }

            if !cell_errors.is_empty() && !a.force {
                return Err(CercisError::Invalid(format!(
                    "有 {} 处格子不符合模板的类型规则，整批未写入（先加 --dry-run 看清单；要照写加 --force）。示例：{}",
                    cell_errors.len(),
                    cell_errors
                        .iter()
                        .take(3)
                        .map(|e| format!("第 {} 行 {}：{}", e.row, e.field, e.message))
                        .collect::<Vec<_>>()
                        .join("；")
                )));
            }

            let refs: Vec<&str> = columns.iter().map(|s| s.as_str()).collect();
            let outcome = p.store.append_rows(
                &a.table,
                &refs,
                rows,
                &a.action,
                &summary,
                &a.source_ref,
            )?;

            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "dry_run": false,
                    "table": a.table,
                    "rows_written": outcome.rows_written,
                    "batch_id": outcome.batch_id,
                    "hash_after": outcome.hash_after,
                    "action": a.action,
                    "source_ref": a.source_ref,
                }));
            } else {
                println!(
                    "已导入 {} 行到表 {}（一次事务、一条流水；批次 {}）",
                    outcome.rows_written, a.table, outcome.batch_id
                );
            }
        }

        Command::Correct(a) => {
            let p = Project::open(&a.path, None)?;
            let id = p.store.correct(
                &a.table, &a.target, &a.field, &a.old, &a.new, &a.reason, &a.source,
            )?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "correction_id": id,
                }));
            } else {
                println!("已记录更正：{id}（原值永久保留）");
            }
        }

        Command::Log(a) => {
            let p = Project::open(&a.path, None)?;
            let entries = p.store.read_change_log()?;
            let list: Vec<_> = entries.iter().rev().take(a.limit).collect();
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "count": list.len(),
                    "entries": list,
                }));
            } else {
                for e in list {
                    println!(
                        "{}  {}  {}  {}  {}",
                        e.ts, e.action, e.target_table, e.target_id, e.summary
                    );
                }
            }
        }

        Command::Note(a) => {
            let p = Project::open(&a.path, None)?;
            if a.action.trim().is_empty() || a.summary.trim().is_empty() {
                return Err(CercisError::Invalid(
                    "note 需要 --action 与 --summary（这条流水说明了什么）".into(),
                ));
            }
            let mut entry = ChangeEntry::new(
                &p.store.identity.user_id,
                &p.store.identity.device_for_log(),
                a.action.trim(),
                &a.table,
                &a.target,
                &a.summary,
                "",
                "",
            );
            entry.source_ref = a.source_ref.clone();
            entry.tool_version = cercis_core::CORE_VERSION.to_string();
            p.store.append_change(&entry)?;

            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "change_id": entry.change_id,
                    "action": entry.action,
                    "ts": entry.ts,
                }));
            } else {
                println!("已记入流水：{}（{}）", entry.action, entry.summary);
            }
        }

        Command::Checkpoint(a) => {
            let p = Project::open(&a.path, None)?;
            let dir = p.checkpoint(&a.note)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "checkpoint": dir.display().to_string(),
                }));
            } else {
                println!("已创建检查点：{}", dir.display());
            }
        }

        Command::Restore(a) => {
            let p = Project::open(&a.path, None)?;
            p.store.restore_object_to_file(&a.hash, &a.to, &a.summary)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "restored_to": a.to.display().to_string(),
                    "hash": a.hash,
                }));
            } else {
                println!("已还原到 {}", a.to.display());
            }
        }
    }
    Ok(())
}

fn print_json(v: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(v).unwrap_or_else(|_| "{}".into())
    );
}

/// 强度写成中文（值类型清单用）。
fn severity_cn(severity: &str) -> &'static str {
    match severity {
        "error" => "错误（阻止写入）",
        "warning" => "提醒（放行）",
        _ => "不校验",
    }
}

/// 置信度写成"最高匹配 integer 99%（99/100 格）"这样。
fn confidence_text(profile: &cercis_core::infer::Profile) -> String {
    let best = profile
        .candidates
        .iter()
        .filter(|c| c.domain != "text")
        .max_by(|a, b| {
            a.confidence
                .partial_cmp(&b.confidence)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    match best {
        Some(c) => format!(
            "最高匹配 {} {:.0}%（{}/{} 格）",
            c.domain,
            c.confidence * 100.0,
            c.matched,
            profile.stats.non_empty
        ),
        None => "没有候选类型".into(),
    }
}

/// 把 `--type/--range/...` 组装成字段补丁。
fn build_field_patch(
    a: &SetFieldTypeArgs,
) -> Result<cercis_core::template::FieldPatch, CercisError> {
    let range = match &a.range {
        None => None,
        Some(spec) => {
            let (lo, hi) = spec.split_once(',').ok_or_else(|| {
                CercisError::Invalid(format!(
                    "范围要写成 `最小值,最大值`（可留空一端，如 `0,` 或 `,250`）：{spec}"
                ))
            })?;
            let parse = |s: &str| -> Result<Option<f64>, CercisError> {
                let s = s.trim();
                if s.is_empty() {
                    return Ok(None);
                }
                s.parse::<f64>()
                    .map(Some)
                    .map_err(|_| CercisError::Invalid(format!("范围里的数读不出来：{s}")))
            };
            Some(vec![parse(lo)?, parse(hi)?])
        }
    };
    Ok(cercis_core::template::FieldPatch {
        field_type: a.field_type.clone(),
        required: if a.required {
            Some(true)
        } else if a.optional {
            Some(false)
        } else {
            None
        },
        unit: a.unit.clone(),
        range,
        clear_range: a.clear_range,
        allowed_values: a.allowed_values.clone(),
        pattern: a.pattern.clone(),
        precision: a.precision,
        clear_precision: a.clear_precision,
        reference: a.reference.clone(),
        notes: a.notes.clone(),
    })
}

/// 两条校验结论是不是同一条（用于算"这次改动新增了哪些问题"）。
fn same_issue(a: &cercis_core::Issue, b: &cercis_core::Issue) -> bool {
    a.kind == b.kind
        && a.table == b.table
        && a.row == b.row
        && a.field == b.field
        && a.message == b.message
}

/// 用内核的类型规则检查一批**即将写入**的行（§5.9.3 的第一个校验时点：写入前）。
///
/// 只检查模板里声明过的列：模板之外的列（`--allow-new-columns`）没有类型可依据，
/// 直接跳过——**放行不猜**。
fn check_cells_against_template(
    template: &cercis_core::Template,
    table: &str,
    rows: &[Row],
) -> (Vec<cercis_core::Issue>, Vec<cercis_core::Issue>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let Some(td) = template.table(table) else {
        return (errors, warnings);
    };
    for (i, row) in rows.iter().enumerate() {
        for f in &td.fields {
            let Some(cell) = row.get(&f.name) else {
                continue;
            };
            for finding in cercis_core::value::check_value(f.domain(), &f.constraints(), cell) {
                let issue = cercis_core::Issue {
                    level: finding.severity.as_str().to_string(),
                    kind: finding.kind.to_string(),
                    table: table.to_string(),
                    row: i + 1,
                    field: f.name.clone(),
                    message: finding.message,
                };
                match finding.severity {
                    cercis_core::Severity::Error => errors.push(issue),
                    cercis_core::Severity::Warning => warnings.push(issue),
                }
            }
        }
    }
    (errors, warnings)
}

/// AI 宿主的技能目录（技能包要装到这里才会被自动加载）。
fn skill_host_dir(host: &str) -> Result<PathBuf, CercisError> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| CercisError::Invalid("取不到 HOME，无法定位技能目录".into()))?;
    match host {
        "codex" => Ok(home.join(".codex").join("skills")),
        "claude" => Ok(home.join(".claude").join("skills")),
        other => Err(CercisError::Invalid(format!(
            "未知的宿主 {other}（可用：codex / claude；其他宿主请用 --out <目录>）"
        ))),
    }
}

/// 读取批量输入：CSV/TSV 文件、JSON 数组文件、`-`（标准输入读 JSON）或内联 JSON。
fn read_rows_input(from: &str) -> Result<Vec<Row>, CercisError> {
    if from == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        return parse_json_rows(&buf, "<stdin>");
    }

    let path = Path::new(from);
    if path.is_file() {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        return match ext.as_str() {
            "csv" => read_delimited_rows(path, b','),
            "tsv" => read_delimited_rows(path, b'\t'),
            _ => {
                let text = std::fs::read_to_string(path)?;
                parse_json_rows(&text, from)
            }
        };
    }

    let trimmed = from.trim_start();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return parse_json_rows(from, "<inline>");
    }
    Err(CercisError::NotFound(format!("输入文件不存在: {from}")))
}

/// 解析分隔符文本（首行为表头）。
fn read_delimited_rows(path: &Path, delimiter: u8) -> Result<Vec<Row>, CercisError> {
    let mut rdr = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .delimiter(delimiter)
        .from_path(path)?;

    let headers: Vec<String> = rdr
        .headers()?
        .iter()
        .map(|s| s.trim().to_string())
        .collect();
    if headers.is_empty() || headers.iter().any(|h| h.is_empty()) {
        return Err(CercisError::Invalid(format!(
            "{} 的表头为空或存在空列名",
            path.display()
        )));
    }

    let mut rows = Vec::new();
    for (i, rec) in rdr.records().enumerate() {
        let rec = rec.map_err(|e| {
            CercisError::Invalid(format!("{} 第 {} 行解析失败: {e}", path.display(), i + 2))
        })?;
        let mut row = Row::new();
        for (j, col) in headers.iter().enumerate() {
            row.insert(col.clone(), rec.get(j).unwrap_or("").to_string());
        }
        rows.push(row);
    }
    Ok(rows)
}

/// 解析 JSON：数组（每个元素是对象）或单个对象；标量一律转成字符串。
fn parse_json_rows(text: &str, source: &str) -> Result<Vec<Row>, CercisError> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| CercisError::Invalid(format!("{source} 不是合法 JSON: {e}")))?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        obj @ serde_json::Value::Object(_) => vec![obj],
        other => {
            return Err(CercisError::Invalid(format!(
                "{source} 需要 JSON 数组或对象，实际是 {other}"
            )))
        }
    };

    let mut rows = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let obj = item.as_object().ok_or_else(|| {
            CercisError::Invalid(format!("{source} 第 {} 项不是 JSON 对象", i + 1))
        })?;
        let mut row = Row::new();
        for (k, v) in obj {
            let cell = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            row.insert(k.clone(), cell);
        }
        rows.push(row);
    }
    Ok(rows)
}

fn hostname_or(fallback: &str) -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}
