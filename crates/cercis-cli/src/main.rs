//! Cercis 命令行（`cercis`）：内核能力的第一个使用者。
//!
//! 设计目标：**人能用、脚本能用、AI 能用**。
//! - 所有命令支持 `--json`（稳定输出，带 schema 版本）
//! - 只读默认；写入命令明确列出，且全部经由内核通道留痕

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

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
    /// 记录一条更正（写入；不覆盖原值）
    Correct(CorrectArgs),
    /// 查看变更流水
    Log(LogArgs),
    /// 创建检查点（文件级备份）
    Checkpoint(CheckpointArgs),
    /// 还原某个历史对象到文件（回滚 = 新变更）
    Restore(RestoreArgs),
    /// 内核信息
    Version,
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

fn hostname_or(fallback: &str) -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}
