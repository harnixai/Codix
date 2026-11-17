//! 把 `locales/<语言>/*.yaml` 全部编进二进制。加一种语言就是加一个目录，不用改代码。

use std::fmt::Write as _;
use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("locales");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut langs: Vec<_> = std::fs::read_dir(&root)
        .expect("locales 目录")
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .collect();
    langs.sort_by_key(|e| e.file_name());
    let mut out = String::from("pub(crate) static BUILTIN: &[(&str, &[(&str, &str)])] = &[\n");
    for lang in langs {
        let name = lang.file_name().to_string_lossy().to_string();
        println!("cargo:rerun-if-changed={}", lang.path().display());
        let mut files: Vec<_> = std::fs::read_dir(lang.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "yaml"))
            .collect();
        files.sort();
        writeln!(out, "    ({name:?}, &[").unwrap();
        for f in files {
            println!("cargo:rerun-if-changed={}", f.display());
            let file = f.file_name().unwrap().to_string_lossy().to_string();
            writeln!(
                out,
                "        ({file:?}, include_str!({:?})),",
                f.display().to_string()
            )
            .unwrap();
        }
        writeln!(out, "    ]),").unwrap();
    }
    out.push_str("];\n");
    let dest = Path::new(&std::env::var("OUT_DIR").unwrap()).join("builtin.rs");
    std::fs::write(dest, out).unwrap();
}
