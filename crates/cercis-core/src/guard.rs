//! 分享守卫（S12 / FR-I04–FR-I05，红线：**发布零数据**）。
//!
//! 用途：在把任何东西发出去（GitHub、同行、投稿附件）之前先扫一遍，
//! 回答三个问题：
//! 1. 这个目录能不能当作分享包/发布物？（白名单 + 真实数据拦截）
//! 2. 逐文件清单：每个文件是放行、要人工确认、还是拒绝，以及为什么
//! 3. 里面的文字有没有隐私线索（邮箱、手机号、身份证号、经纬度、自定义关键词）
//!
//! 设计取向：**宁严勿宽**——不认识的东西一律标"需人工确认"，像项目数据的
//! 一律拒绝；只有"合成示例"（项目里有 `SYNTHETIC.md` 标记）才放行数据类文件。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::crypto;
use crate::{CercisError, Result};

/// 报告格式版本。
pub const GUARD_FORMAT: &str = "cercis-guard/1";
/// 合成示例的标记文件名（放在项目根；内容写清"全部为虚构数据"）。
pub const SYNTHETIC_MARKER: &str = "SYNTHETIC.md";

/// 一句话放行：这些扩展名是工具自己的文档/代码/空白模板。
const ALLOW_EXT: &[&str] = &[
    "md", "txt", "rs", "py", "js", "mjs", "ts", "tsx", "jsx", "html", "css", "scss", "json",
    "yaml", "yml", "toml", "sh", "bat", "ps1", "sql", "lock", "cfg", "ini", "svg", "xml",
    "gitignore", "editorconfig",
];
/// 直接拒绝：真实数据的典型位置与形态。
const REFUSE_PREFIXES: &[&str] = &[
    "tables/",
    "history/",
    "access/",
    "_labnote/",
    "import/",
    "exports/",
    "media/",
    "methods/",
];
/// 直接拒绝：媒体与文档类（可能有真人、真图、真配方）。
const REFUSE_EXT: &[&str] = &[
    "csv", "tsv", "xlsx", "xls", "docx", "doc", "pdf", "pptx", "ppt", "jpg", "jpeg", "png",
    "heic", "tif", "tiff", "bmp", "gif", "webp", "mp3", "m4a", "wav", "aac", "mp4", "mov",
    "avi", "zip", "7z", "rar", "tar", "gz", "key", "pem", "p12", "pfx", "sqlite", "db",
];
/// 任何情况下都不该出门的目录/文件名。
/// 没有扩展名但确定是工具自身文档的常见文件名。
const ALLOW_NAMES: &[&str] = &[
    "LICENSE",
    "NOTICE",
    "COPYING",
    "README",
    "CHANGELOG",
    "Makefile",
    "Dockerfile",
    "AGENTS",
    "CLAUDE",
];

const NEVER_DIRS: &[&str] = &[
    ".git",
    "target",
    "node_modules",
    ".idea",
    ".vscode",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
];
const NEVER: &[&str] = &[
    ".DS_Store",
    ".env",
    ".cercis-tmp",
    ".session.json",
];

/// 一个文件的判定。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardFile {
    pub path: String,
    pub bytes: u64,
    /// `allow` / `review` / `refuse` / `skip`
    pub class: String,
    pub reason: String,
}

/// 隐私线索。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyHit {
    pub path: String,
    /// email / phone / id_card / geo / keyword
    pub kind: String,
    pub sample: String,
}

/// 扫描报告。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardReport {
    pub format: String,
    pub path: String,
    /// 是否带"合成示例"标记
    pub synthetic: bool,
    /// `ok` / `review` / `refuse`
    pub verdict: String,
    pub files: Vec<GuardFile>,
    pub allowed: usize,
    pub to_review: usize,
    pub refused: usize,
    pub skipped: usize,
    pub privacy: Vec<PrivacyHit>,
    pub notes: Vec<String>,
}

