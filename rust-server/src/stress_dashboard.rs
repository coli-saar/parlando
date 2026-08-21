//! Shared terminal presentation for Parlando's optional stress tools.
//!
//! The dashboard owns only terminal lifecycle and rendering. Workload binaries
//! provide bounded measurement snapshots, so the renderer cannot become a
//! second source of pass/fail semantics.

use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use anyhow::Result;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEventKind},
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Sparkline, Wrap},
    DefaultTerminal, Frame,
};
use tokio::sync::watch;

const UI_TICK: Duration = Duration::from_millis(100);

/// One label, value, and health color shown in a dashboard metric panel.
#[derive(Clone, Debug)]
pub struct DashboardMetric {
    /// Stable human-readable metric label.
    pub label: String,
    /// Current measured value.
    pub value: String,
    /// Visual health classification supplied by the workload.
    pub health: DashboardHealth,
}

/// Color category used consistently by all stress dashboards.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DashboardHealth {
    /// A healthy or informational measurement.
    Good,
    /// A measured degradation or expected impairment.
    Warning,
    /// A failed invariant or fatal condition.
    Error,
    /// A neutral label or unavailable measurement.
    Neutral,
}

/// One bordered metric panel in the shared dashboard layout.
#[derive(Clone, Debug)]
pub struct DashboardPanel {
    /// Panel title rendered in its border.
    pub title: String,
    /// Bounded rows rendered in order.
    pub metrics: Vec<DashboardMetric>,
}

/// A selectable rolling time series rendered as the shared sparkline.
#[derive(Clone, Debug)]
pub struct DashboardSeries {
    /// Series title and unit.
    pub title: String,
    /// Bounded chronological samples.
    pub values: Vec<u64>,
}

/// A per-room health tile used by the shared heatmap.
#[derive(Clone, Debug)]
pub struct DashboardTile {
    /// Current tile health.
    pub health: DashboardHealth,
}

/// Measured, presentation-safe state rendered by the shared stress dashboard.
#[derive(Clone, Debug)]
pub struct DashboardSnapshot {
    /// Dashboard title, for example `Parlando audio stress`.
    pub title: String,
    /// Workload mode name.
    pub mode: String,
    /// Current phase name, if the workload has phases.
    pub phase: Option<String>,
    /// Elapsed workload duration.
    pub elapsed: Duration,
    /// Configured workload duration.
    pub duration: Duration,
    /// Whether the workload has completed its final validation.
    pub finished: bool,
    /// Whether a user requested clean cancellation.
    pub cancelled: bool,
    /// Fatal failure text, intentionally bounded by the workload.
    pub failure: Option<String>,
    /// Exactly three primary metric panels.
    pub panels: [DashboardPanel; 3],
    /// One or more selectable rolling series.
    pub series: Vec<DashboardSeries>,
    /// Current health for each active room.
    pub tiles: Vec<DashboardTile>,
    /// Heatmap legend supplied by the workload.
    pub tile_legend: String,
    /// Bounded newest-first milestones and events.
    pub events: Vec<String>,
    /// Extra footer text after the shared controls.
    pub footer: String,
}

/// Runs the common interactive terminal loop until a workload finishes.
///
/// Pressing `q` or `Esc` sets `cancelled`; workloads own draining and final
/// validation. `Tab` selects the next sparkline series. The renderer receives
/// only the caller's bounded snapshot adapter and restores the terminal before
/// returning.
pub fn run_dashboard<T, F>(
    terminal: &mut DefaultTerminal,
    snapshots: &mut watch::Receiver<T>,
    cancelled: Arc<AtomicBool>,
    selected_series: Arc<AtomicUsize>,
    render: F,
) -> Result<()>
where
    T: Clone,
    F: Fn(&T) -> DashboardSnapshot,
{
    loop {
        let source = snapshots.borrow_and_update().clone();
        let snapshot = render(&source);
        terminal.draw(|frame| draw_dashboard(frame, &snapshot, &selected_series))?;
        if snapshot.finished {
            std::thread::sleep(Duration::from_millis(900));
            return Ok(());
        }
        if event::poll(UI_TICK)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                        KeyCode::Tab if !snapshot.series.is_empty() => {
                            selected_series.fetch_add(1, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }
            }
        }
        let _ = snapshots.has_changed();
    }
}

