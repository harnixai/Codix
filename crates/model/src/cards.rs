//! 编写 / 作业会话最上面的卡片：流程的节点设计、作业的详情（交互 §5.3、§5.4）。右栏只放
//! 摘要，详细的在左边看；改它们走下面的对话。

use port::{FlowDetail, Job, JobEntry, JobKind, JobOutcome, StationView};
use tui::{Line, Tone, display_width, truncate_to_width};

use crate::text::wrap_text;

#[derive(Debug, Clone, PartialEq)]
pub enum Card {
    Flow(Box<FlowDetail>),
    Job(Box<Job>, Vec<JobEntry>),
}

pub(crate) fn render(card: &Card, width: usize) -> Vec<Line> {
    let w = width.clamp(24, 100);
    let (title, body) = match card {
        Card::Flow(d) => (
            format!("流程设计：{}", d.summary.title),
            flow_body(d, w - 4),
        ),
        Card::Job(j, h) => (format!("作业：{}", j.name), job_body(j, h, w - 4)),
    };
    boxed(&title, body, w)
}

fn boxed(title: &str, body: Vec<Line>, w: usize) -> Vec<Line> {
    let inner = w - 4;
    let head = truncate_to_width(&format!("─ {title} "), w - 2);
    let mut out = vec![Line::toned(
        format!("╭{head}{}╮", "─".repeat(w - 2 - display_width(&head))),
        Tone::Accent,
    )];
    for l in body {
        let lw = l.width().min(inner);
        let mut row = Line::toned("│ ", Tone::Accent);
        if l.width() > inner {
            row = row.push(truncate_to_width(&l.text(), inner), Tone::Normal);
        } else {
            for s in l.spans {
                row = row.push(s.text, s.tone);
            }
        }
        out.push(
            row.push(" ".repeat(inner - lw), Tone::Normal)
                .push(" │", Tone::Accent),
        );
    }
    out.push(Line::toned(
        format!("╰{}╯", "─".repeat(w - 2)),
        Tone::Accent,
    ));
    out
}

/// 站按依赖分层：同一层的可以并行，层号就是图上的列。
pub(crate) fn layers(stations: &[StationView]) -> Vec<usize> {
    let mut depth = vec![0usize; stations.len()];
    for i in 0..stations.len() {
        let needs: Vec<usize> = if stations[i].needs.is_empty() {
            i.checked_sub(1).into_iter().collect()
        } else {
            stations[i]
                .needs
                .iter()
                .filter_map(|n| stations.iter().position(|s| &s.id == n))
                .collect()
        };
        depth[i] = needs.iter().map(|j| depth[*j] + 1).max().unwrap_or(0);
    }
    depth
}

fn flow_body(d: &FlowDetail, inner: usize) -> Vec<Line> {
    let origin = match d.summary.origin {
        port::FlowOrigin::Builtin => "内置",
        port::FlowOrigin::Override => "覆盖",
        port::FlowOrigin::User => "用户",
    };
    let mut out = vec![
        Line::toned(
            format!(
                "{} · {origin} · {} 站 · {} 个在跑",
                d.summary.name,
                d.stations.len(),
                d.summary.active_runs
            ),
            Tone::Dim,
        ),
        Line::default(),
    ];
    let depth = layers(&d.stations);
    let max = depth.iter().copied().max().unwrap_or(0);
    // 节点图：一层一行，同层并行用 ∥ 连起来。
    let mut chain = vec![];
    for l in 0..=max {
        let names: Vec<&str> = d
            .stations
            .iter()
            .zip(&depth)
            .filter(|(_, x)| **x == l)
            .map(|(s, _)| s.title.as_str())
            .collect();
        chain.push(names.join(" ∥ "));
    }
    out.extend(wrap_text(
        &chain.join(" → "),
        Tone::Accent,
        inner,
        &Line::default(),
        &Line::plain("  "),
    ));
    out.push(Line::default());
    let title_w = d
        .stations
        .iter()
        .map(|s| display_width(&s.title))
        .max()
        .unwrap_or(4)
        .min(14);
    for (s, l) in d.stations.iter().zip(&depth) {
        let pad = title_w.saturating_sub(display_width(&s.title));
        let mut line = Line::toned(format!("{:>2}. ", l + 1), Tone::Dim)
            .push(format!("{}{}", s.title, " ".repeat(pad)), Tone::Strong)
            .push(format!("  {:<10}", s.work), Tone::Normal);
        if let Some(sk) = &s.skill {
            line = line.push(format!("  skill {sk}"), Tone::Code);
        }
        if !s.exit.is_empty() {
            line = line.push(format!("  出口 {}", s.exit.join("、")), Tone::Dim);
        }
        if s.needs.len() > 1 {
            let names: Vec<&str> = s
                .needs
                .iter()
                .filter_map(|n| {
                    d.stations
                        .iter()
                        .find(|x| &x.id == n)
                        .map(|x| x.title.as_str())
                })
                .collect();
            line = line.push(format!("  ← 等 {}", names.join("、")), Tone::Warn);
        }
        out.push(line);
    }
    out.push(Line::default());
    out.push(Line::toned(
        "在下面的对话里修改：加一站、改出口条件、调整顺序、启动一次运行…",
        Tone::Dim,
    ));
    out
}

