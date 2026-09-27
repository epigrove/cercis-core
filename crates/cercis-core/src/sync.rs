//! 与云盘 / Git 共存的检测（S13 / FR-I06）。
//!
//! 背景（需求 §12.1）：NAS 直连与"工作副本"是推荐通道；**云盘目录**（iCloud /
//! OneDrive / 坚果云 / 百度网盘……）属于"目录级同步"，能用但有风险——
//! 多端同时写会分叉，云盘自己还会留一份"某某 的冲突副本.csv"。
//!
//! 本模块只做**只读体检**：告诉你
//! 1. 这个项目是不是待在云盘同步目录里（是的话给三条注意事项）；
//! 2. 项目是不是在 Git 仓库里（Git 适合管工具与文档，不适合当数据同步通道）；
//! 3. 有没有**已经发生的分叉迹象**：云盘冲突副本、残留的写临时文件、`.icloud` 占位符、
//!    Office 的 `~$` 锁文件……有的话给出"用 `cercis merge` 收拢"的具体建议。
//!
//! 注意：这是**体检**，不改任何东西，也不替你删"冲突副本"——里面可能就有你要的数据。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;

/// 报告格式版本。
pub const SYNC_FORMAT: &str = "cercis-sync/1";

/// 一条发现。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncFinding {
    /// `cloud_copy` / `stale_tmp` / `icloud_placeholder` / `office_lock` / `other`
    pub kind: String,
    pub path: String,
    pub detail: String,
    pub advice: String,
}

/// Git 情况。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GitInfo {
    pub repo_root: String,
    pub branch: String,
    /// `git status --porcelain` 的非空行数（拿不到 git 时为 -1）
    pub dirty_files: i64,
}

/// 体检报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReport {
    pub format: String,
    pub path: String,
    /// 认出来的云盘（没认出来就是空）
    #[serde(default)]
    pub cloud: String,
    #[serde(default)]
    pub git: Option<GitInfo>,
    pub findings: Vec<SyncFinding>,
    /// `ok` / `attention` / `needs-merge`
    pub verdict: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl SyncReport {
    pub fn summary(&self) -> String {
        format!(
            "共存体检：{}（云盘 {}，Git {}，发现 {} 条）",
            match self.verdict.as_str() {
                "ok" => "一切正常",
                "attention" => "可以用，但有注意事项",
                _ => "发现分叉迹象，建议尽快收拢",
            },
            if self.cloud.is_empty() { "无" } else { &self.cloud },
            match &self.git {
                Some(g) => format!("在 {}（{}）", g.repo_root, g.branch),
                None => "无".to_string(),
            },
            self.findings.len()
        )
    }

    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str("# 云盘 / Git 共存体检\n\n");
        s.push_str(&format!("- 项目：`{}`\n", self.path));
        s.push_str(&format!("- 结论：{}\n", self.summary()));
        if !self.findings.is_empty() {
            s.push_str("\n## 发现\n\n");
            for f in &self.findings {
                s.push_str(&format!("- `{}`（{}）：{}\n  - 建议：{}\n", f.path, f.kind, f.detail, f.advice));
            }
        }
        if !self.notes.is_empty() {
            s.push_str("\n## 说明\n\n");
            for n in &self.notes {
                s.push_str(&format!("- {n}\n"));
            }
        }
        s
    }
}

/// 体检一个项目（只读）。
pub fn check(root: &Path) -> Result<SyncReport> {
    let abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let path_str = abs.to_string_lossy().to_string();
    let cloud = detect_cloud(&abs);
    let git = detect_git(&abs);
    let mut findings = Vec::new();
    let mut notes = Vec::new();

    // 1) 目录内容里的"分叉迹象"
    let mut stack = vec![abs.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let rel = p
                .strip_prefix(&abs)
                .map(|x| x.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| name.clone());
            if p.is_dir() {
                // 跳过工具自用与生成物
                if matches!(name.as_str(), ".git" | "target" | "node_modules" | "__pycache__") {
                    continue;
                }
                stack.push(p);
                continue;
            }
            if name.ends_with(".icloud") {
                findings.push(SyncFinding {
                    kind: "icloud_placeholder".into(),
                    path: rel.clone(),
                    detail: "iCloud 还没把这份文件下载到本机（只是个占位符）".into(),
                    advice: "先在 Finder 里右键「立即下载」，再跑一遍体检".into(),
                });
            } else if name.starts_with("~$") {
                findings.push(SyncFinding {
                    kind: "office_lock".into(),
                    path: rel.clone(),
                    detail: "Office 的临时锁文件（说明有人正打开这个文档）".into(),
                    advice: "关掉 Office 里的文档后删掉它；别在同步中编辑".into(),
                });
            } else if name.ends_with(crate::tx::TMP_SUFFIX) {
                findings.push(SyncFinding {
                    kind: "stale_tmp".into(),
                    path: rel.clone(),
                    detail: "上次写入被打断（断电或同步中拔盘）留下的临时文件".into(),
                    advice: "确认正式文件可读后删掉它；下次体检不该再出现".into(),
                });
            } else if looks_like_cloud_copy(&name) {
                findings.push(SyncFinding {
                    kind: "cloud_copy".into(),
                    path: rel.clone(),
                    detail: "云盘生成的「冲突副本」：两台设备同时改过同一份东西".into(),
                    advice:
                        "用底部建议核对内容后收拢：把两边要的都并进主文件（表格用 cercis merge，\
                         文档手工挑），然后删掉副本"
                            .into(),
                });
            }
        }
    }

    // 2) 云盘 / Git 的注意事项
    if !cloud.is_empty() {
        notes.push(format!(
            "项目在「{cloud}」同步目录里：① 同一时刻只在一台设备上写；② 运输期间另一端只读；\
             ③ 同步还没完成时别打开项目"
        ));
        notes.push("云盘同步的是**文件**，它不负责合并数据；真分叉了就用 cercis pack / merge 收拢".into());
    }
    if let Some(g) = &git {
        notes.push(format!(
            "项目在 Git 仓库 {}（分支 {}）里：Git 适合管工具、文档与脚本，**不适合当数据同步通道**\
             （二进制媒体会撑爆仓库、diff 也看不懂 CSV 的语义）",
            g.repo_root, g.branch
        ));
        if g.dirty_files > 0 {
            notes.push(format!(
                "Git 里有 {} 个未提交的改动——如果是数据表，请改用 cercis 的通道（数据只增不改、更正留痕）",
                g.dirty_files
            ));
        }
    }

    let has_conflict = findings
        .iter()
        .any(|f| f.kind == "cloud_copy" || f.kind == "stale_tmp");
    let verdict = if has_conflict {
        "needs-merge"
    } else if !cloud.is_empty() || git.is_some() || !findings.is_empty() {
        "attention"
    } else {
        "ok"
    };

    Ok(SyncReport {
        format: SYNC_FORMAT.to_string(),
        path: path_str,
        cloud,
        git,
        findings,
        verdict: verdict.to_string(),
        notes,
    })
}

