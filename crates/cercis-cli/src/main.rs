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
    /// 创建检查点（文件级备份）
    Checkpoint(CheckpointArgs),
    /// 还原某个历史对象到文件（回滚 = 新变更）
    Restore(RestoreArgs),
    /// 查看项目模板（表结构与字段定义）
    Template(TemplateArgs),
    /// 装入模板（示例研究 / 通用）
    InstallTemplate(InstallTemplateArgs),
    /// 按条件查询某张表（组合筛选 + 排序）
    Query(QueryArgs),
    /// 校验项目（引用完整性 + 字段规则）
    Validate(ProjectArg),
    /// 刷新机器清单与 AI 材料（AGENTS.md / CLAUDE.md / ai_contract.json）
    Agents(ProjectArg),
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
}

#[derive(Args, Debug)]
struct InstallTemplateArgs {
    path: PathBuf,
    /// 模板名：generic / example-study
    #[arg(long)]
    name: String,
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
            let t = p.template()?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "template": t,
                }));
            } else {
                println!("模板：{}（{}）", t.name, t.label_cn);
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
            let t = match a.name.as_str() {
                "generic" => cercis_core::Template::generic(),
                "example-study" => cercis_core::Template::example_study(),
                other => {
                    return Err(CercisError::Invalid(format!(
                        "未知模板 {other}（可用：generic / example-study）"
                    )))
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
                if created.is_empty() {
                    println!("（表已存在，未新建）");
                } else {
                    println!("新建表：{}", created.join(", "));
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
            // 若表不存在，按本次提供的列创建（列名按字母序稳定）
            let owned: Vec<String> = row.keys().cloned().collect();
            let cols: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();
            p.store
                .append_row(&a.table, &cols, row, &a.target, &a.summary)?;
            if cli.json {
                print_json(&serde_json::json!({
                    "schema": "cercis.cli/1",
                    "ok": true,
                    "table": a.table,
                    "target": a.target,
                }));
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

            if a.dry_run {
                if cli.json {
                    print_json(&serde_json::json!({
                        "schema": "cercis.cli/1",
                        "ok": true,
                        "dry_run": true,
                        "table": a.table,
                        "rows": rows.len(),
                        "columns": columns,
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
                    println!("（未写入任何数据；去掉 --dry-run 即正式导入）");
                }
                return Ok(());
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
