//! 原子写入：先写同目录临时文件 → fsync → 原子改名。
//!
//! 目的：**任何时刻断电或 NAS 断线，目标文件要么是旧内容，要么是新内容**，
//! 不会出现“写了一半”的 CSV。

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::Result;

/// 临时文件后缀（合并/体检时可识别并清理残留）。
pub const TMP_SUFFIX: &str = ".cercis-tmp";

/// 以原子方式写入字节。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = tmp_path(path);

    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }

    // 同目录改名在同一文件系统内是原子操作。
    // 改名失败是**网络盘/云盘上最常见的坑**（不支持原子改名，例如某些 WebDAV 挂载）：
    // 这里把"怎么办"直接写进错误里，别让用户对着 Input/output error 发呆。
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(crate::CercisError::Invalid(format!(
            "写入失败：临时文件改名不到位（{} → {}）：{e}\n\
             - 如果这是在网络盘 / 云盘 / WebDAV / NAS 上：先体检 `cercis remote test <那个位置>`；\n\
             - 体检说「不支持原子改名」时，不要直接在上面编辑，改用工作副本：\n\
               cercis workcopy create <主副本> --to <本地目录>（本地照记，联网后 workcopy sync）\n\
             - 如果只是权限问题：确认那个目录可写（同目录需要有创建与改名权限）",
            tmp.display(),
            path.display()
        )));
    }

    // 尽力而为地同步目录项（部分平台不支持，忽略错误）。
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// 以原子方式写入文本（UTF-8）。
pub fn write_atomic_str(path: &Path, text: &str) -> Result<()> {
    write_atomic(path, text.as_bytes())
}

/// 读取文本；文件不存在时返回 `Ok(None)`。
pub fn read_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// 生成同目录临时文件路径。
pub fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    name.push_str(TMP_SUFFIX);
    path.with_file_name(name)
}

/// 清理遗留的临时文件（崩溃/断电后可能残留）。
pub fn clean_stale_tmp(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut removed = Vec::new();
    if !dir.exists() {
        return Ok(removed);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let p = entry.path();
        if p.to_string_lossy().ends_with(TMP_SUFFIX) {
            fs::remove_file(&p)?;
            removed.push(p);
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_and_read() {
        let dir = std::env::temp_dir().join("cercis-tx-test");
        let _ = fs::remove_dir_all(&dir);
        let f = dir.join("a.csv");
        write_atomic_str(&f, "h1,h2\n1,2\n").unwrap();
        assert_eq!(read_optional(&f).unwrap().unwrap(), "h1,h2\n1,2\n");
        // 覆盖写
        write_atomic_str(&f, "x\n").unwrap();
        assert_eq!(read_optional(&f).unwrap().unwrap(), "x\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
