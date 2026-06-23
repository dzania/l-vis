use std::collections::BTreeMap;

use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

use crate::linear::{Issue, Priority, WorkflowStateType};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Metrics {
    pub total: usize,
    pub open: usize,
    pub done: usize,
    pub overdue: usize,
    pub urgent: usize,
    pub updated_today: usize,
    pub total_estimate: u32,
    pub average_age_days: u32,
}

impl Metrics {
    pub fn completion_percent(&self) -> u16 {
        if self.total == 0 {
            return 0;
        }

        ((self.done * 100) / self.total) as u16
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Bucket {
    pub label: String,
    pub count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivityPoint {
    pub date: NaiveDate,
    pub created: usize,
    pub updated: usize,
    pub completed: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeatmapCell {
    pub date: NaiveDate,
    pub count: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssigneeLoad {
    pub name: String,
    pub open: usize,
}

pub fn metrics(issues: &[Issue], now: DateTime<Utc>) -> Metrics {
    let today = now.date_naive();
    let mut total_age_days = 0_u64;
    let mut total_estimate = 0_u32;
    let mut result = Metrics {
        total: issues.len(),
        ..Metrics::default()
    };

    for issue in issues {
        if issue.is_done() {
            result.done += 1;
        } else {
            result.open += 1;
        }

        if issue.is_overdue(today) {
            result.overdue += 1;
        }

        if issue.priority == Priority::Urgent {
            result.urgent += 1;
        }

        if issue.updated_at.date_naive() == today {
            result.updated_today += 1;
        }

        total_age_days += now
            .signed_duration_since(issue.created_at)
            .num_days()
            .max(0) as u64;
        total_estimate += issue.estimate.unwrap_or(0.0).max(0.0).round() as u32;
    }

    result.total_estimate = total_estimate;
    result.average_age_days = if issues.is_empty() {
        0
    } else {
        (total_age_days / u64::try_from(issues.len()).unwrap_or(1)) as u32
    };
    result
}

pub fn status_buckets(issues: &[Issue]) -> Vec<Bucket> {
    let mut counts = BTreeMap::<String, usize>::new();
    for issue in issues {
        *counts.entry(issue.state.name.clone()).or_default() += 1;
    }

    let mut buckets = counts
        .into_iter()
        .map(|(label, count)| Bucket { label, count })
        .collect::<Vec<_>>();
    buckets.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then(left.label.cmp(&right.label))
    });
    buckets
}

pub fn priority_buckets(issues: &[Issue]) -> Vec<Bucket> {
    let priorities = [
        Priority::Urgent,
        Priority::High,
        Priority::Normal,
        Priority::Low,
        Priority::None,
    ];
    let mut buckets = Vec::with_capacity(priorities.len());

    for priority in priorities {
        let count = issues
            .iter()
            .filter(|issue| issue.priority == priority)
            .count();
        buckets.push(Bucket {
            label: priority.name().to_owned(),
            count,
        });
    }

    buckets
}

pub fn activity_series(issues: &[Issue], days: usize, today: NaiveDate) -> Vec<ActivityPoint> {
    let days = days.clamp(1, 90);
    let mut points = Vec::with_capacity(days);

    for offset in (0..days).rev() {
        let date = today - Duration::days(i64::try_from(offset).unwrap_or(0));
        let mut point = ActivityPoint {
            date,
            created: 0,
            updated: 0,
            completed: 0,
        };

        for issue in issues {
            if issue.created_at.date_naive() == date {
                point.created += 1;
            }
            if issue.updated_at.date_naive() == date {
                point.updated += 1;
            }
            if issue
                .completed_at
                .map(|completed_at| completed_at.date_naive() == date)
                .unwrap_or(false)
            {
                point.completed += 1;
            }
        }

        points.push(point);
    }

    points
}

pub fn heatmap(issues: &[Issue], weeks: usize, today: NaiveDate) -> Vec<Vec<HeatmapCell>> {
    let weeks = weeks.clamp(1, 26);
    let days = weeks * 7;
    let start = today - Duration::days(i64::try_from(days - 1).unwrap_or(0));
    let mut rows = (0..7)
        .map(|_| Vec::with_capacity(weeks))
        .collect::<Vec<_>>();

    for day_offset in 0..days {
        let date = start + Duration::days(i64::try_from(day_offset).unwrap_or(0));
        let count = issues
            .iter()
            .filter(|issue| issue.updated_at.date_naive() == date)
            .count();
        let weekday = date.weekday().num_days_from_monday() as usize;
        rows[weekday].push(HeatmapCell { date, count });
    }

    rows
}

pub fn assignee_load(issues: &[Issue], limit: usize) -> Vec<AssigneeLoad> {
    let mut counts = BTreeMap::<String, usize>::new();
    for issue in issues {
        if issue.state.kind == WorkflowStateType::Completed {
            continue;
        }
        let assignee = issue
            .assignee
            .as_ref()
            .map(|user| user.display_label().to_owned())
            .unwrap_or_else(|| "Unassigned".to_owned());
        *counts.entry(assignee).or_default() += 1;
    }

    let mut loads = counts
        .into_iter()
        .map(|(name, open)| AssigneeLoad { name, open })
        .collect::<Vec<_>>();
    loads.sort_by(|left, right| right.open.cmp(&left.open).then(left.name.cmp(&right.name)));
    loads.truncate(limit);
    loads
}

pub fn throughput_values(series: &[ActivityPoint]) -> Vec<u64> {
    series
        .iter()
        .map(|point| u64::try_from(point.completed).unwrap_or(0))
        .collect()
}

pub fn update_values(series: &[ActivityPoint]) -> Vec<u64> {
    series
        .iter()
        .map(|point| u64::try_from(point.updated).unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use crate::linear::{IssueLimit, demo_snapshot};

    use super::{activity_series, heatmap, metrics};

    #[test]
    fn demo_metrics_have_expected_shape() {
        let snapshot = demo_snapshot(IssueLimit::new(20).expect("valid test limit"));
        let result = metrics(&snapshot.issues, Utc::now());

        assert_eq!(result.total, 20);
        assert!(result.open > 0);
        assert!(result.total_estimate > 0);
    }

    #[test]
    fn heatmap_respects_requested_weeks() {
        let snapshot = demo_snapshot(IssueLimit::new(20).expect("valid test limit"));
        let rows = heatmap(&snapshot.issues, 12, Utc::now().date_naive());

        assert_eq!(rows.len(), 7);
        assert!(rows.iter().all(|row| row.len() == 12));
    }

    #[test]
    fn activity_series_is_bounded() {
        let snapshot = demo_snapshot(IssueLimit::new(20).expect("valid test limit"));
        let series = activity_series(&snapshot.issues, 14, Utc::now().date_naive());

        assert_eq!(series.len(), 14);
    }
}