/// Renders the fixed shared dashboard geometry from a measured snapshot.
fn draw_dashboard(frame: &mut Frame, snapshot: &DashboardSnapshot, selected_series: &AtomicUsize) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(8),
            Constraint::Length(5),
            Constraint::Min(7),
            Constraint::Length(5),
            Constraint::Length(1),
        ])
        .split(frame.area());
    draw_progress(frame, outer[0], snapshot);
    draw_metrics(frame, outer[1], snapshot);
    draw_series(frame, outer[2], snapshot, selected_series);
    draw_tiles(frame, outer[3], snapshot);
    draw_events(frame, outer[4], snapshot);
    let footer = if snapshot.finished {
        "Complete — terminal will close shortly".to_string()
    } else {
        format!(
            "q / Esc: stop cleanly   •   Tab: next series   •   {}",
            snapshot.footer
        )
    };
    frame.render_widget(
        Paragraph::new(footer).style(Style::default().fg(Color::DarkGray)),
        outer[5],
    );
}

/// Renders elapsed progress and the current workload status.
fn draw_progress(frame: &mut Frame, area: Rect, snapshot: &DashboardSnapshot) {
    let fraction = if snapshot.duration.is_zero() {
        0.0
    } else {
        (snapshot.elapsed.as_secs_f64() / snapshot.duration.as_secs_f64()).clamp(0.0, 1.0)
    };
    let status = if snapshot.failure.is_some() {
        "FAILED"
    } else if snapshot.cancelled {
        "STOPPED"
    } else if snapshot.finished {
        "COMPLETE"
    } else {
        "RUNNING"
    };
    let color = if snapshot.failure.is_some() {
        Color::Red
    } else if snapshot.finished {
        Color::Green
    } else {
        Color::Cyan
    };
    let phase = snapshot
        .phase
        .as_deref()
        .map(|value| format!(" · {value}"))
        .unwrap_or_default();
    frame.render_widget(
        Gauge::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {} · {}{} ", snapshot.title, snapshot.mode, phase)),
            )
            .gauge_style(Style::default().fg(color).add_modifier(Modifier::BOLD))
            .ratio(fraction)
            .label(format!(
                " {status}  {:5.1}%  {} / {} ",
                fraction * 100.0,
                format_clock(snapshot.elapsed),
                format_clock(snapshot.duration)
            )),
        area,
    );
}

/// Renders the three workload-supplied metric panels.
fn draw_metrics(frame: &mut Frame, area: Rect, snapshot: &DashboardSnapshot) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(34),
            Constraint::Percentage(33),
        ])
        .split(area);
    for (area, panel_data) in columns.iter().zip(snapshot.panels.iter()) {
        let lines = panel_data
            .metrics
            .iter()
            .map(|metric| metric_line(metric))
            .collect::<Vec<_>>();
        frame.render_widget(panel(&panel_data.title, lines), *area);
    }
}

/// Renders the selected rolling series or a neutral empty series.
fn draw_series(
    frame: &mut Frame,
    area: Rect,
    snapshot: &DashboardSnapshot,
    selected_series: &AtomicUsize,
) {
    let selected = selected_series.load(Ordering::Relaxed);
    let series = snapshot.series.get(selected % snapshot.series.len().max(1));
    let (title, data) = match series {
        Some(series) => (
            series.title.clone(),
            if series.values.is_empty() {
                vec![0]
            } else {
                visible_series(&series.values, area.width.saturating_sub(2) as usize)
            },
        ),
        None => ("No measured series".to_string(), vec![0]),
    };
    frame.render_widget(
        Sparkline::default()
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} ")),
            )
            .data(&data)
            .style(Style::default().fg(Color::Cyan)),
        area,
    );
}

