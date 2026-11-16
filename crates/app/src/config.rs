//! `~/.codix/codix.toml`（CODIX_HOME 指定时以它为准）。只放 Codix 自己的配置；模型、provider、
//! 权限沿用 Agix 的 settings.json（architecture §8.1）。

use std::path::PathBuf;

use serde::Deserialize;
use tui::Size;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub instance: InstanceConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct InstanceConfig {
    /// `"30m"`、`"2h"`、`"0"`（不超时），需求 I-10。
    pub idle_timeout: String,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        InstanceConfig {
            idle_timeout: "30m".into(),
        }
    }
}

/// `45s`、`30m`、`2h`、`1d`、`0`；读不懂返回 `None`。
pub fn parse_duration_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    if s == "0" {
        return Some(0);
    }
    let (num, unit) = s.split_at(s.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = num.parse().ok()?;
    let per = match unit.trim() {
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return None,
    };
    Some(n * per)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    /// `"160x45"` 或 `"off"`（需求 L-06）。
    pub initial_size: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            initial_size: "160x45".into(),
        }
    }
}

impl UiConfig {
    pub fn initial_size(&self) -> Option<Size> {
        let (c, r) = self.initial_size.split_once(['x', 'X', '×'])?;
        Some(Size::new(c.trim().parse().ok()?, r.trim().parse().ok()?))
    }
}

pub fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("CODIX_HOME") {
        return PathBuf::from(h);
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
        .join(".codix")
}

/// 读不出来时用缺省值，并把原因交给调用方显示：配置写错不该让程序起不来。
pub fn load() -> (Config, Option<String>) {
    let path = home().join("codix.toml");
    match std::fs::read_to_string(&path) {
        Err(_) => (Config::default(), None),
        Ok(text) => match toml::from_str(&text) {
            Ok(c) => (c, None),
            Err(e) => (
                Config::default(),
                Some(format!("{} 有误，已用缺省值：{e}", path.display())),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_size_or_off() {
        let c: Config = toml::from_str("[ui]\ninitial_size = \"120x40\"").unwrap();
        assert_eq!(c.ui.initial_size(), Some(Size::new(120, 40)));
        let c: Config = toml::from_str("[ui]\ninitial_size = \"off\"").unwrap();
        assert_eq!(c.ui.initial_size(), None);
        assert_eq!(
            Config::default().ui.initial_size(),
            Some(Size::new(160, 45))
        );
    }
}
