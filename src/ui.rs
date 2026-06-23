use std::io;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{Datelike, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Margin, Rect};
use ratatui::prelude::{Color, Frame, Line, Modifier, Span, Style, Text};
use ratatui::widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph, Wrap};

use crate::analytics::{
    activity_series, assignee_load, heatmap, priority_buckets, status_buckets, throughput_values,
    update_values,
};
use crate::app::{App, StatusKind, ViewMode};
use crate::cli::ThemeMode;
use crate::linear::{Issue, Priority, WorkflowStateType};

const TICK_RATE: Duration = Duration::from_millis(90);
const HEATMAP_WEEKS: usize = 16;
const SERIES_DAYS: usize = 35;

type TuiTerminal = Terminal<CrosstermBackend<io::Stdout>>;

#[derive(Clone, Copy, Debug)]
struct Palette {
    text: Color,
    muted: Color,
    label: Color,
}

impl Palette {
    fn from_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Auto => Self {
                text: Color::Reset,
                muted: Color::Gray,
                label: Color::DarkGray,
            },
            ThemeMode::Dark => Self {
                text: Color::White,
                muted: Color::Gray,
                label: Color::Gray,
            },
            ThemeMode::Light => Self {
                text: Color::Black,
                muted: Color::DarkGray,
                label: Color::DarkGray,
            },
        }
    }

    fn text_style(self) -> Style {
        Style::default().fg(self.text)
    }

    fn muted_style(self) -> Style {
        Style::default().fg(self.muted)
    }

    fn label_style(self) -> Style {
        Style::default().fg(self.label)
    }
}

pub async fn run(mut app: App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app).await;
    let restore_result = restore_terminal(&mut terminal);

    match (result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut last_tick = Instant::now();

    loop {
        terminal.draw(|frame| draw(frame, app))?;

        let timeout = TICK_RATE
            .checked_sub(last_tick.elapsed())
            .unwrap_or_default();
        if event::poll(timeout)? {
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if key.kind == KeyEventKind::Press && handle_key(app, key).await? {
                return Ok(());
            }
        }

        if last_tick.elapsed() >= TICK_RATE {
            app.tick();
            last_tick = Instant::now();
        }
    }
}

fn restore_terminal(terminal: &mut TuiTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

async fn handle_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    if app.editing_query {
        return handle_query_key(app, key);
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => return Ok(true),
        KeyCode::Char('1') => app.set_view(ViewMode::Dashboard),
        KeyCode::Char('2') => app.set_view(ViewMode::Issues),
        KeyCode::Char('3') => app.set_view(ViewMode::Heatmap),
        KeyCode::Char('4') => app.set_view(ViewMode::Charts),
        KeyCode::Char('?') => app.set_view(ViewMode::Help),
        KeyCode::Tab => app.next_view(),
        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.select_previous(),
        KeyCode::Home => app.select_first(),
        KeyCode::End => app.select_last(),
        KeyCode::Char('/') => app.begin_query_edit(),
        KeyCode::Char('x') => app.clear_query(),
        KeyCode::Char('t') => app.cycle_team_filter(),
        KeyCode::Char('T') => app.clear_team_filter(),
        KeyCode::Char('r') => app.refresh().await,
        KeyCode::Char('d') => app.toggle_demo().await,
        KeyCode::Char('m') => app.complete_selected().await,
        _ => {}
    }

    Ok(false)
}