impl GuardReport {
    pub fn is_shareable(&self) -> bool {
        self.verdict == "ok" || self.verdict == "review"
    }

    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str("# 分享守卫报告\n\n");
        s.push_str(&format!("- 扫描：`{}`\n", self.path));
        s.push_str(&format!(
            "- 结论：**{}**\n",
            match self.verdict.as_str() {
                "ok" => "可以分享（白名单内，未发现真实数据）",
                "review" => "需要人工确认（有数据类文件，但都带合成标记或已手动放行）",
                _ => "不能分享（发现真实数据类文件）",
            }
        ));
        s.push_str(&format!(
            "- 文件：放行 {} / 待确认 {} / 拒绝 {} / 跳过 {}\n",
            self.allowed, self.to_review, self.refused, self.skipped
        ));
        s.push_str("\n## 逐文件清单\n\n");
        for f in &self.files {
            s.push_str(&format!(
                "- `{}`（{} 字节）→ **{}**：{}\n",
                f.path, f.bytes, f.class, f.reason
            ));
        }
        if !self.privacy.is_empty() {
            s.push_str("\n## 隐私线索（请人工过目）\n\n");
            for p in &self.privacy {
                s.push_str(&format!("- `{}`：{} → `{}`\n", p.path, p.kind, p.sample));
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

/// 扫描选项。
#[derive(Debug, Clone)]
pub struct GuardOptions {
    /// 手动放行的 glob（如 `examples/**`）
    pub allow: Vec<String>,
    /// 自定义敏感关键词（如"配方、专利"）
    pub keywords: Vec<String>,
    /// 是否做隐私文本扫描（默认做）
    pub scan_privacy: bool,
}

impl Default for GuardOptions {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            keywords: Vec::new(),
            scan_privacy: true, // 默认就扫：隐私线索宁多报不漏报
        }
    }
}

/// 扫描一个目录（只读，不改任何东西）。
pub fn scan(root: &Path, opts: &GuardOptions) -> Result<GuardReport> {
    if !root.exists() {
        return Err(CercisError::NotFound(format!("路径不存在：{}", root.display())));
    }
    let synthetic = root.join(SYNTHETIC_MARKER).exists()
        || crate::tx::read_optional(&root.join("_labnote/synthetic.json"))
            .ok()
            .flatten()
            .map(|t| t.contains("\"synthetic\": true") || t.contains("\"synthetic\":true"))
            .unwrap_or(false);

    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let mut privacy = Vec::new();
    let mut counts = (0usize, 0usize, 0usize, 0usize); // allow, review, refuse, skip

    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p
                .strip_prefix(root)
                .map(|x| x.to_string_lossy().replace('\\', "/"))
                .unwrap_or_default();
            if p.is_dir() {
                let dir_name = p.file_name().map(|n| n.to_string_lossy().to_string());
                let name = dir_name.unwrap_or_default();
                // 生成物与工具自用目录（不论在哪一层）：不进清单也不进包
                if NEVER_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(p);
                continue;
            }
            let size = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let class = classify(&rel, synthetic, opts);
            let (cls, reason) = class;
            match cls {
                "allow" => counts.0 += 1,
                "review" => counts.1 += 1,
                "refuse" => counts.2 += 1,
                _ => counts.3 += 1,
            }
            if opts.scan_privacy && cls != "refuse" && is_texty(&rel) && size < 2 * 1024 * 1024 {
                if let Ok(text) = fs::read_to_string(&p) {
                    privacy.extend(scan_text(&rel, &text, &opts.keywords));
                }
            }
            files.push(GuardFile {
                path: rel,
                bytes: size,
                class: cls.to_string(),
                reason,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));

    let verdict = if counts.2 > 0 {
        "refuse"
    } else if counts.1 > 0 || !privacy.is_empty() {
        "review"
    } else {
        "ok"
    };
    let mut notes = Vec::new();
    if synthetic {
        notes.push(format!(
            "发现合成示例标记（SYNTHETIC.md）：数据类文件按「虚构数据」放行，但仍请人工过目一遍"
        ));
    } else {
        notes.push(
            "没有合成示例标记：数据类文件（CSV/媒体/历史/加密声明）一律按真实数据拒绝".to_string(),
        );
    }
    if files.iter().any(|f| {
        f.class != "refuse"
            && matches!(
                f.path.rsplit('.').next().unwrap_or(""),
                "jpg" | "jpeg" | "png" | "heic" | "tif" | "tiff" | "mp4" | "mov"
            )
    }) {
        notes.push("清单里有图片/视频：可能拍到人脸或车牌，请逐张确认（本工具不做人脸识别）".to_string());
    }

    Ok(GuardReport {
        format: GUARD_FORMAT.to_string(),
        path: root.display().to_string(),
        synthetic,
        verdict: verdict.to_string(),
        files,
        allowed: counts.0,
        to_review: counts.1,
        refused: counts.2,
        skipped: counts.3,
        privacy,
        notes,
    })
}

fn classify<'a>(rel: &str, synthetic: bool, opts: &'a GuardOptions) -> (&'a str, String) {
    if NEVER
        .iter()
        .any(|n| rel.ends_with(n) || rel.rsplit('/').next() == Some(*n))
    {
        return ("skip", "本工具自身的目录/文件，不进分享包".to_string());
    }
    if opts.allow.iter().any(|g| crate::access::matches_path(g, rel)) {
        return ("allow", "手动放行（--allow）".to_string());
    }
    let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    let base = rel.rsplit('/').next().unwrap_or(rel);
    if ALLOW_NAMES.contains(&base) {
        return ("allow", "工具自身的说明/许可文件".to_string());
    }
    let in_data_area = REFUSE_PREFIXES.iter().any(|p| rel.starts_with(p));
    let data_ext = REFUSE_EXT.contains(&ext.as_str());
    let looks_like_project = rel == "project.yaml" || rel == "数据字典.csv";

    if synthetic {
        // 合成示例：数据类文件放行，但要人过目
        if in_data_area || data_ext || looks_like_project {
            return ("review", "合成示例里的数据类文件（请人工确认确实是虚构数据）".to_string());
        }
        if ALLOW_EXT.contains(&ext.as_str()) || ext.is_empty() {
            return ("allow", "工具文档/代码/空白模板".to_string());
        }
        return ("review", "不认识的类型，请人工确认".to_string());
    }

    // 普通目录（工具仓库、分享目录……）
    if looks_like_project || in_data_area {
        return (
            "refuse",
            "像真实项目数据（project.yaml / tables / history / access / media / methods）：\
             分享包必须只含工具文档、空白模板与合成示例"
                .to_string(),
        );
    }
    if data_ext {
        return (
            "refuse",
            format!("数据/媒体/压缩文件（.{ext}）：可能是真实记录，不能直接进分享包"),
        );
    }
    if ALLOW_EXT.contains(&ext.as_str()) || ext.is_empty() {
        return ("allow", "工具文档/代码/空白模板".to_string());
    }
    ("review", "不认识的类型，请人工确认".to_string())
}

