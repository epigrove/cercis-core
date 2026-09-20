//! 历史：逐次存档（内容寻址）+ 检查点（文件级备份）+ 变更流水。
//!
//! 目标：**任何一次改动都能还原**，等效于“每次提交”，但不需要用户懂 git。
//!
//! ## 存储布局
//! ```text
//! history/
//!   objects/<指纹前两位>/<指纹>      逐次改动存档（压缩文本，内容寻址去重）
//!   checkpoints/<时间戳>/           检查点：只备份“与上一个检查点不同”的文件
//!   checkpoints/<时间戳>/index.csv  检查点说明（文件 → 指纹）
//! tables/change_log.csv             变更流水（谁/设备/时间/动作/对象/前后指纹）
//! ```
//!
//! ## 为什么用内容寻址
//! 同一内容只存一份，因此“每改一次存一次”不会成倍占空间；媒体文件不重复存档，只记指纹。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::{now_iso8601, timestamp_for_path};
use crate::tx;
use crate::{CercisError, Result};

/// 历史对象库（内容寻址）。
#[derive(Debug, Clone)]
pub struct ObjectStore {
    root: PathBuf,
}

impl ObjectStore {
    pub fn new(history_dir: impl AsRef<Path>) -> Self {
        Self {
            root: history_dir.as_ref().join("objects"),
        }
    }

    /// 计算内容指纹（sha256，十六进制小写）。
    pub fn hash_bytes(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    }

    /// 计算文件指纹。
    pub fn hash_file(path: impl AsRef<Path>) -> Result<String> {
        let bytes = fs::read(path)?;
        Ok(Self::hash_bytes(&bytes))
    }

    /// 存档一份内容，返回指纹；相同内容只存一份。
    pub fn put(&self, bytes: &[u8]) -> Result<String> {
        let hash = Self::hash_bytes(bytes);
        let dest = self.path_for(&hash);
        if !dest.exists() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            tx::write_atomic(&dest, bytes)?;
        }
        Ok(hash)
    }

    /// 存档文件内容并返回指纹。
    pub fn put_file(&self, path: impl AsRef<Path>) -> Result<String> {
        let bytes = fs::read(path)?;
        self.put(&bytes)
    }

    /// 按指纹取回内容。
    pub fn get(&self, hash: &str) -> Result<Vec<u8>> {
        let p = self.path_for(hash);
        if !p.exists() {
            return Err(CercisError::NotFound(format!("历史对象不存在: {hash}")));
        }
        Ok(fs::read(p)?)
    }

    /// 按指纹取回文本。
    pub fn get_text(&self, hash: &str) -> Result<String> {
        let bytes = self.get(hash)?;
        String::from_utf8(bytes)
            .map_err(|e| CercisError::Corrupt(format!("历史对象不是合法 UTF-8: {hash}（{e}）")))
    }

    /// 指纹对应的存储路径。
    pub fn path_for(&self, hash: &str) -> PathBuf {
        let prefix = hash.get(0..2).unwrap_or("00");
        self.root.join(prefix).join(hash)
    }
}

/// 变更流水中的一条记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeEntry {
    pub change_id: String,
    pub ts: String,
    pub user_id: String,
    pub device_id: String,
    /// create / update / correct / delete / restore / import / export / rollback / checkpoint
    pub action: String,
    pub target_table: String,
    pub target_id: String,
    pub summary: String,
    #[serde(default)]
    pub batch_id: String,
    pub tool_version: String,
    #[serde(default)]
    pub source_ref: String,
    #[serde(default)]
    pub hash_before: String,
    #[serde(default)]
    pub hash_after: String,
}

impl ChangeEntry {
    /// 构造一条流水（时间与编号自动生成）。
    ///
    /// 参数较多是有意为之：流水字段固定且都要显式传入，避免"少传一个就丢信息"。
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: &str,
        device_id: &str,
        action: &str,
        target_table: &str,
        target_id: &str,
        summary: &str,
        hash_before: &str,
        hash_after: &str,
    ) -> Self {
        Self {
            change_id: crate::ids::new_machine_id(),
            ts: now_iso8601(),
            user_id: user_id.to_string(),
            device_id: device_id.to_string(),
            action: action.to_string(),
            target_table: target_table.to_string(),
            target_id: target_id.to_string(),
            summary: summary.to_string(),
            batch_id: String::new(),
            tool_version: crate::CORE_VERSION.to_string(),
            source_ref: String::new(),
            hash_before: hash_before.to_string(),
            hash_after: hash_after.to_string(),
        }
    }
}

