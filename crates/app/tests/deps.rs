//! 依赖方向守卫（docs/design/architecture.md §2）：只看各 crate 的直接依赖。

use std::collections::BTreeMap;
use std::process::Command;

fn direct_deps() -> BTreeMap<String, Vec<String>> {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .output()
        .expect("cargo metadata");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| {
            let deps = p["dependencies"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|d| d["kind"].is_null())
                .map(|d| d["name"].as_str().unwrap().to_string())
                .collect();
            (p["name"].as_str().unwrap().to_string(), deps)
        })
        .collect()
}

#[test]
fn layers_point_one_way() {
    let deps = direct_deps();
    let forbidden: &[(&str, &[&str])] = &[
        (
            "port",
            &[
                "tui",
                "keys",
                "model",
                "mock",
                "app",
                "ratatui",
                "crossterm",
                "agix",
            ],
        ),
        (
            "tui",
            &[
                "port",
                "keys",
                "model",
                "mock",
                "app",
                "tokio",
                "crossterm",
                "agix",
            ],
        ),
        (
            "keys",
            &["port", "tui", "model", "mock", "app", "tokio", "agix"],
        ),
        (
            "model",
            &["mock", "app", "tokio", "crossterm", "ratatui", "agix"],
        ),
        (
            "mock",
            &[
                "tui",
                "keys",
                "model",
                "app",
                "ratatui",
                "crossterm",
                "agix",
            ],
        ),
        ("app", &["agix"]),
    ];
    let mut broken = Vec::new();
    for (krate, bad) in forbidden {
        let have = deps
            .get(*krate)
            .unwrap_or_else(|| panic!("no crate {krate}"));
        for b in *bad {
            if have.iter().any(|d| d == b) {
                broken.push(format!("{krate} → {b}"));
            }
        }
    }
    assert!(broken.is_empty(), "依赖方向被破坏：{broken:?}");
}