/// `11-16 09:00`（UTC）。
pub(crate) fn fmt_clock(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    let (h, m) = (secs % 86_400 / 3600, secs % 3600 / 60);
    // Howard Hinnant 的 civil_from_days。
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{mo:02}-{d:02} {h:02}:{m:02}")
}

pub(crate) fn outcome(o: &JobOutcome) -> (String, Tone) {
    match o {
        JobOutcome::Running => ("● 运行中".into(), Tone::Ok),
        JobOutcome::Done => ("✓ 完成".into(), Tone::Ok),
        JobOutcome::Failed { message } => (format!("✗ 失败：{message}"), Tone::Error),
        JobOutcome::Skipped => ("⊘ 跳过".into(), Tone::Dim),
    }
}

pub(crate) fn job_info(j: &Job) -> Vec<Line> {
    let next = j
        .next_fire_ms
        .map(|t| format!("下次 {}", fmt_clock(t)))
        .unwrap_or_else(|| "不会再触发".into());
    let mut out = vec![Line::toned(
        format!(
            "{}（{next}）· {} · 错过补跑：{}",
            j.cron,
            if j.enabled { "启用" } else { "已停用" },
            if j.catch_up { "是" } else { "否" }
        ),
        Tone::Normal,
    )];
    match &j.kind {
        JobKind::Simple {
            prompt,
            skills,
            project,
            scene,
            model,
        } => {
            out.push(Line::toned("简单作业：提示词 + skills", Tone::Dim));
            out.push(Line::toned("提示词 ", Tone::Dim).push(prompt.clone(), Tone::Normal));
            out.push(Line::toned(
                format!(
                    "skills {} · 项目 {} · 场景 {} · 模型 {}",
                    if skills.is_empty() {
                        "—".into()
                    } else {
                        skills.join("、")
                    },
                    project.as_deref().unwrap_or("—"),
                    scene.as_deref().unwrap_or("—"),
                    model.as_deref().unwrap_or("缺省")
                ),
                Tone::Dim,
            ));
        }
        JobKind::Flow { flow, subject, .. } => {
            out.push(Line::toned("流程作业", Tone::Dim));
            out.push(Line::toned("流程 ", Tone::Dim).push(flow.clone(), Tone::Normal));
            out.push(Line::toned("主题 ", Tone::Dim).push(subject.clone(), Tone::Normal));
        }
    }
    out
}

fn job_body(j: &Job, history: &[JobEntry], inner: usize) -> Vec<Line> {
    let mut out = job_info(j);
    out.push(Line::default());
    out.push(Line::toned("最近的触发", Tone::Strong));
    if history.is_empty() {
        out.push(Line::toned("还没有触发过", Tone::Dim));
    }
    for e in history.iter().take(5) {
        let (o, tone) = outcome(&e.outcome);
        out.push(
            Line::toned(format!("{}  ", fmt_clock(e.fired_ms)), Tone::Dim)
                .push(truncate_to_width(&o, inner.saturating_sub(16)), tone),
        );
    }
    out.push(Line::default());
    out.push(Line::toned(
        "在下面的对话里修改或删除这个作业，或者让它立即跑一次",
        Tone::Dim,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_is_utc_month_day() {
        assert_eq!(fmt_clock(1_763_280_000_000), "11-16 08:00");
    }

    fn st(id: &str, needs: &[&str]) -> StationView {
        StationView {
            id: id.into(),
            title: id.into(),
            needs: needs.iter().map(|n| (*n).into()).collect(),
            work: String::new(),
            skill: None,
            exit: vec![],
            status: port::StationStatus::Pending,
            tasks: vec![],
            elapsed_ms: 0,
            usage: Default::default(),
            unmet: None,
        }
    }

    #[test]
    fn layers_follow_needs_and_default_to_previous() {
        let s = vec![
            st("a", &[]),
            st("b", &[]),
            st("c", &["b"]),
            st("d", &["b"]),
            st("e", &["c", "d"]),
        ];
        assert_eq!(layers(&s), vec![0, 1, 2, 2, 3]);
    }
}
