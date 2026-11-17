//! 守卫（需求 L-11）：界面文字都在 `crates/i18n/locales/` 里，代码里不再写死中文。
//! 扫 model、tui、keys、app 的源码：去掉 `//` 注释后，一行里还有中文（汉字或全角标点）就算
//! 漏了。`#[cfg(test)]` 之后的测试代码、行尾带 `i18n-allow` 的行不算。

use std::path::{Path, PathBuf};

fn is_cjk(c: char) -> bool {
    matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// 漏掉的行：`文件:行号: 内容`。
pub fn leftovers() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = vec![];
    for c in ["model", "tui", "keys", "app"] {
        rs_files(&root.join(c).join("src"), &mut files);
    }
    files.sort();
    let mut out = vec![];
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("#[cfg(test)]") {
                break;
            }
            if line.contains("i18n-allow") {
                continue;
            }
            let code = match line.find("//") {
                Some(i) => &line[..i],
                None => line,
            };
            if code.chars().any(is_cjk) {
                let rel = f.strip_prefix(&root).unwrap_or(&f).display().to_string();
                out.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    out
}

#[test]
fn no_hardcoded_chinese_in_ui_code() {
    let left = leftovers();
    assert!(
        left.is_empty(),
        "这些行还写死了中文，挪到 crates/i18n/locales/ 里用 t!(...) 取（共 {} 行）：\n{}",
        left.len(),
        left.join("\n")
    );
}
