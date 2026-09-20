#![forbid(unsafe_code)]
//! # Cercis 内核（cercis-core）
//!
//! 定位：**行业普适的“文件夹即数据库”引擎**，只做两件核心的事——
//! **数据存取** 与 **AI 友好**。它不认识任何学科概念（研究、样本、观测……），
//! 只认识表、记录、字段、文件、历史；字段语义由插件模板提供。
//!
//! ## S1（本阶段）范围
//! - 项目创建与打开（`Project`）
//! - 表读写（`Table`：CSV，UTF-8 无 BOM，LF 换行）
//! - 事务写入（`tx`：先写临时文件再改名，断电不产生半写文件）
//! - 编号与格式规则（`ids`、`format`）
//! - 逐次存档与检查点（`history`：内容寻址对象库 + 文件级检查点）
//!
//! ## S2 范围（已完成）
//! - 表结构定义机制（`template`：字段语义由模板提供，内核不含学科词）
//! - 查询原语（`query`：组合筛选、排序、分页，确定性输出）
//! - 引用完整性与字段校验（`validate`：结构错误阻止、业务规则警告）
//!
//! ## S3 范围（本阶段）
//! - 机器清单（`manifest`：AI 与外部工具进入项目的入口）
//! - 插件协议（`plugin`：清单、权限模型、界面扩展点、信任与指纹）
//! - AI 契约（`ai`：AGENTS.md / CLAUDE.md / ai_contract.json 自动生成）
//!
//! ## 尚未实现（后续阶段）
//! - S4：app 桌面界面（主界面、插件总管、扩展点渲染）
//!
//! ## 设计红线
//! 1. 数据只增不改：更正写 `corrections.csv`，原值由历史对象永久保留。
//! 2. 写入必须原子：任何时刻断电，磁盘上的文件要么是旧内容、要么是新内容。
//! 3. 可移植：不用本工具，仅靠 CSV/媒体文件也能理解并使用数据。

pub mod ai;
pub mod config;
pub mod error;
pub mod format;
pub mod history;
pub mod ids;
pub mod manifest;
pub mod plugin;
pub mod project;
pub mod query;
pub mod store;
pub mod table;
pub mod template;
pub mod tx;
pub mod validate;

/// 内核版本（来自 Cargo.toml）。
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 项目结构版本：升级时用于迁移。
pub const SCHEMA_VERSION: u32 = 1;

pub use ai::AiContract;
pub use config::ProjectConfig;
pub use error::{CercisError, Result};
pub use manifest::Manifest;
pub use plugin::{DiscoveredPlugin, PluginManifest, TrustRecord, PLUGIN_API_VERSION};
pub use project::Project;
pub use query::{Condition, Op, Query, SortDir, SortKey};
pub use store::{Identity, Store};
pub use table::Table;
pub use template::Template;
pub use validate::{validate_project, Issue, Report};