/// Selects the newest samples that fit inside the sparkline's bordered viewport.
fn visible_series(values: &[u64], width: usize) -> Vec<u64> {
    let visible = width.max(1).min(values.len());
    values[values.len().saturating_sub(visible)..].to_vec()
}

/// Renders the workload's current per-room health tiles.
fn draw_tiles(frame: &mut Frame, area: Rect, snapshot: &DashboardSnapshot) {
    let tiles_per_row = area.width.saturating_sub(2).max(1) as usize;
    let visible_rows = area.height.saturating_sub(2).max(1) as usize;
    let visible_tiles = snapshot.tiles.len().min(tiles_per_row * visible_rows);
    let mut lines = Vec::new();
    for chunk in snapshot.tiles[..visible_tiles].chunks(tiles_per_row) {
        lines.push(Line::from(
            chunk
                .iter()
                .map(|tile| Span::styled("█", Style::default().fg(health_color(tile.health))))
                .collect::<Vec<_>>(),
        ));
    }
    let hidden = snapshot.tiles.len().saturating_sub(visible_tiles);
    let visibility = if hidden == 0 {
        String::new()
    } else {
        format!(" · {hidden} off-screen")
    };
    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(format!(
                " Room activity — {} rooms{visibility} · {} ",
                snapshot.tiles.len(),
                snapshot.tile_legend
            )))
            .wrap(Wrap { trim: false }),
        area,
    );
}

/// Renders bounded recent events without participant content.
fn draw_events(frame: &mut Frame, area: Rect, snapshot: &DashboardSnapshot) {
    let lines = snapshot
        .events
        .iter()
        .take(area.height.saturating_sub(2) as usize)
        .map(|entry| Line::from(format!("• {entry}")))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Recent events "),
        ),
        area,
    );
}

/// Builds one aligned panel row from a metric measurement.
fn metric_line(metric: &DashboardMetric) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<21}  ", metric.label),
            Style::default().fg(Color::Gray),
        ),
        Span::styled(
            metric.value.clone(),
            Style::default()
                .fg(health_color(metric.health))
                .add_modifier(Modifier::BOLD),
        ),
    ])
}

/// Wraps metric rows in a standard bordered panel.
fn panel(title: &str, lines: Vec<Line<'static>>) -> Paragraph<'static> {
    Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .title(title.to_string()),
    )
}

/// Converts a workload health category into the shared palette.
fn health_color(health: DashboardHealth) -> Color {
    match health {
        DashboardHealth::Good => Color::Green,
        DashboardHealth::Warning => Color::Yellow,
        DashboardHealth::Error => Color::Red,
        DashboardHealth::Neutral => Color::Reset,
    }
}

/// Formats elapsed time consistently across stress-tool dashboards.
fn format_clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms a full sparkline viewport follows the newest samples.
    #[test]
    fn visible_series_scrolls_at_the_right_edge() {
        assert_eq!(visible_series(&[1, 2, 3, 4, 5], 3), vec![3, 4, 5]);
        assert_eq!(visible_series(&[1, 2], 5), vec![1, 2]);
        assert_eq!(visible_series(&[7], 0), vec![7]);
    }

    /// Keeps neutral metric values theme-aware and visibly separated from their labels.
    #[test]
    fn neutral_metric_line_uses_terminal_foreground_and_label_gap() {
        let line = metric_line(&DashboardMetric {
            label: "Messages/s / actions/s".to_string(),
            value: "12 / 34".to_string(),
            health: DashboardHealth::Neutral,
        });

        assert_eq!(line.spans[0].content, "Messages/s / actions/s  ");
        assert_eq!(line.spans[1].style.fg, Some(Color::Reset));
    }
}