fn handle_query_key(app: &mut App, key: KeyEvent) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.cancel_query_edit(),
        KeyCode::Enter => app.finish_query_edit(),
        KeyCode::Backspace => app.pop_query_char(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => app.clear_query(),
        KeyCode::Char(character) => app.push_query_char(character),
        _ => {}
    }

    Ok(false)
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let root = frame.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(root);

    draw_header(frame, rows[0], app);
    match app.view {
        ViewMode::Dashboard => draw_dashboard(frame, rows[1], app),
        ViewMode::Issues => draw_issues(frame, rows[1], app),
        ViewMode::Heatmap => draw_heatmap(frame, rows[1], app),
        ViewMode::Charts => draw_charts(frame, rows[1], app),
        ViewMode::Help => draw_help(frame, rows[1], app.theme),
    }
    draw_footer(frame, rows[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let palette = Palette::from_mode(app.theme);
    let spinner = ["-", "\\", "|", "/"][(app.tick as usize / 2) % 4];
    let viewer = app
        .snapshot
        .viewer
        .as_ref()
        .map(|viewer| viewer.name.as_str())
        .unwrap_or("offline");
    let org = app
        .snapshot
        .organization
        .as_ref()
        .map(|org| org.name.as_str())
        .unwrap_or("workspace");

    let tabs = [
        (ViewMode::Dashboard, "1 Dashboard"),
        (ViewMode::Issues, "2 Issues"),
        (ViewMode::Heatmap, "3 Heatmap"),
        (ViewMode::Charts, "4 Charts"),
        (ViewMode::Help, "? Help"),
    ];
    let mut tab_spans = Vec::with_capacity(tabs.len() * 2 + 4);
    tab_spans.push(Span::styled(
        " L-VIS ",
        Style::default().fg(Color::Black).bg(Color::Cyan),
    ));
    tab_spans.push(Span::styled(
        format!(" {spinner} "),
        Style::default().fg(Color::Magenta),
    ));
    for (index, (mode, label)) in tabs.into_iter().enumerate() {
        let style = if mode == app.view {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(chart_color(index))
        };
        tab_spans.push(Span::styled(format!(" {label} "), style));
    }

    let lines = vec![
        Line::from(tab_spans),
        Line::from(vec![
            Span::styled("Source ", palette.label_style()),
            Span::styled(app.source_label(), source_style(app)),
            Span::raw("  "),
            Span::styled("View ", palette.label_style()),
            Span::styled(app.view.label(), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled("Team ", palette.label_style()),
            Span::styled(app.team_filter_label(), palette.text_style()),
            Span::raw("  "),
            Span::styled("Workspace ", palette.label_style()),
            Span::styled(org, palette.text_style()),
            Span::raw("  "),
            Span::styled("Viewer ", palette.label_style()),
            Span::styled(viewer, palette.text_style()),
            Span::raw("  "),
            Span::styled("Fetched ", palette.label_style()),
            Span::styled(
                app.snapshot.fetched_at.format("%Y-%m-%d %H:%M").to_string(),
                palette.muted_style(),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn draw_dashboard(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(34),
            Constraint::Percentage(33),
            Constraint::Percentage(33),
        ])
        .split(area);

    draw_metrics_panel(frame, columns[0], app);
    draw_radar_panel(frame, columns[1], app);
    draw_focus_queue(frame, columns[2], app);
}

fn draw_metrics_panel(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let palette = Palette::from_mode(app.theme);
    let metrics = app.metrics();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(7),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(3),
        ])
        .split(area);

    let text = Text::from(vec![
        Line::from(vec![
            Span::styled("Total ", palette.label_style()),
            Span::styled(metrics.total.to_string(), Style::default().fg(Color::Cyan)),
            Span::raw("  "),
            Span::styled("Open ", palette.label_style()),
            Span::styled(metrics.open.to_string(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("Done ", palette.label_style()),
            Span::styled(metrics.done.to_string(), Style::default().fg(Color::Green)),
            Span::raw("  "),
            Span::styled("Overdue ", palette.label_style()),
            Span::styled(metrics.overdue.to_string(), Style::default().fg(Color::Red)),
        ]),
        Line::from(vec![
            Span::styled("Urgent ", palette.label_style()),
            Span::styled(
                metrics.urgent.to_string(),
                Style::default().fg(Color::LightRed),
            ),
            Span::raw("  "),
            Span::styled("Updated today ", palette.label_style()),
            Span::styled(
                metrics.updated_today.to_string(),
                Style::default().fg(Color::Blue),
            ),
        ]),
        Line::from(vec![
            Span::styled("Estimate ", palette.label_style()),
            Span::styled(
                metrics.total_estimate.to_string(),
                Style::default().fg(Color::Magenta),
            ),
            Span::raw("  "),
            Span::styled("Avg age ", palette.label_style()),
            Span::styled(
                format!("{}d", metrics.average_age_days),
                palette.muted_style(),
            ),
        ]),
    ]);
    frame.render_widget(
        Paragraph::new(text).block(Block::bordered().title("Workspace Pulse")),
        rows[0],
    );

    frame.render_widget(
        Gauge::default()
            .block(Block::bordered().title("Completion"))
            .gauge_style(Style::default().fg(Color::Green))
            .percent(metrics.completion_percent()),
        rows[1],
    );

    let overdue_percent = if metrics.total == 0 {
        0
    } else {
        ((metrics.overdue * 100) / metrics.total) as u16
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::bordered().title("Overdue Risk"))
            .gauge_style(Style::default().fg(risk_color(metrics.overdue)))
            .percent(overdue_percent),
        rows[2],
    );

    let visible = app.visible_issues();
    let loads = assignee_load(&visible, 6);
    let lines = loads
        .iter()
        .map(|load| {
            Line::from(vec![
                Span::styled(truncate(&load.name, 16), palette.text_style()),
                Span::raw(" "),
                Span::styled(bar(load.open, 12), Style::default().fg(Color::Cyan)),
                Span::raw(format!(" {}", load.open)),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("Load")),
        rows[3],
    );
}

fn draw_radar_panel(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let inner = area.inner(Margin {
        vertical: 1,
        horizontal: 1,
    });
    let mut lines = Vec::new();
    let height = inner.height.max(1) as usize;
    let width = inner.width.max(1) as usize;
    let center = height / 2;
    let phase = app.tick as f64 / 4.0;

    for row in 0..height {
        let mut text = String::with_capacity(width);
        for col in 0..width {
            let x = col as f64 / width.max(1) as f64;
            let wave = ((x * 18.0) + phase).sin();
            let y = center as f64 + wave * (height as f64 / 4.0);
            let distance = (row as f64 - y).abs();
            let sweep = (usize::try_from(app.tick / 2).unwrap_or(0) + row) % width.max(1);
            let character = if col == sweep {
                '|'
            } else if distance < 0.35 {
                '*'
            } else if distance < 0.75 {
                '+'
            } else if row == center {
                '.'
            } else {
                ' '
            };
            text.push(character);
        }
        let color = if row == center {
            Color::Cyan
        } else if row % 3 == 0 {
            Color::Magenta
        } else if row % 3 == 1 {
            Color::Blue
        } else {
            Color::Green
        };
        lines.push(Line::styled(text, Style::default().fg(color)));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Animated Activity Radar"))
            .alignment(Alignment::Center),
        area,
    );
}

fn draw_focus_queue(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let palette = Palette::from_mode(app.theme);
    let mut issues = app.visible_issues();
    issues.sort_by(|left, right| {
        left.priority
            .rank()
            .cmp(&right.priority.rank())
            .then(right.updated_at.cmp(&left.updated_at))
    });
    let items = issues
        .iter()
        .filter(|issue| !issue.is_done())
        .take(10)
        .map(|issue| {
            let style = priority_style(issue.priority);
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<8}", issue.identifier),
                    style.add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(truncate(&issue.title, 38), palette.text_style()),
            ]))
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        List::new(items)
            .block(Block::bordered().title("Focus Queue"))
            .highlight_symbol("> "),
        area,
    );
}

fn draw_issues(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let palette = Palette::from_mode(app.theme);
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);
    let visible = app.visible_issues();
    let items = visible
        .iter()
        .map(|issue| {
            let done_marker = if issue.is_done() { "x" } else { " " };
            let priority = issue.priority.rank();
            let assignee = issue
                .assignee
                .as_ref()
                .map(|user| user.display_label())
                .unwrap_or("-");
            ListItem::new(Line::from(vec![
                Span::styled(format!("[{done_marker}] "), state_style(&issue.state.kind)),
                Span::styled(
                    format!("{:<9}", issue.identifier),
                    priority_style(issue.priority),
                ),
                Span::styled(format!(" P{priority} "), palette.label_style()),
                Span::styled(truncate(&issue.title, 32), palette.text_style()),
                Span::styled(format!("  {assignee}"), palette.muted_style()),
            ]))
        })
        .collect::<Vec<_>>();

    let mut state = ListState::default();
    if !visible.is_empty() {
        state.select(Some(app.selected.min(visible.len() - 1)));
    }
    frame.render_stateful_widget(
        List::new(items)
            .block(Block::bordered().title(format!("Issues ({})", visible.len())))
            .highlight_style(Style::default().fg(Color::Black).bg(Color::Cyan))
            .highlight_symbol("> "),
        columns[0],
        &mut state,
    );

    draw_issue_detail(frame, columns[1], app.selected_issue(), app.theme);
}

fn draw_issue_detail(frame: &mut Frame<'_>, area: Rect, issue: Option<&Issue>, theme: ThemeMode) {
    let palette = Palette::from_mode(theme);
    let Some(issue) = issue else {
        frame.render_widget(
            Paragraph::new(Line::styled("No issue selected", palette.text_style()))
                .block(Block::bordered().title("Issue Detail")),
            area,
        );
        return;
    };

    let assignee = issue
        .assignee
        .as_ref()
        .map(|user| user.display_label())
        .unwrap_or("Unassigned");
    let due = issue
        .due_date
        .map(|date| date.to_string())
        .unwrap_or_else(|| "-".to_owned());
    let estimate = issue
        .estimate
        .map(|value| format!("{value:.0}"))
        .unwrap_or_else(|| "-".to_owned());
    let labels = if issue.labels.is_empty() {
        "-".to_owned()
    } else {
        issue
            .labels
            .iter()
            .map(|label| label.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut lines = vec![
        Line::styled(
            format!("{}  {}", issue.identifier, issue.title),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        labeled_line(
            "State",
            &issue.state.name,
            state_style(&issue.state.kind),
            palette,
        ),
        labeled_line(
            "Priority",
            issue.priority.name(),
            priority_style(issue.priority),
            palette,
        ),
        labeled_line(
            "Team",
            &format!("{} {}", issue.team.key, issue.team.name),
            palette.text_style(),
            palette,
        ),
        labeled_line("Assignee", assignee, palette.text_style(), palette),
        labeled_line("Due", &due, Style::default().fg(Color::Yellow), palette),
        labeled_line(
            "Estimate",
            &estimate,
            Style::default().fg(Color::Magenta),
            palette,
        ),
        labeled_line(
            "Labels",
            &labels,
            Style::default().fg(Color::Green),
            palette,
        ),
        labeled_line(
            "Updated",
            &issue.updated_at.format("%Y-%m-%d %H:%M").to_string(),
            palette.muted_style(),
            palette,
        ),
        Line::raw(""),
    ];

    if let Some(url) = issue.url.as_ref() {
        lines.push(labeled_line(
            "URL",
            url,
            Style::default().fg(Color::Blue),
            palette,
        ));
    }

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Issue Detail"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_heatmap(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let palette = Palette::from_mode(app.theme);
    let visible = app.visible_issues();
    let rows = heatmap(&visible, HEATMAP_WEEKS, Utc::now().date_naive());
    let max_count = rows
        .iter()
        .flat_map(|row| row.iter().map(|cell| cell.count))
        .max()
        .unwrap_or(1)
        .max(1);
    let month_labels = month_labels(&rows);
    let mut lines = vec![Line::from(month_labels)];
    let weekday_labels = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

    for (row_index, row) in rows.iter().enumerate() {
        let mut spans = vec![Span::styled(
            format!("{:<4}", weekday_labels[row_index]),
            palette.label_style(),
        )];
        for cell in row {
            spans.push(Span::styled(
                heatmap_symbol(cell.count),
                Style::default().fg(heatmap_color(cell.count, max_count)),
            ));
            spans.push(Span::raw(" "));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("Less ", palette.label_style()),
        Span::styled(". ", palette.label_style()),
        Span::styled("+ ", Style::default().fg(Color::Blue)),
        Span::styled("* ", Style::default().fg(Color::Cyan)),
        Span::styled("# ", Style::default().fg(Color::Green)),
        Span::styled("More", palette.label_style()),
    ]));

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Update Heatmap"))
            .alignment(Alignment::Left),
        area,
    );
}

fn draw_charts(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let palette = Palette::from_mode(app.theme);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Min(10),
        ])
        .split(area);
    let top = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[0]);
    let visible = app.visible_issues();

    draw_bucket_bars(
        frame,
        top[0],
        "Status",
        status_buckets(&visible),
        palette,
        0,
    );
    draw_bucket_bars(
        frame,
        top[1],
        "Priority",
        priority_buckets(&visible),
        palette,
        2,
    );

    let series = activity_series(&visible, SERIES_DAYS, Utc::now().date_naive());
    draw_activity_timeline(frame, rows[1], &series, palette);

    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[2]);
    draw_assignee_chart(frame, bottom[0], &visible, palette);
    draw_chart_summary(frame, bottom[1], app, palette);
}