/// 认一下路径是不是落在常见云盘目录里。
fn detect_cloud(abs: &Path) -> String {
    let s = abs.to_string_lossy().to_lowercase();
    let table: &[(&str, &str)] = &[
        ("com~apple~clouddocs", "iCloud 云盘"),
        ("library/mobile documents", "iCloud 云盘"),
        ("dropbox", "Dropbox"),
        ("onedrive", "OneDrive"),
        ("google drive", "Google Drive"),
        ("my drive", "Google Drive"),
        ("nutstore", "坚果云"),
        ("坚果云", "坚果云"),
        ("baidunetdisk", "百度网盘"),
        ("百度网盘", "百度网盘"),
        ("synologydrive", "Synology Drive"),
        ("box sync", "Box"),
        ("pcloud", "pCloud"),
    ];
    for (needle, label) in table {
        if s.contains(needle) {
            return (*label).to_string();
        }
    }
    String::new()
}

/// 往上找 `.git`；有 git 命令就顺手问一下分支与改动数。
fn detect_git(abs: &Path) -> Option<GitInfo> {
    let mut cur: Option<&Path> = Some(abs);
    let mut repo_root = None;
    while let Some(dir) = cur {
        if dir.join(".git").exists() {
            repo_root = Some(dir.to_path_buf());
            break;
        }
        cur = dir.parent();
    }
    let root = repo_root?;
    let mut branch = String::new();
    if let Ok(text) = fs::read_to_string(root.join(".git/HEAD")) {
        branch = text
            .trim()
            .strip_prefix("ref: refs/heads/")
            .unwrap_or(text.trim())
            .to_string();
    }
    let dirty_files = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .filter(|l| !l.trim().is_empty())
                .count() as i64
        })
        .unwrap_or(-1);
    Some(GitInfo {
        repo_root: root.display().to_string(),
        branch,
        dirty_files,
    })
}

/// 云盘/同步软件留下的"副本"命名习惯。
fn looks_like_cloud_copy(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.contains("conflict") || lower.contains("conflicted copy") {
        return true;
    }
    if name.contains("冲突") || name.contains("副本") || name.contains("的副本") {
        return true;
    }
    // `记录 (1).csv` / `记录(2).xlsx` 这类"序号副本"
    let re = regex::Regex::new(r"\((\d+)\)\.[A-Za-z0-9]+$").unwrap();
    re.is_match(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_cloud_copy_and_stale_tmp() {
        let dir = std::env::temp_dir().join("cercis-sync-test");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tables")).unwrap();
        fs::write(dir.join("project.yaml"), "schema_version: 1\nname: 试验\n").unwrap();
        fs::write(dir.join("tables/records.csv"), "record_id\nR-1\n").unwrap();
        fs::write(dir.join("tables/records (1).csv"), "record_id\nR-2\n").unwrap();
        fs::write(
            dir.join(format!("tables/notes.csv{}", crate::tx::TMP_SUFFIX)),
            "half written",
        )
        .unwrap();

        let rep = check(&dir).unwrap();
        assert_eq!(rep.verdict, "needs-merge");
        assert!(rep.findings.iter().any(|f| f.kind == "cloud_copy"));
        assert!(rep.findings.iter().any(|f| f.kind == "stale_tmp"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clean_project_is_ok() {
        let dir = std::env::temp_dir().join("cercis-sync-test-clean");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tables")).unwrap();
        fs::write(dir.join("project.yaml"), "schema_version: 1\nname: 试验\n").unwrap();
        fs::write(dir.join("tables/records.csv"), "record_id\nR-1\n").unwrap();
        let rep = check(&dir).unwrap();
        // 临时目录本身不在云盘里、也不是 git 仓库 → 一切正常
        assert_eq!(rep.verdict, "ok", "{rep:?}");
        assert!(rep.findings.is_empty());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cloud_paths_are_recognized() {
        assert_eq!(
            detect_cloud(Path::new("/Users/x/Library/Mobile Documents/com~apple~CloudDocs/项目")),
            "iCloud 云盘"
        );
        assert_eq!(detect_cloud(Path::new("/Users/x/Dropbox/项目")), "Dropbox");
        assert_eq!(detect_cloud(Path::new("/Users/x/PycharmProjects/Cercis")), "");
    }
}
