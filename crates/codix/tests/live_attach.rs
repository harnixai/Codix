//! LIVE：单一接入（architecture §8.3）——握手、被占用、强制接入踢人、脱离放开接入、
//! 空闲脱离、心跳超时断开后重连、实例被停时接入者收到通知。

mod live;

use std::time::Duration;

use live::{Beat, Live, NAME, Opts, QUICK};
use port::{Attach, Command, ConnState, Event, Instances, PortError, Reply};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn hello_reports_the_instance_its_capabilities_and_the_holder() {
    let lv = Live::new();
    let c = lv.start().await;
    let Reply::Hello {
        instance,
        flows,
        jobs,
    } = c
        .ok(Command::Hello {
            client: client::this_client(),
            force: false,
            idle_timeout_ms: None,
        })
        .await
    else {
        panic!("hello")
    };
    assert_eq!(instance.name.0, NAME);
    assert_eq!(
        instance.project.as_deref(),
        Some(lv.project_path().as_str())
    );
    assert!(flows, "流程接上了");
    assert!(jobs);
    assert_eq!(
        instance.holder.map(|h| h.pid),
        Some(std::process::id()),
        "接入者是自己"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_second_client_is_refused_until_it_forces_and_then_the_first_is_kicked() {
    let lv = Live::new();
    let mut a = lv.start().await;

    match lv.attach(Attach::default()).await {
        Err(PortError::Occupied { holder, .. }) => assert_eq!(holder.pid, std::process::id()),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("同一实例只能接一个客户端"),
    }
    let b = lv
        .attach(Attach {
            force: true,
            ..Default::default()
        })
        .await
        .expect("强制接入");
    a.wait_event("被踢", QUICK, |e| {
        matches!(e, Event::Kicked { by, .. } if by.pid == std::process::id()).then_some(())
    })
    .await;
    // 被踢的一方不重连，命令得到断开。
    assert!(matches!(
        a.call(Command::ListJobs).await,
        Err(PortError::Disconnected)
    ));
    assert!(b.call(Command::ListJobs).await.is_ok(), "新接入者正常可用");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn detaching_releases_the_instance_for_the_next_client() {
    let lv = Live::new();
    let a = lv.start().await;
    assert!(!lv.read("instance.json")["holder"].is_null());
    a.ok(Command::Detach).await;
    // 实例在连接收尾时才放开接入、重写发现文件。
    let deadline = std::time::Instant::now() + QUICK;
    while !lv.read("instance.json")["holder"].is_null() {
        assert!(
            std::time::Instant::now() < deadline,
            "发现文件里的接入者没清掉"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    lv.attach(Attach::default())
        .await
        .expect("脱离之后不带 force 也能接入");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn an_idle_client_is_detached_and_told_so() {
    let lv = Live::new();
    let a = lv.start().await;
    a.ok(Command::Detach).await;
    drop(a);
    let mut b = loop {
        // 上一条连接的收尾与这次接入可能交错：被占用就稍后再试。
        match lv
            .attach(Attach {
                force: false,
                idle_timeout_ms: Some(2_000),
            })
            .await
        {
            Ok(b) => break b,
            Err(PortError::Occupied { .. }) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(e) => panic!("{e:?}"),
        }
    };
    b.heartbeat(Beat::None);
    let after = b
        .wait_event("空闲脱离", QUICK, |e| match e {
            Event::IdleDetached { after_ms } => Some(*after_ms),
            _ => None,
        })
        .await;
    assert!(after >= 2_000, "{after}");
    lv.attach(Attach::default())
        .await
        .expect("空闲脱离之后实例空出来了");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_silent_client_is_dropped_after_the_heartbeat_timeout_and_reconnects() {
    let lv = Live::with(Opts {
        env: vec![("CODIX_TEST_HEARTBEAT_TIMEOUT_MS".into(), "2000".into())],
        ..Default::default()
    });
    let mut a = lv.start().await;
    a.heartbeat(Beat::None);
    a.wait_event("心跳超时断开", QUICK, |e| {
        matches!(e, Event::Connection(ConnState::Lost)).then_some(())
    })
    .await;
    a.wait_event("重连上", QUICK, |e| {
        matches!(e, Event::Connection(ConnState::Resynced)).then_some(())
    })
    .await;
    a.heartbeat(Beat::Active);
    assert!(a.call(Command::ListJobs).await.is_ok(), "重连后照常可用");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn stopping_the_instance_tells_the_attached_client() {
    let lv = Live::new();
    let mut a = lv.start().await;
    lv.stop().await;
    let reason = a
        .wait_event("实例停了", QUICK, |e| match e {
            Event::InstanceStopped { reason } => Some(reason.clone()),
            _ => None,
        })
        .await;
    assert_eq!(reason, "stopped");
    assert!(lv.instances.list(false).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_stop_instance_command_shuts_the_instance_down() {
    let lv = Live::new();
    let a = lv.start().await;
    a.ok(Command::StopInstance).await;
    let deadline = std::time::Instant::now() + QUICK;
    while !lv.instances.list(false).await.unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "实例没停下");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(lv.read("meta.json")["stopped_ms"].is_u64());
    assert!(!lv.layout().socket(NAME).exists());
}
