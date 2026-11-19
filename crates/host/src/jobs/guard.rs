//! 无人值守的护栏（architecture §8.5）：全局缺省与天花板、作业自己写的项、实际生效的值。
//! 天花板来自这台机器的 `codix.yaml`，作业定义来自仓库：仓库里的作业只能比天花板更紧。

use port::{JobGuard, Overlap};

const MIN: u64 = 60_000;

#[derive(Debug, Clone, PartialEq)]
pub struct GuardConfig {
    /// 作业没写时用；每项都有值。
    pub defaults: JobGuard,
    pub max_timeout_ms: u64,
    pub max_tokens: u64,
    /// 这个实例所有无人值守的工作一天合计；0 是不限。
    pub daily_tokens: u64,
    pub max_concurrent: u32,
}

impl Default for GuardConfig {
    fn default() -> Self {
        GuardConfig {
            defaults: JobGuard {
                timeout_ms: Some(30 * MIN),
                max_tokens: Some(2_000_000),
                overlap: Some(Overlap::Skip),
                disable_after: Some(3),
            },
            max_timeout_ms: 240 * MIN,
            max_tokens: 20_000_000,
            daily_tokens: 50_000_000,
            max_concurrent: 2,
        }
    }
}

impl GuardConfig {
    /// 作业写的项取与天花板的较小值，没写的用缺省（缺省同样不超过天花板）。返回生效值与被压过的项名。
    pub fn effective(&self, g: &JobGuard) -> (JobGuard, Vec<String>) {
        let mut capped = vec![];
        let mut cap = |name: &str, want: Option<u64>, default: Option<u64>, max: u64| {
            let v = want.or(default).unwrap_or(max);
            if v > max {
                if want.is_some() {
                    capped.push(name.to_string());
                }
                max
            } else {
                v
            }
        };
        let timeout = cap(
            "timeout",
            g.timeout_ms,
            self.defaults.timeout_ms,
            self.max_timeout_ms,
        );
        let tokens = cap(
            "max_tokens",
            g.max_tokens,
            self.defaults.max_tokens,
            self.max_tokens,
        );
        (
            JobGuard {
                timeout_ms: Some(timeout),
                max_tokens: Some(tokens),
                overlap: g.overlap.or(self.defaults.overlap).or(Some(Overlap::Skip)),
                disable_after: g.disable_after.or(self.defaults.disable_after).or(Some(0)),
            },
            capped,
        )
    }
}

/// `30m`、`2h`、`90s`、`1h30m`；纯数字按分钟。
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Ok(n) = s.parse::<u64>() {
        return Some(n * MIN);
    }
    let (mut total, mut num) = (0u64, String::new());
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        total += n * match c {
            's' => 1_000,
            'm' => MIN,
            'h' => 60 * MIN,
            'd' => 24 * 60 * MIN,
            _ => return None,
        };
    }
    num.is_empty().then_some(total).filter(|t| *t > 0)
}

/// `2M`、`500k`、`1.5M`、纯数字。
pub fn parse_tokens(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mul) = match s.chars().last()? {
        'k' | 'K' => (&s[..s.len() - 1], 1_000.0),
        'm' | 'M' => (&s[..s.len() - 1], 1_000_000.0),
        _ => (s, 1.0),
    };
    let v: f64 = num.trim().parse().ok()?;
    (v > 0.0).then_some((v * mul) as u64)
}

pub fn fmt_duration(ms: u64) -> String {
    let m = ms / MIN;
    match (m / 60, m % 60) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h{m}m"),
    }
}

pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_token_counts_parse() {
        assert_eq!(parse_duration("30m"), Some(30 * MIN));
        assert_eq!(parse_duration("1h30m"), Some(90 * MIN));
        assert_eq!(parse_duration("45"), Some(45 * MIN));
        assert_eq!(parse_duration("90s"), Some(90_000));
        assert_eq!(parse_duration("3x"), None);
        assert_eq!(parse_duration("1h3"), None);
        assert_eq!(parse_tokens("2M"), Some(2_000_000));
        assert_eq!(parse_tokens("1.5m"), Some(1_500_000));
        assert_eq!(parse_tokens("500k"), Some(500_000));
        assert_eq!(parse_tokens("abc"), None);
    }

    /// 仓库里的作业放不宽这台机器的天花板；没写的用缺省。
    #[test]
    fn a_jobs_own_guard_is_capped_by_the_machines_limits() {
        let c = GuardConfig::default();
        let (g, capped) = c.effective(&JobGuard {
            timeout_ms: Some(48 * 60 * MIN),
            max_tokens: Some(500_000),
            overlap: Some(Overlap::Queue),
            disable_after: None,
        });
        assert_eq!(g.timeout_ms, Some(240 * MIN));
        assert_eq!(g.max_tokens, Some(500_000));
        assert_eq!(g.overlap, Some(Overlap::Queue));
        assert_eq!(g.disable_after, Some(3));
        assert_eq!(capped, ["timeout"]);
        let (d, capped) = c.effective(&JobGuard::default());
        assert_eq!(d, c.defaults);
        assert!(capped.is_empty());
    }
}