fn draw_bucket_bars(
    frame: &mut Frame<'_>,
    area: Rect,
    title: &'static str,
    buckets: Vec<crate::analytics::Bucket>,
    palette: Palette,
    color_offset: usize,
) {
    let max_count = buckets
        .iter()
        .map(|bucket| bucket.count)
        .max()
        .unwrap_or(1)
        .max(1);
    let bar_width = usize::from(area.width).saturating_sub(24).clamp(8, 72);
    let lines = buckets
        .iter()
        .take(area.height.saturating_sub(2) as usize)
        .enumerate()
        .map(|(index, bucket)| {
            let (filled, empty) = scaled_bar(bucket.count, max_count, bar_width);
            Line::from(vec![
                Span::styled(
                    format!("{:<12}", truncate(&bucket.label, 12)),
                    palette.text_style(),
                ),
                Span::styled(format!("{:>4} ", bucket.count), palette.muted_style()),
                Span::styled(
                    filled,
                    Style::default().fg(chart_color(index + color_offset)),
                ),
                Span::styled(empty, palette.label_style()),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title(title))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_activity_timeline(
    frame: &mut Frame<'_>,
    area: Rect,
    series: &[crate::analytics::ActivityPoint],
    palette: Palette,
) {
    let width = usize::from(area.width).saturating_sub(18).clamp(12, 90);
    let created = series
        .iter()
        .map(|point| u64::try_from(point.created).unwrap_or(0))
        .collect::<Vec<_>>();
    let updated = update_values(series);
    let completed = throughput_values(series);
    let range = match (series.first(), series.last()) {
        (Some(first), Some(last)) => {
            format!(
                "{} to {}",
                first.date.format("%b %d"),
                last.date.format("%b %d")
            )
        }
        _ => "no activity".to_owned(),
    };
    let lines = vec![
        Line::from(vec![
            Span::styled("Range      ", palette.label_style()),
            Span::styled(range, palette.text_style()),
        ]),
        timeline_line("Created", &created, width, Color::Magenta, palette),
        timeline_line("Updated", &updated, width, Color::Cyan, palette),
        timeline_line("Done", &completed, width, Color::Green, palette),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Activity Timeline"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_assignee_chart(frame: &mut Frame<'_>, area: Rect, issues: &[Issue], palette: Palette) {
    let loads = assignee_load(issues, 8);
    let max_count = loads.iter().map(|load| load.open).max().unwrap_or(1).max(1);
    let bar_width = usize::from(area.width).saturating_sub(24).clamp(8, 72);
    let lines = loads
        .iter()
        .take(area.height.saturating_sub(2) as usize)
        .enumerate()
        .map(|(index, load)| {
            let (filled, empty) = scaled_bar(load.open, max_count, bar_width);
            Line::from(vec![
                Span::styled(
                    format!("{:<12}", truncate(&load.name, 12)),
                    palette.text_style(),
                ),
                Span::styled(format!("{:>4} ", load.open), palette.muted_style()),
                Span::styled(filled, Style::default().fg(chart_color(index + 4))),
                Span::styled(empty, palette.label_style()),
            ])
        })
        .collect::<Vec<_>>();

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Assignee Load"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_chart_summary(frame: &mut Frame<'_>, area: Rect, app: &App, palette: Palette) {
    let metrics = app.metrics();
    let lines = vec![
        Line::from(vec![
            Span::styled("Visible    ", palette.label_style()),
            Span::styled(metrics.total.to_string(), palette.text_style()),
        ]),
        Line::from(vec![
            Span::styled("Team       ", palette.label_style()),
            Span::styled(app.team_filter_label(), palette.text_style()),
        ]),
        Line::from(vec![
            Span::styled("Open       ", palette.label_style()),
            Span::styled(metrics.open.to_string(), Style::default().fg(Color::Yellow)),
        ]),
        Line::from(vec![
            Span::styled("Done       ", palette.label_style()),
            Span::styled(metrics.done.to_string(), Style::default().fg(Color::Green)),
        ]),
        Line::from(vec![
            Span::styled("Overdue    ", palette.label_style()),
            Span::styled(
                metrics.overdue.to_string(),
                Style::default().fg(risk_color(metrics.overdue)),
            ),
        ]),
        Line::from(vec![
            Span::styled("Avg age    ", palette.label_style()),
            Span::styled(
                format!("{}d", metrics.average_age_days),
                palette.text_style(),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title("Chart Scope")),
        area,
    );
}

fn timeline_line(
    label: &'static str,
    values: &[u64],
    width: usize,
    color: Color,
    palette: Palette,
) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10} "), palette.label_style()),
        Span::styled(sparkline_ascii(values, width), Style::default().fg(color)),
        Span::raw(" "),
        Span::styled(
            format!("{:>3}", values.iter().sum::<u64>()),
            palette.muted_style(),
        ),
    ])
}

fn scaled_bar(value: usize, max_value: usize, width: usize) -> (String, String) {
    let filled = if value == 0 {
        0
    } else {
        value.saturating_mul(width).div_ceil(max_value)
    }
    .min(width);
    ("#".repeat(filled), ".".repeat(width.saturating_sub(filled)))
}

fn sparkline_ascii(values: &[u64], width: usize) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }

    let values = downsample_max(values, width);
    let max_value = values.iter().copied().max().unwrap_or(1).max(1);
    values
        .iter()
        .map(|value| match *value {
            0 => '.',
            value => {
                let level = ((value * 5) / max_value).clamp(1, 5);
                match level {
                    1 => ':',
                    2 => '-',
                    3 => '=',
                    4 => '*',
                    _ => '#',
                }
            }
        })
        .collect()
}

fn downsample_max(values: &[u64], width: usize) -> Vec<u64> {
    if values.len() <= width {
        return values.to_vec();
    }

    let mut result = Vec::with_capacity(width);
    for index in 0..width {
        let start = index * values.len() / width;
        let end = ((index + 1) * values.len() / width).max(start + 1);
        let max_value = values[start..end.min(values.len())]
            .iter()
            .copied()
            .max()
            .unwrap_or(0);
        result.push(max_value);
    }
    result
}

fn chart_color(index: usize) -> Color {
    const COLORS: [Color; 8] = [
        Color::Cyan,
        Color::Blue,
        Color::Magenta,
        Color::Yellow,
        Color::Green,
        Color::LightRed,
        Color::LightBlue,
        Color::LightMagenta,
    ];
    COLORS[index % COLORS.len()]
}

fn draw_help(frame: &mut Frame<'_>, area: Rect, theme: ThemeMode) {
    let palette = Palette::from_mode(theme);
    let lines = vec![
        Line::styled(
            "Keys",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::raw(""),
        Line::styled(
            "1-4       switch dashboard, issues, heatmap, charts",
            palette.text_style(),
        ),
        Line::styled("tab       next view", palette.text_style()),
        Line::styled("j/k       move issue selection", palette.text_style()),
        Line::styled("/         filter visible issues", palette.text_style()),
        Line::styled("x         clear text filter", palette.text_style()),
        Line::styled("t         cycle team filter", palette.text_style()),
        Line::styled("T         clear team filter", palette.text_style()),
        Line::styled(
            "r         refresh Linear data or regenerate demo data",
            palette.text_style(),
        ),
        Line::styled(
            "m         move selected issue to the completed workflow state",
            palette.text_style(),
        ),
        Line::styled("d         toggle demo mode", palette.text_style()),
        Line::styled("q/esc     quit", palette.text_style()),
        Line::raw(""),
        Line::styled(
            "CLI",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Line::styled("l-vis sync", palette.text_style()),
        Line::styled("l-vis teams", palette.text_style()),
        Line::styled(
            "l-vis create --team ENG \"Fix issue\"",
            palette.text_style(),
        ),
        Line::styled("l-vis complete ENG-123", palette.text_style()),
    ];

    frame.render_widget(
        Paragraph::new(lines)
            .block(Block::bordered().title("Help"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let palette = Palette::from_mode(app.theme);
    let query = if app.query.is_empty() {
        "<none>".to_owned()
    } else {
        app.query.clone()
    };
    let query_style = if app.editing_query {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::Yellow)
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Status ", palette.label_style()),
            Span::styled(&app.status.text, status_style(app.status.kind)),
        ]),
        Line::from(vec![
            Span::styled("Text ", palette.label_style()),
            Span::styled(query, query_style),
            Span::raw("  "),
            Span::styled("Team ", palette.label_style()),
            Span::styled(app.team_filter_label(), palette.text_style()),
            Span::raw("  "),
            Span::styled("Keys ", palette.label_style()),
            Span::styled(
                "q quit  r refresh  / filter  t team  m complete  ? help",
                palette.muted_style(),
            ),
        ]),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::TOP)),
        area,
    );
}

fn labeled_line(label: &str, value: &str, value_style: Style, palette: Palette) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), palette.label_style()),
        Span::styled(value.to_owned(), value_style),
    ])
}