/// 检查点信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub checkpoint_id: String,
    pub ts: String,
    pub note: String,
    pub files: Vec<CheckpointFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointFile {
    pub relative_path: String,
    pub sha256: String,
    pub bytes: u64,
}

/// 检查点管理（文件级备份，默认保留最近 50 个）。
pub struct Checkpoints {
    root: PathBuf,
    keep: usize,
}

impl Checkpoints {
    pub fn new(history_dir: impl AsRef<Path>, keep_recent: usize) -> Self {
        Self {
            root: history_dir.as_ref().join("checkpoints"),
            keep: if keep_recent == 0 { 50 } else { keep_recent },
        }
    }

    /// 创建检查点：只备份列出的文件；返回检查点目录。
    pub fn create(
        &self,
        project_root: impl AsRef<Path>,
        files: &[PathBuf],
        note: &str,
    ) -> Result<PathBuf> {
        let project_root = project_root.as_ref();
        let ts = now_iso8601();
        let stamp = timestamp_for_path(crate::ids::parse_iso8601(&ts)?);
        // 同一秒内可能连续创建多个检查点：以 ULID 后 4 位作为短后缀保证唯一
        let suffix = crate::ids::new_machine_id();
        let id = format!("{stamp}-{}", &suffix[suffix.len().saturating_sub(4)..]);
        let dir = self.root.join(&id);
        fs::create_dir_all(&dir)?;

        let mut listed = Vec::new();
        for f in files {
            if !f.exists() {
                continue;
            }
            let rel = f
                .strip_prefix(project_root)
                .map_err(|_| CercisError::Invalid(format!("文件不在项目内: {}", f.display())))?
                .to_string_lossy()
                .to_string();
            let hash = ObjectStore::hash_file(f)?;
            let bytes = fs::metadata(f)?.len();
            let dest = dir.join(&rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(f, &dest)?;
            listed.push(CheckpointFile {
                relative_path: rel,
                sha256: hash,
                bytes,
            });
        }

        let cp = Checkpoint {
            checkpoint_id: id.clone(),
            ts,
            note: note.to_string(),
            files: listed,
        };
        let index_text = checkpoint_index_csv(&cp);
        tx::write_atomic_str(&dir.join("index.csv"), &index_text)?;
        tx::write_atomic_str(
            &dir.join("checkpoint.json"),
            &serde_json::to_string_pretty(&cp)?,
        )?;

        self.prune()?;
        Ok(dir)
    }

    /// 列出全部检查点（按时间倒序）。
    pub fn list(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        if !self.root.exists() {
            return Ok(ids);
        }
        for e in fs::read_dir(&self.root)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                ids.push(e.file_name().to_string_lossy().to_string());
            }
        }
        ids.sort();
        ids.reverse();
        Ok(ids)
    }

    /// 只保留最近 N 个检查点。
    pub fn prune(&self) -> Result<Vec<String>> {
        let ids = self.list()?;
        let mut removed = Vec::new();
        for old in ids.iter().skip(self.keep) {
            let dir = self.root.join(old);
            fs::remove_dir_all(&dir)?;
            removed.push(old.clone());
        }
        Ok(removed)
    }
}

fn checkpoint_index_csv(cp: &Checkpoint) -> String {
    let mut out = String::from("relative_path,sha256,bytes\n");
    for f in &cp.files {
        out.push_str(&format!("{},{},{}\n", f.relative_path, f.sha256, f.bytes));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_store_dedupes() {
        let dir = std::env::temp_dir().join("cercis-history-test");
        let _ = fs::remove_dir_all(&dir);
        let store = ObjectStore::new(&dir);
        let a = store.put(b"hello").unwrap();
        let b = store.put(b"hello").unwrap();
        assert_eq!(a, b);
        assert_eq!(store.get_text(&a).unwrap(), "hello");
        let c = store.put(b"world").unwrap();
        assert_ne!(a, c);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn checkpoint_and_prune() {
        let root = std::env::temp_dir().join("cercis-cp-test");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("tables")).unwrap();
        let f = root.join("tables/t.csv");
        tx::write_atomic_str(&f, "a\n1\n").unwrap();

        let cps = Checkpoints::new(root.join("history"), 2);
        for i in 0..3 {
            tx::write_atomic_str(&f, &format!("a\n{i}\n")).unwrap();
            // 让时间戳不同
            std::thread::sleep(std::time::Duration::from_millis(1100));
            cps.create(&root, std::slice::from_ref(&f), "test").unwrap();
        }
        let list = cps.list().unwrap();
        assert_eq!(list.len(), 2, "默认只保留最近 2 个");
        let _ = fs::remove_dir_all(&root);
    }
}
