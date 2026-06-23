use chrono::Utc;

use crate::analytics::{Metrics, metrics};
use crate::cli::ThemeMode;
use crate::linear::{
    Issue, IssueLimit, LinearAuthConfig, LinearClient, SnapshotSource, WorkspaceSnapshot,
    demo_snapshot,
};
use crate::storage::IssueCache;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewMode {
    Dashboard,
    Issues,
    Heatmap,
    Charts,
    Help,
}

impl ViewMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dashboard => "Dashboard",
            Self::Issues => "Issues",
            Self::Heatmap => "Heatmap",
            Self::Charts => "Charts",
            Self::Help => "Help",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Dashboard => Self::Issues,
            Self::Issues => Self::Heatmap,
            Self::Heatmap => Self::Charts,
            Self::Charts | Self::Help => Self::Dashboard,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AppConfig {
    pub auth: Option<LinearAuthConfig>,
    pub demo: bool,
    pub limit: IssueLimit,
    pub theme: ThemeMode,
    pub team_filter: Option<String>,
}

#[derive(Clone, Debug)]
pub struct StatusMessage {
    pub text: String,
    pub kind: StatusKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug)]
pub struct App {
    client: Option<LinearClient>,
    cache: IssueCache,
    limit: IssueLimit,
    demo: bool,
    pub theme: ThemeMode,
    pub team_filter: Option<String>,
    pub snapshot: WorkspaceSnapshot,
    pub filtered: Vec<usize>,
    pub selected: usize,
    pub query: String,
    pub editing_query: bool,
    pub view: ViewMode,
    pub status: StatusMessage,
    pub tick: u64,
}

impl App {
    pub async fn bootstrap(config: AppConfig) -> anyhow::Result<Self> {
        let cache = IssueCache::new()?;
        let client = match config.auth {
            Some(auth) if !config.demo => Some(LinearClient::from_auth_config(auth).await?),
            _ => None,
        };

        let mut app = Self {
            client,
            cache,
            limit: config.limit,
            demo: config.demo,
            theme: config.theme,
            team_filter: normalize_filter(config.team_filter),
            snapshot: demo_snapshot(config.limit),
            filtered: Vec::new(),
            selected: 0,
            query: String::new(),
            editing_query: false,
            view: ViewMode::Dashboard,
            status: StatusMessage {
                text: "Starting l-vis".to_owned(),
                kind: StatusKind::Info,
            },
            tick: 0,
        };

        app.load_initial_snapshot().await;
        Ok(app)
    }

    pub fn metrics(&self) -> Metrics {
        let visible = self.visible_issues();
        metrics(&visible, Utc::now())
    }

    pub fn visible_issues(&self) -> Vec<Issue> {
        self.filtered
            .iter()
            .filter_map(|index| self.snapshot.issues.get(*index).cloned())
            .collect()
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        self.filtered
            .get(self.selected)
            .and_then(|index| self.snapshot.issues.get(*index))
    }

