//! 5 段 cron（分 时 日 月 周）。

use chrono::{DateTime, Local, TimeZone};
use croner::Cron;

fn parse(expr: &str) -> Result<Cron, String> {
    // 先数段数：croner 对 6、7 段的串在未开选项时的报错信息很含糊。
    let n = expr.split_whitespace().count();
    if n != 5 {
        return Err(format!("cron 需要 5 段（分 时 日 月 周），收到 {n} 段"));
    }
    Cron::new(expr).parse().map_err(|e| e.to_string())
}

pub fn validate(expr: &str) -> Result<(), String> {
    parse(expr).map(|_| ())
}

/// 本地时区下严格晚于 `after_ms` 的下一次触发。
pub fn next_after(expr: &str, after_ms: u64) -> Option<u64> {
    next_after_in(expr, after_ms, &Local)
}

/// 时区可注入，测试才能不依赖机器设置。
pub fn next_after_in<Tz: TimeZone>(expr: &str, after_ms: u64, tz: &Tz) -> Option<u64> {
    let cron = parse(expr).ok()?;
    // 先截到整秒：croner 的"严格晚于"是加 1 秒再找，带毫秒的起点会让结果带着零头。
    let secs = i64::try_from(after_ms / 1000).ok()?;
    let start: DateTime<Tz> = tz.timestamp_opt(secs, 0).single()?;
    let next = cron.find_next_occurrence(&start, false).ok()?;
    u64::try_from(next.timestamp_millis()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    // 2024-01-01T00:00:00Z，周一。
    const T0: u64 = 1_704_067_200_000;
    const MIN: u64 = 60_000;
    const HOUR: u64 = 60 * MIN;

    #[test]
    fn every_minute_is_strictly_after() {
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(next_after_in("* * * * *", T0, &utc), Some(T0 + MIN));
        assert_eq!(next_after_in("* * * * *", T0 + 500, &utc), Some(T0 + MIN));
        assert_eq!(next_after_in("* * * * *", T0 - 1, &utc), Some(T0));
    }

    #[test]
    fn daily_respects_timezone() {
        let utc = FixedOffset::east_opt(0).unwrap();
        let cst = FixedOffset::east_opt(8 * 3600).unwrap();
        assert_eq!(next_after_in("0 9 * * *", T0, &utc), Some(T0 + 9 * HOUR));
        // 东八区的 09:00 是 UTC 01:00。
        assert_eq!(next_after_in("0 9 * * *", T0, &cst), Some(T0 + HOUR));
        assert_eq!(
            next_after_in("0 9 * * *", T0 + HOUR, &cst),
            Some(T0 + 25 * HOUR)
        );
    }

    #[test]
    fn weekday_field() {
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(next_after_in("0 0 * * 3", T0, &utc), Some(T0 + 48 * HOUR));
    }

    #[test]
    fn validate_rejects_bad() {
        assert!(validate("*/5 * * * *").is_ok());
        assert!(validate("").is_err());
        assert!(validate("* * * *").is_err());
        assert!(validate("0 0 * * * *").is_err());
        assert!(validate("61 * * * *").is_err());
        assert_eq!(next_after("nope", T0), None);
    }
}