fn source_style(app: &App) -> Style {
    if app.is_demo() {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::Green)
    }
}

fn status_style(kind: StatusKind) -> Style {
    match kind {
        StatusKind::Info => Style::default().fg(Color::Cyan),
        StatusKind::Success => Style::default().fg(Color::Green),
        StatusKind::Warning => Style::default().fg(Color::Yellow),
        StatusKind::Error => Style::default().fg(Color::Red),
    }
}

fn state_style(kind: &WorkflowStateType) -> Style {
    match kind {
        WorkflowStateType::Backlog => Style::default().fg(Color::DarkGray),
        WorkflowStateType::Unstarted => Style::default().fg(Color::Blue),
        WorkflowStateType::Started => Style::default().fg(Color::Yellow),
        WorkflowStateType::Completed => Style::default().fg(Color::Green),
        WorkflowStateType::Canceled => Style::default().fg(Color::Red),
        WorkflowStateType::Unknown(_) => Style::default().fg(Color::Gray),
    }
}

fn priority_style(priority: Priority) -> Style {
    match priority {
        Priority::Urgent => Style::default().fg(Color::Red),
        Priority::High => Style::default().fg(Color::LightRed),
        Priority::Normal => Style::default().fg(Color::Yellow),
        Priority::Low => Style::default().fg(Color::Blue),
        Priority::None => Style::default().fg(Color::DarkGray),
        Priority::Unknown(_) => Style::default().fg(Color::Gray),
    }
}