    pub fn source_label(&self) -> &'static str {
        self.snapshot.source.as_str()
    }

    pub fn team_filter_label(&self) -> &str {
        self.team_filter.as_deref().unwrap_or("all")
    }

    pub fn is_demo(&self) -> bool {
        self.demo || self.snapshot.source == SnapshotSource::Demo
    }

    pub fn tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }

    pub fn select_next(&mut self) {
        if self.filtered.is_empty() {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected + 1).min(self.filtered.len() - 1);
    }

    pub fn select_previous(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn select_first(&mut self) {
        self.selected = 0;
    }

    pub fn select_last(&mut self) {
        self.selected = self.filtered.len().saturating_sub(1);
    }

    pub fn begin_query_edit(&mut self) {
        self.editing_query = true;
        self.status = StatusMessage {
            text: "Filter by issue, state, team, assignee, or label".to_owned(),
            kind: StatusKind::Info,
        };
    }

    pub fn push_query_char(&mut self, character: char) {
        if !character.is_control() {
            self.query.push(character);
            self.apply_filter();
        }
    }

    pub fn pop_query_char(&mut self) {
        self.query.pop();
        self.apply_filter();
    }

    pub fn finish_query_edit(&mut self) {
        self.editing_query = false;
        self.status = StatusMessage {
            text: format!("Filter matched {} issues", self.filtered.len()),
            kind: StatusKind::Success,
        };
    }

    pub fn cancel_query_edit(&mut self) {
        self.editing_query = false;
    }

    pub fn clear_query(&mut self) {
        self.query.clear();
        self.apply_filter();
        self.status = StatusMessage {
            text: "Filter cleared".to_owned(),
            kind: StatusKind::Info,
        };
    }

    pub fn clear_team_filter(&mut self) {
        self.team_filter = None;
        self.apply_filter();
        self.status = StatusMessage {
            text: "Team filter cleared".to_owned(),
            kind: StatusKind::Info,
        };
    }

    pub fn cycle_team_filter(&mut self) {
        let teams = self.team_filter_options();
        if teams.is_empty() {
            self.status = StatusMessage {
                text: "No teams available to filter".to_owned(),
                kind: StatusKind::Warning,
            };
            return;
        }

        let next = match self.team_filter.as_deref() {
            None => Some(teams[0].clone()),
            Some(current) => {
                let current_index = teams
                    .iter()
                    .position(|team| team.eq_ignore_ascii_case(current));
                match current_index {
                    Some(index) if index + 1 < teams.len() => Some(teams[index + 1].clone()),
                    Some(_) => None,
                    None => Some(teams[0].clone()),
                }
            }
        };

        self.team_filter = next;
        self.apply_filter();
        self.status = StatusMessage {
            text: format!("Team filter: {}", self.team_filter_label()),
            kind: StatusKind::Info,
        };
    }

    pub fn set_view(&mut self, view: ViewMode) {
        self.view = view;
    }

    pub fn next_view(&mut self) {
        self.view = self.view.next();
    }

    pub async fn refresh(&mut self) {
        if self.demo {
            self.replace_snapshot(demo_snapshot(self.limit));
            self.status = StatusMessage {
                text: "Regenerated demo workspace".to_owned(),
                kind: StatusKind::Success,
            };
            return;
        }

        let Some(client) = self.client.as_ref() else {
            self.replace_snapshot(demo_snapshot(self.limit));
            self.demo = true;
            self.status = StatusMessage {
                text: "No Linear key found; switched to demo mode".to_owned(),
                kind: StatusKind::Warning,
            };
            return;
        };

        match client.fetch_workspace(self.limit).await {
            Ok(snapshot) => {
                let issue_count = snapshot.issues.len();
                if let Err(error) = self.cache.save(&snapshot) {
                    self.status = StatusMessage {
                        text: format!("Fetched {issue_count} issues but cache failed: {error}"),
                        kind: StatusKind::Warning,
                    };
                } else {
                    self.status = StatusMessage {
                        text: format!("Fetched {issue_count} issues from Linear"),
                        kind: StatusKind::Success,
                    };
                }
                self.replace_snapshot(snapshot);
            }
            Err(error) => {
                self.status = StatusMessage {
                    text: format!("Refresh failed: {error}"),
                    kind: StatusKind::Error,
                };
            }
        }
    }

    pub async fn toggle_demo(&mut self) {
        self.demo = !self.demo;
        if self.demo {
            self.replace_snapshot(demo_snapshot(self.limit));
            self.status = StatusMessage {
                text: "Demo mode enabled".to_owned(),
                kind: StatusKind::Info,
            };
        } else {
            self.status = StatusMessage {
                text: "Linear mode enabled".to_owned(),
                kind: StatusKind::Info,
            };
            self.refresh().await;
        }
    }

    pub async fn complete_selected(&mut self) {
        let Some(issue) = self.selected_issue().cloned() else {
            self.status = StatusMessage {
                text: "No issue selected".to_owned(),
                kind: StatusKind::Warning,
            };
            return;
        };

        if issue.is_done() {
            self.status = StatusMessage {
                text: format!("{} is already done", issue.identifier),
                kind: StatusKind::Info,
            };
            return;
        }

        let Some(client) = self.client.as_ref() else {
            self.status = StatusMessage {
                text: "Completing issues needs LINEAR_API_KEY".to_owned(),
                kind: StatusKind::Warning,
            };
            return;
        };

        match client.complete_issue(&issue.id).await {
            Ok(updated) => {
                self.replace_issue(updated.clone());
                self.status = StatusMessage {
                    text: format!("Moved {} to {}", updated.identifier, updated.state.name),
                    kind: StatusKind::Success,
                };
            }
            Err(error) => {
                self.status = StatusMessage {
                    text: format!("Could not complete {}: {error}", issue.identifier),
                    kind: StatusKind::Error,
                };
            }
        }
    }

    async fn load_initial_snapshot(&mut self) {
        if self.demo {
            self.replace_snapshot(demo_snapshot(self.limit));
            self.status = StatusMessage {
                text: "Demo mode loaded".to_owned(),
                kind: StatusKind::Info,
            };
            return;
        }

        if self.client.is_some() {
            self.refresh().await;
            if self.snapshot.source == SnapshotSource::Linear {
                return;
            }
        }

        match self.cache.load() {
            Ok(Some(snapshot)) => {
                self.replace_snapshot(snapshot);
                self.status = StatusMessage {
                    text: "Loaded cached Linear issues".to_owned(),
                    kind: StatusKind::Warning,
                };
            }
            Ok(None) => {
                self.demo = true;
                self.replace_snapshot(demo_snapshot(self.limit));
                self.status = StatusMessage {
                    text: "No Linear cache available; loaded demo workspace".to_owned(),
                    kind: StatusKind::Warning,
                };
            }
            Err(error) => {
                self.demo = true;
                self.replace_snapshot(demo_snapshot(self.limit));
                self.status = StatusMessage {
                    text: format!("Cache failed, loaded demo workspace: {error}"),
                    kind: StatusKind::Warning,
                };
            }
        }
    }

    fn replace_snapshot(&mut self, snapshot: WorkspaceSnapshot) {
        self.snapshot = snapshot;
        self.apply_filter();
    }

    fn replace_issue(&mut self, issue: Issue) {
        if let Some(existing) = self
            .snapshot
            .issues
            .iter_mut()
            .find(|candidate| candidate.id == issue.id)
        {
            *existing = issue;
        }
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let query = self.query.trim().to_ascii_lowercase();
        self.filtered.clear();

        for (index, issue) in self.snapshot.issues.iter().enumerate() {
            let matches_query = query.is_empty() || issue.searchable_text().contains(&query);
            let matches_team = self
                .team_filter
                .as_ref()
                .map(|team_filter| issue_matches_team(issue, team_filter))
                .unwrap_or(true);

            if matches_query && matches_team {
                self.filtered.push(index);
            }
        }

        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    fn team_filter_options(&self) -> Vec<String> {
        let mut teams = self
            .snapshot
            .issues
            .iter()
            .map(|issue| issue.team.key.clone())
            .collect::<Vec<_>>();
        teams.sort();
        teams.dedup();
        teams
    }
}

fn normalize_filter(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn issue_matches_team(issue: &Issue, team_filter: &str) -> bool {
    issue.team.key.eq_ignore_ascii_case(team_filter)
        || issue.team.name.eq_ignore_ascii_case(team_filter)
        || issue.team.id.eq_ignore_ascii_case(team_filter)
}

#[cfg(test)]
mod tests {
    use crate::cli::ThemeMode;
    use crate::linear::IssueLimit;

    use super::{App, AppConfig};

    #[tokio::test]
    async fn team_filter_limits_visible_issue_set() {
        let app = App::bootstrap(AppConfig {
            auth: None,
            demo: true,
            limit: IssueLimit::new(30).expect("valid test limit"),
            theme: ThemeMode::Auto,
            team_filter: Some("PLT".to_owned()),
        })
        .await
        .expect("demo app should start");

        let visible = app.visible_issues();
        assert!(!visible.is_empty());
        assert!(visible.len() < app.snapshot.issues.len());
        assert!(visible.iter().all(|issue| issue.team.key == "PLT"));
    }

    #[tokio::test]
    async fn cycling_team_filter_changes_visible_issue_set() {
        let mut app = App::bootstrap(AppConfig {
            auth: None,
            demo: true,
            limit: IssueLimit::new(30).expect("valid test limit"),
            theme: ThemeMode::Auto,
            team_filter: None,
        })
        .await
        .expect("demo app should start");

        let all_count = app.visible_issues().len();
        app.cycle_team_filter();

        assert_ne!(app.team_filter_label(), "all");
        assert!(app.visible_issues().len() < all_count);

        app.clear_team_filter();
        assert_eq!(app.team_filter_label(), "all");
        assert_eq!(app.visible_issues().len(), all_count);
    }
}
