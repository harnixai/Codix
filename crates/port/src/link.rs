//! TUI 与实例之间的链路帧（architecture §8.3）：端口消息原样序列化，一行一帧。

use serde::{Deserialize, Serialize};

use crate::{Command, Event, PortError, Reply};

/// TUI → 实例。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientFrame {
    pub id: u64,
    pub cmd: Command,
}

/// 实例 → TUI：某个请求的回复，或一个事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ServerFrame {
    Reply { id: u64, reply: LinkReply },
    Event { event: Event },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)] // 一帧序列化完就丢，不值得为大小装箱
pub enum LinkReply {
    Ok(Reply),
    Err(PortError),
}

impl From<Result<Reply, PortError>> for LinkReply {
    fn from(r: Result<Reply, PortError>) -> Self {
        match r {
            Ok(r) => LinkReply::Ok(r),
            Err(e) => LinkReply::Err(e),
        }
    }
}

impl From<LinkReply> for Result<Reply, PortError> {
    fn from(r: LinkReply) -> Self {
        match r {
            LinkReply::Ok(r) => Ok(r),
            LinkReply::Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_as_one_line() {
        let frames = [
            ServerFrame::Reply {
                id: 3,
                reply: LinkReply::Err(PortError::Disconnected),
            },
            ServerFrame::Reply {
                id: 4,
                reply: LinkReply::Ok(Reply::Ok),
            },
            ServerFrame::Event {
                event: Event::Notice {
                    level: crate::NoticeLevel::Info,
                    text: "a\nb".into(),
                },
            },
        ];
        for f in frames {
            let line = serde_json::to_string(&f).unwrap();
            assert!(!line.contains('\n'));
            assert_eq!(serde_json::from_str::<ServerFrame>(&line).unwrap(), f);
        }
        let c = ClientFrame {
            id: 1,
            cmd: Command::Detach,
        };
        let line = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<ClientFrame>(&line).unwrap(), c);
    }
}
