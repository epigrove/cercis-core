//! 统一错误类型。所有可预期的失败都返回可读中文说明——AI 与人都要能看懂。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CercisError {
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("CSV 错误: {0}")]
    Csv(#[from] csv::Error),

    #[error("JSON 错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("YAML 错误: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("未找到: {0}")]
    NotFound(String),

    #[error("输入不合法: {0}")]
    Invalid(String),

    #[error("冲突: {0}")]
    Conflict(String),

    #[error("数据损坏: {0}")]
    Corrupt(String),

    #[error("不允许的操作: {0}")]
    Forbidden(String),
}

pub type Result<T> = std::result::Result<T, CercisError>;
