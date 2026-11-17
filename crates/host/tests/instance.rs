//! 实例目录的认领：锁、meta、发现文件与 socket。

use std::os::unix::fs::PermissionsExt;

use client::layout::read_json;
use client::{InstanceFile, InstanceMeta, Layout};
use host::instance::{ClaimError, claim};

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test]
async fn claim_locks_binds_and_releases() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    let c = claim(&layout, "a", Some("/p")).unwrap();
    assert_eq!(mode(&layout.instance("a")), 0o700);
    assert!(matches!(
        claim(&layout, "a", None),
        Err(ClaimError::AlreadyRunning(n)) if n == "a"
    ));

    // 上次崩溃留下的 socket 不能挡住 bind。
    std::fs::write(layout.socket("a"), b"").unwrap();
    let _listener = c.bind().unwrap();
    assert_eq!(mode(&layout.socket("a")), 0o600);

    let f = InstanceFile {
        name: "a".into(),
        pid: std::process::id(),
        host: host::instance::hostname(),
        project: Some("/p".into()),
        started_ms: 1,
        version: "0".into(),
        holder: None,
    };
    c.write_discovery(&f).unwrap();
    assert_eq!(read_json::<InstanceFile>(&layout.discovery("a")), Some(f));

    c.release().unwrap();
    assert!(!layout.discovery("a").exists() && !layout.socket("a").exists());
    let meta = read_json::<InstanceMeta>(&layout.meta("a")).unwrap();
    assert_eq!(meta.project.as_deref(), Some("/p"));
    assert!(meta.stopped_ms.is_some());

    // 锁随 Claim 放掉；不给项目时沿用 meta 里的。
    let c = claim(&layout, "a", None).unwrap();
    let meta = read_json::<InstanceMeta>(&layout.meta("a")).unwrap();
    assert_eq!(
        (meta.project.as_deref(), meta.stopped_ms),
        (Some("/p"), None)
    );
    drop(c);
}
