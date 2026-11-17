//! `~/.codix/codix.yaml`（CODIX_HOME 指定时以它为准）。只放 Codix 自己的配置：模型、provider、
//! 权限沿用 Agix 的 settings.json（architecture §8.1）。文件不存在时写一份带注释的缺省配置。

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;
use tui::Size;

pub const FILE: &str = "codix.yaml";

/// 首次启动写到 `~/.codix/codix.yaml` 的内容：每一项都写出来并注释，用户照着改。
/// 模板放在单独的文件里，中英文各一份，不挤进代码。
pub const DEFAULT_YAML_ZH: &str = include_str!("codix.zh.yaml");
pub const DEFAULT_YAML_EN: &str = include_str!("codix.en.yaml");

/// 缺省配置按语言选模板；没有对应语言的模板用英文。
pub fn default_yaml(lang: &str) -> &'static str {
    if lang == "zh" {
        DEFAULT_YAML_ZH
    } else {
        DEFAULT_YAML_EN
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Config {
    pub ui: UiConfig,
    pub instance: InstanceConfig,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub language: String,
    pub initial_size: String,
    pub ratio: u16,
    pub compact_tool_lines: usize,
    pub syntax_theme: String,
    pub theme: ThemeConfig,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            language: "zh".into(),
            initial_size: "160x45".into(),
            ratio: 60,
            compact_tool_lines: 3,
            syntax_theme: "base16-ocean.dark".into(),
            theme: ThemeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ThemeConfig {
    pub preset: String,
    pub colors: BTreeMap<String, String>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        ThemeConfig {
            preset: "claude".into(),
            colors: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct InstanceConfig {
    pub idle_timeout: String,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        InstanceConfig {
            idle_timeout: "30m".into(),
        }
    }
}

impl UiConfig {
    pub fn initial_size(&self) -> Option<Size> {
        let (c, r) = self.initial_size.split_once(['x', 'X', '×'])?; // i18n-allow: 解析用户写的尺寸，× 是输入而非界面文字
        Some(Size::new(c.trim().parse().ok()?, r.trim().parse().ok()?))
    }
}

impl Config {
    /// 换成 model 的设置。读不懂的颜色、主题、时长都退回缺省，并把原因放进返回的列表。
    pub fn settings(&self) -> (model::Settings, Vec<String>) {
        let mut problems = vec![];
        let mut theme = tui::Theme::preset(&self.ui.theme.preset).unwrap_or_else(|| {
            problems.push(i18n::t!(
                "app.config.bad_preset",
                value = format!("{:?}", self.ui.theme.preset)
            ));
            tui::Theme::default()
        });
        for (k, v) in &self.ui.theme.colors {
            if let Err(e) = theme.set(k, v) {
                problems.push(i18n::t!("app.config.bad_color", key = k, error = e));
            }
        }
        let syntax_theme = if model::theme_names().contains(&self.ui.syntax_theme) {
            self.ui.syntax_theme.clone()
        } else {
            problems.push(i18n::t!(
                "app.config.bad_syntax_theme",
                value = format!("{:?}", self.ui.syntax_theme)
            ));
            model::Settings::default().syntax_theme
        };
        let idle_timeout_ms = parse_duration_ms(&self.instance.idle_timeout).unwrap_or_else(|| {
            problems.push(i18n::t!(
                "app.config.bad_idle_timeout",
                value = format!("{:?}", self.instance.idle_timeout)
            ));
            model::DEFAULT_IDLE_TIMEOUT_MS
        });
        (
            model::Settings {
                theme,
                syntax_theme,
                compact_tool_lines: self.ui.compact_tool_lines.max(1),
                ratio_pct: self.ui.ratio,
                idle_timeout_ms,
            },
            problems,
        )
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

/// 界面语言：`CODIX_LANG` 优先，其次配置的 `ui.language`；`auto` 照 `LC_ALL` / `LC_MESSAGES` /
/// `LANG`，都没有就用中文。
pub fn language(cfg: &Config) -> String {
    let want = std::env::var("CODIX_LANG").unwrap_or_else(|_| cfg.ui.language.clone());
    if want.trim() != "auto" {
        return i18n::normalize(want.trim());
    }
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|v| std::env::var(v).ok())
        .find(|v| !v.is_empty())
        .map(|v| i18n::normalize(&v))
        .filter(|l| l != "c" && l != "posix")
        .unwrap_or_else(|| i18n::REFERENCE.to_string())
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

/// 读配置；文件不存在就写一份缺省的。读不出来时用缺省值并把原因交给调用方显示：
/// 配置写错不该让程序起不来。
pub fn load() -> (Config, Vec<String>) {
    load_from(&home())
}

pub fn load_from(dir: &std::path::Path) -> (Config, Vec<String>) {
    let path = dir.join(FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => {
            let _ = std::fs::create_dir_all(dir);
            // 此时语言还没初始化：模板按环境（CODIX_LANG、LANG）选，写出后用户可以再改。
            let template = default_yaml(&language(&Config {
                ui: UiConfig {
                    language: "auto".into(),
                    ..UiConfig::default()
                },
                ..Config::default()
            }));
            let _ = std::fs::write(&path, template);
            template.to_string()
        }
    };
    match serde_yaml::from_str::<Config>(&text) {
        Ok(c) => (c, vec![]),
        Err(e) => (
            Config::default(),
            vec![i18n::t!(
                "app.config.bad_file",
                path = path.display(),
                error = e
            )],
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_created_and_parses_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let (c, problems) = load_from(dir.path());
        assert!(problems.is_empty(), "{problems:?}");
        assert!(dir.path().join(FILE).exists(), "写出了缺省配置");
        assert_eq!(c.ui.initial_size(), Some(Size::new(160, 45)));
        let (s, problems) = c.settings();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(s.theme, tui::Theme::claude());
        assert_eq!(s.idle_timeout_ms, 30 * 60_000);
    }

    #[test]
    fn overrides_and_bad_values_are_reported_not_fatal() {
        let c: Config = serde_yaml::from_str(
            "ui:\n  initial_size: off\n  syntax_theme: nope\n  theme:\n    preset: aqua\n    colors: {accent: '#010203', bogus: red}\ninstance:\n  idle_timeout: soon\n",
        )
        .unwrap();
        assert_eq!(c.ui.initial_size(), None);
        let (s, problems) = c.settings();
        assert_eq!(s.theme.accent, ratatui::style::Color::Rgb(1, 2, 3));
        assert_eq!(problems.len(), 3, "{problems:?}");
        assert_eq!(parse_duration_ms("2h"), Some(7_200_000));
    }
}
