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
//! ## 尚未实现（后续阶段）
//! - S2：完整的表结构定义与约束、查询原语（组合筛选/排序/分页）
//! - S3：机器清单完善、插件注册协议、UI 扩展点协议
//!
//! ## 设计红线
//! 1. 数据只增不改：更正写 `corrections.csv`，原值由历史对象永久保留。
//! 2. 写入必须原子：任何时刻断电，磁盘上的文件要么是旧内容、要么是新内容。
//! 3. 可移植：不用本工具，仅靠 CSV/媒体文件也能理解并使用数据。

pub mod config;
pub mod error;
pub mod format;
pub mod history;
pub mod ids;
pub mod project;
pub mod store;
pub mod table;
pub mod tx;

/// 内核版本（来自 Cargo.toml）。
pub const CORE_VERSION: &str = env!("CARGO_PKG_VERSION");
/// 项目结构版本：升级时用于迁移。
pub const SCHEMA_VERSION: u32 = 1;

pub use config::ProjectConfig;
pub use error::{CercisError, Result};
pub use project::Project;
pub use store::{Identity, Store};
pub use table::Table;
