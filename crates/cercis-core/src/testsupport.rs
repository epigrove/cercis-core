//! 测试用的小工具（只在 `cargo test` 时编译）。
//!
//! ## 为什么要有它
//! Rust 的测试**默认并行**跑在同一个进程里，而 `std::env::set_var` 是**进程级**的。
//! 早先的写法是"每个测试自己 set、跑完 remove"，结果：
//! A 测试正在用 `CERCIS_SHARE_HOME`，B 测试跑完把它 `remove` 了 →
//! A 掉到真实的 `$HOME` 上（要么写不动，要么污染真实目录），表现为**随机失败**。
//!
//! 现在统一成"进程内只初始化一次、谁都不许删"。

use std::path::PathBuf;
use std::sync::OnceLock;

/// 加密共享相关测试共用的"用户目录"（会话、钥匙串账号都落这里）。
pub fn share_home() -> PathBuf {
    static HOME: OnceLock<PathBuf> = OnceLock::new();
    HOME.get_or_init(|| {
        let home = std::env::temp_dir().join("cercis-core-tests-share-home");
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::create_dir_all(&home);
        std::env::set_var("CERCIS_SHARE_HOME", &home);
        // 口令派生走轻量档：测试要快，真实使用仍走默认强度
        std::env::set_var("CERCIS_SHARE_KDF", "light");
        home
    })
    .clone()
}

/// 关掉系统钥匙串（测试里不碰真实钥匙串）。
pub fn keychain_off() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("CERCIS_KEYCHAIN", "off");
    });
}