fn is_texty(rel: &str) -> bool {
    let ext = rel.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(
        ext.as_str(),
        "md" | "txt" | "rs" | "py" | "js" | "ts" | "html" | "css" | "json" | "yaml" | "yml"
            | "toml" | "sh" | "sql" | "csv" | "tsv"
    )
}

/// 文本里的隐私线索（启发式，只提示不判罪）。
fn scan_text(rel: &str, text: &str, keywords: &[String]) -> Vec<PrivacyHit> {
    let mut hits = Vec::new();
    let email = regex::Regex::new(r"[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}").unwrap();
    let phone = regex::Regex::new(r"\b1[3-9]\d{9}\b").unwrap();
    let idcard = regex::Regex::new(r"\b\d{17}[\dXx]\b").unwrap();
    let geo = regex::Regex::new(r"(经度|纬度|latitude|longitude)[^\n]{0,20}-?\d{1,3}\.\d+").unwrap();

    for (kind, re) in [
        ("email", &email),
        ("phone", &phone),
        ("id_card", &idcard),
        ("geo", &geo),
    ] {
        for m in re.find_iter(text).take(5) {
            hits.push(PrivacyHit {
                path: rel.to_string(),
                kind: kind.to_string(),
                sample: m.as_str().to_string(),
            });
        }
    }
    for kw in keywords {
        let kw = kw.trim();
        if kw.is_empty() {
            continue;
        }
        if let Some(pos) = text.find(kw) {
            let end = (pos + kw.len() + 12).min(text.len());
            hits.push(PrivacyHit {
                path: rel.to_string(),
                kind: "keyword".to_string(),
                sample: text[pos..end].replace('\n', " "),
            });
        }
    }
    hits
}

