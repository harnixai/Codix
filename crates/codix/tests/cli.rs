//! 命令行帮助按语言显示（需求 L-11）：起真实的二进制、切到英文。

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
    })
}

fn help_of(args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_codix"))
        .args(args)
        .env("CODIX_LANG", "en")
        .env("CODIX_HOME", tempfile::tempdir().unwrap().path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn cli_help_is_english() {
    let text = help_of(&["--help"]);
    assert!(text.contains("agent workbench"), "{text}");
    assert!(!has_cjk(&text), "{text}");
    for sub in ["new", "list", "attach", "start", "stop"] {
        let text = help_of(&[sub, "--help"]);
        assert!(!has_cjk(&text), "{sub}: {text}");
    }
    assert!(help_of(&["attach", "--help"]).contains("Kick out"));
}
