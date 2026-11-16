use unicode_width::UnicodeWidthChar;

/// 终端里显示的列数。控制字符记 0。
pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// 截到不超过 `width` 列；被截断时末尾放 `…`（它也占一列）。
pub fn truncate_to_width(s: &str, width: usize) -> String {
    if display_width(s) <= width {
        return s.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width - 1 {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cjk_counts_double() {
        assert_eq!(display_width("会话ab"), 6);
        assert_eq!(truncate_to_width("重构认证模块", 7), "重构认…");
        assert_eq!(truncate_to_width("abc", 3), "abc");
    }
}