/// 按白名单把目录打成分享包（拒绝项存在时直接不干）。
pub fn pack_share(root: &Path, out: &Path, opts: &GuardOptions) -> Result<GuardReport> {
    let mut report = scan(root, opts)?;
    if !report.is_shareable() {
        let bad: Vec<String> = report
            .files
            .iter()
            .filter(|f| f.class == "refuse")
            .map(|f| format!("{}（{}）", f.path, f.reason))
            .collect();
        return Err(CercisError::Invalid(format!(
            "不能作为分享包：发现 {} 个真实数据类文件，已拒绝打包。\n- {}",
            bad.len(),
            bad.join("\n- ")
        )));
    }
    fs::create_dir_all(out)?;
    let mut copied = 0usize;
    let mut manifest = String::from("{\n  \"format\": \"cercis-share/1\",\n  \"files\": [\n");
    let mut first = true;
    for f in report.files.iter().filter(|f| f.class != "refuse" && f.class != "skip") {
        let src = root.join(&f.path);
        let bytes = fs::read(&src)?;
        let dest = out.join(&f.path);
        crate::tx::write_atomic(&dest, &bytes)?;
        let sha = crypto::sha256_hex(&bytes);
        if !first {
            manifest.push_str(",\n");
        }
        first = false;
        manifest.push_str(&format!(
            "    {{\"path\": {}, \"bytes\": {}, \"sha256\": \"{}\", \"class\": \"{}\"}}",
            serde_json::to_string(&f.path)?,
            bytes.len(),
            sha,
            f.class
        ));
        copied += 1;
    }
    manifest.push_str("\n  ]\n}\n");
    crate::tx::write_atomic_str(&out.join("share-manifest.json"), &manifest)?;
    crate::tx::write_atomic_str(&out.join("SHARE-README.md"), &report.to_markdown())?;
    report
        .notes
        .push(format!("已写出分享包：{copied} 个文件 → {}", out.display()));
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_real_project_data() {
        let dir = std::env::temp_dir().join("cercis-guard-test-project");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tables")).unwrap();
        fs::write(dir.join("project.yaml"), "schema_version: 1\nname: 真实项目\n").unwrap();
        fs::write(dir.join("tables/records.csv"), "record_id\nR-1\n").unwrap();
        fs::write(dir.join("README.md"), "# 说明\n").unwrap();

        let rep = scan(&dir, &GuardOptions::default()).unwrap();
        assert_eq!(rep.verdict, "refuse");
        assert!(rep.refused >= 2, "project.yaml 与 CSV 都该被拒");
        assert!(pack_share(&dir, &dir.join("out"), &GuardOptions::default()).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn allows_tool_docs_and_flags_privacy() {
        let dir = std::env::temp_dir().join("cercis-guard-test-docs");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("README.md"), "联系：someone@example.com\n").unwrap();
        fs::write(dir.join("src/lib.rs"), "pub fn hi() {}\n").unwrap();
        fs::write(dir.join("LICENSE"), "Apache-2.0\n").unwrap();

        let rep = scan(&dir, &GuardOptions::default()).unwrap();
        assert_eq!(rep.verdict, "review", "有隐私线索 → 要人工确认");
        assert!(rep.refused == 0);
        assert_eq!(rep.privacy.len(), 1);
        let out = dir.join("out");
        let rep2 = pack_share(&dir, &out, &GuardOptions::default()).unwrap();
        assert!(rep2.is_shareable());
        assert!(out.join("share-manifest.json").exists());
        assert!(out.join("src/lib.rs").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn synthetic_marker_allows_example_data_as_review() {
        let dir = std::env::temp_dir().join("cercis-guard-test-synth");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("tables")).unwrap();
        fs::write(dir.join(SYNTHETIC_MARKER), "全部为虚构数据\n").unwrap();
        fs::write(dir.join("project.yaml"), "schema_version: 1\nname: 合成示例\n").unwrap();
        fs::write(dir.join("tables/records.csv"), "record_id\nS-1\n").unwrap();

        let rep = scan(&dir, &GuardOptions::default()).unwrap();
        assert_eq!(rep.verdict, "review");
        assert_eq!(rep.refused, 0);
        assert!(rep.to_review >= 2, "数据类文件列为待确认");
        assert!(pack_share(&dir, &dir.join("out"), &GuardOptions::default()).is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_keywords_are_reported() {
        let dir = std::env::temp_dir().join("cercis-guard-test-kw");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("notes.md"), "这里面有未公开配方三个字\n").unwrap();
        let rep = scan(
            &dir,
            &GuardOptions {
                keywords: vec!["未公开配方".to_string()],
                ..Default::default()
            },
        )
        .unwrap();
        assert!(rep.privacy.iter().any(|p| p.kind == "keyword"));
        assert_eq!(rep.verdict, "review");
        let _ = fs::remove_dir_all(&dir);
    }
}