fn risk_color(count: usize) -> Color {
    match count {
        0 => Color::Green,
        1..=3 => Color::Yellow,
        _ => Color::Red,
    }
}

fn heatmap_color(count: usize, max_count: usize) -> Color {
    if count == 0 {
        return Color::DarkGray;
    }

    let scaled = (count * 4) / max_count.max(1);
    match scaled {
        0 | 1 => Color::Blue,
        2 => Color::Cyan,
        3 => Color::Green,
        _ => Color::LightGreen,
    }
}

fn heatmap_symbol(count: usize) -> &'static str {
    match count {
        0 => ".",
        1 => "+",
        2..=3 => "*",
        _ => "#",
    }
}

fn month_labels(rows: &[Vec<crate::analytics::HeatmapCell>]) -> Vec<Span<'static>> {
    let mut spans = vec![Span::raw("    ")];
    let Some(first_row) = rows.first() else {
        return spans;
    };

    let mut previous_month = 0;
    for cell in first_row {
        let month = cell.date.month();
        if month != previous_month {
            spans.push(Span::styled(
                format!("{:<2}", cell.date.format("%b")),
                Style::default().fg(Color::DarkGray),
            ));
            previous_month = month;
        } else {
            spans.push(Span::raw("  "));
        }
    }
    spans
}

fn bar(count: usize, width: usize) -> String {
    let filled = count.min(width);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "#".repeat(filled), ".".repeat(empty))
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }

    let keep = max_chars.saturating_sub(1);
    let mut result = value.chars().take(keep).collect::<String>();
    result.push('~');
    result
}
