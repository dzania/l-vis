use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::num::NonZeroUsize;

use chrono::{DateTime, Duration, NaiveDate, Utc};
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const LINEAR_GRAPHQL_ENDPOINT: &str = "https://api.linear.app/graphql";
const LINEAR_OAUTH_TOKEN_ENDPOINT: &str = "https://api.linear.app/oauth/token";
const PAGE_SIZE_MAX: usize = 100;
const ISSUE_LIMIT_MAX: usize = 250;
const DEMO_ISSUES_DEFAULT: usize = 96;

const ISSUES_QUERY: &str = r#"
query LVisIssues($first: Int!, $after: String) {
  viewer {
    id
    name
    email
  }
  organization {
    id
    name
    urlKey
  }
  issues(first: $first, after: $after, orderBy: updatedAt) {
    nodes {
      id
      identifier
      title
      priority
      estimate
      createdAt
      updatedAt
      completedAt
      dueDate
      url
      team {
        id
        key
        name
      }
      state {
        id
        name
        color
        type
      }
      assignee {
        id
        name
        displayName
        email
      }
      labels(first: 8) {
        nodes {
          id
          name
          color
        }
      }
    }
    pageInfo {
      hasNextPage
      endCursor
    }
  }
}
"#;

const VIEWER_QUERY: &str = r#"
query LVisViewer {
  viewer {
    id
    name
    email
  }
}
"#;

const TEAMS_QUERY: &str = r#"
query LVisTeams {
  teams(first: 100) {
    nodes {
      id
      key
      name
    }
  }
}
"#;

const ISSUE_LOOKUP_QUERY: &str = r#"
query LVisIssue($id: String!) {
  issue(id: $id) {
    id
    identifier
    title
    priority
    estimate
    createdAt
    updatedAt
    completedAt
    dueDate
    url
    team {
      id
      key
      name
      states(first: 100) {
        nodes {
          id
          name
          color
          type
        }
      }
    }
    state {
      id
      name
      color
      type
    }
    assignee {
      id
      name
      displayName
      email
    }
    labels(first: 8) {
      nodes {
        id
        name
        color
      }
    }
  }
}
"#;

const ISSUE_CREATE_MUTATION: &str = r#"
mutation LVisCreateIssue($input: IssueCreateInput!) {
  issueCreate(input: $input) {
    success
    issue {
      id
      identifier
      title
      priority
      estimate
      createdAt
      updatedAt
      completedAt
      dueDate
      url
      team {
        id
        key
        name
      }
      state {
        id
        name
        color
        type
      }
      assignee {
        id
        name
        displayName
        email
      }
      labels(first: 8) {
        nodes {
          id
          name
          color
        }
      }
    }
  }
}
"#;

const ISSUE_UPDATE_MUTATION: &str = r#"
mutation LVisUpdateIssue($id: String!, $input: IssueUpdateInput!) {
  issueUpdate(id: $id, input: $input) {
    success
    issue {
      id
      identifier
      title
      priority
      estimate
      createdAt
      updatedAt
      completedAt
      dueDate
      url
      team {
        id
        key
        name
      }
      state {
        id
        name
        color
        type
      }
      assignee {
        id
        name
        displayName
        email
      }
      labels(first: 8) {
        nodes {
          id
          name
          color
        }
      }
    }
  }
}
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IssueLimit(NonZeroUsize);

impl IssueLimit {
    pub fn new(value: usize) -> Result<Self, IssueLimitError> {
        let non_zero = NonZeroUsize::new(value).ok_or(IssueLimitError::Zero)?;
        if value > ISSUE_LIMIT_MAX {
            return Err(IssueLimitError::TooLarge {
                value,
                max: ISSUE_LIMIT_MAX,
            });
        }

        Ok(Self(non_zero))
    }

    pub fn get(self) -> usize {
        self.0.get()
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum IssueLimitError {
    Zero,
    TooLarge { value: usize, max: usize },
}

impl Display for IssueLimitError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => write!(formatter, "issue limit must be at least 1"),
            Self::TooLarge { value, max } => {
                write!(formatter, "issue limit {value} exceeds maximum {max}")
            }
        }
    }
}

impl Error for IssueLimitError {}

#[derive(Clone, Debug)]
pub struct LinearClient {
    http: reqwest::Client,
    authorization: String,
}

impl LinearClient {
    pub async fn from_auth_config(config: LinearAuthConfig) -> Result<Self, LinearError> {
        match config {
            LinearAuthConfig::ApiKey(api_key) => Self::new(LinearAuth::ApiKey(api_key)),
            LinearAuthConfig::OAuthAccessToken(access_token) => {
                Self::new(LinearAuth::OAuthAccessToken(access_token))
            }
            LinearAuthConfig::OAuthRefreshToken(config) => {
                let token = Self::refresh_oauth_access_token(&config).await?;
                Self::new(LinearAuth::OAuthAccessToken(token.access_token))
            }
        }
    }

    pub fn new(auth: LinearAuth) -> Result<Self, LinearError> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|source| LinearError::Transport { source })?;
        let authorization = auth.authorization_header_value()?;

        Ok(Self {
            http,
            authorization,
        })
    }

    pub async fn refresh_oauth_access_token(
        config: &OAuthRefreshConfig,
    ) -> Result<OAuthToken, LinearError> {
        let client = reqwest::Client::new();
        let response = client
            .post(LINEAR_OAUTH_TOKEN_ENDPOINT)
            .form(&OAuthRefreshForm {
                refresh_token: config.refresh_token.as_str(),
                grant_type: "refresh_token",
                client_id: config.client_id.as_str(),
                client_secret: config.client_secret.as_deref(),
            })
            .send()
            .await
            .map_err(|source| LinearError::Transport { source })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|source| LinearError::Transport { source })?;

        if !status.is_success() {
            return Err(LinearError::HttpStatus { status, body });
        }

        let token: OAuthToken =
            serde_json::from_str(&body).map_err(|source| LinearError::Decode { source })?;

        if token.access_token.trim().is_empty() {
            return Err(LinearError::Protocol {
                message: "OAuth token response did not include an access token",
            });
        }
        if !token.token_type.eq_ignore_ascii_case("bearer") {
            return Err(LinearError::Protocol {
                message: "OAuth token response was not a Bearer token",
            });
        }

        Ok(token)
    }

    pub async fn fetch_workspace(
        &self,
        limit: IssueLimit,
    ) -> Result<WorkspaceSnapshot, LinearError> {
        let mut issues = Vec::with_capacity(limit.get());
        let mut after = None;
        let mut viewer: Option<Viewer> = None;
        let mut organization: Option<Organization> = None;

        while issues.len() < limit.get() {
            let remaining = limit.get() - issues.len();
            let first = remaining.min(PAGE_SIZE_MAX);
            let data: IssuesData = self
                .request(
                    ISSUES_QUERY,
                    IssuesVariables {
                        first: i32::try_from(first).map_err(|_| LinearError::Protocol {
                            message: "page size does not fit GraphQL Int",
                        })?,
                        after,
                    },
                )
                .await?;

            if viewer.is_none() {
                viewer = data.viewer.map(Into::into);
            }
            if organization.is_none() {
                organization = data.organization.map(Into::into);
            }

            issues.extend(data.issues.nodes.into_iter().map(Into::into));

            if !data.issues.page_info.has_next_page {
                break;
            }

            after = data.issues.page_info.end_cursor;
            if after.is_none() {
                break;
            }
        }

        Ok(WorkspaceSnapshot {
            fetched_at: Utc::now(),
            source: SnapshotSource::Linear,
            viewer,
            organization,
            issues,
        })
    }

    pub async fn fetch_viewer(&self) -> Result<Viewer, LinearError> {
        let data: ViewerData = self.request(VIEWER_QUERY, EmptyVariables {}).await?;
        Ok(data.viewer.into())
    }

    pub async fn fetch_teams(&self) -> Result<Vec<Team>, LinearError> {
        let data: TeamsData = self.request(TEAMS_QUERY, EmptyVariables {}).await?;
        Ok(data.teams.nodes.into_iter().map(Into::into).collect())
    }

    pub async fn create_issue(&self, input: CreateIssueInput) -> Result<Issue, LinearError> {
        let team_id = self.resolve_team_id(&input.team).await?;
        let data: IssueCreateData = self
            .request(
                ISSUE_CREATE_MUTATION,
                IssueCreateVariables {
                    input: IssueCreateInput {
                        team_id,
                        title: input.title,
                        description: input.description,
                        priority: input.priority,
                    },
                },
            )
            .await?;

        if !data.issue_create.success {
            return Err(LinearError::Protocol {
                message: "Linear returned success=false for issueCreate",
            });
        }

        data.issue_create
            .issue
            .map(Into::into)
            .ok_or(LinearError::Protocol {
                message: "issueCreate did not return an issue",
            })
    }

    pub async fn complete_issue(&self, issue_id: &str) -> Result<Issue, LinearError> {
        let issue = self.fetch_issue(issue_id).await?;
        if issue.state.kind.is_done() {
            return Ok(issue);
        }

        let state_id = issue
            .team
            .states
            .iter()
            .find(|state| state.kind == WorkflowStateType::Completed)
            .or_else(|| {
                issue.team.states.iter().find(|state| {
                    let name = state.name.to_ascii_lowercase();
                    name == "done" || name == "completed" || name == "complete"
                })
            })
            .map(|state| state.id.clone())
            .ok_or(LinearError::Protocol {
                message: "could not find a completed workflow state for the issue team",
            })?;

        self.update_issue_state(&issue.id, state_id).await
    }

    async fn fetch_issue(&self, issue_id: &str) -> Result<Issue, LinearError> {
        let data: IssueLookupData = self
            .request(
                ISSUE_LOOKUP_QUERY,
                IssueLookupVariables {
                    id: issue_id.to_owned(),
                },
            )
            .await?;

        data.issue.map(Into::into).ok_or(LinearError::Protocol {
            message: "Linear did not return an issue for that identifier",
        })
    }

    async fn update_issue_state(
        &self,
        issue_id: &str,
        state_id: String,
    ) -> Result<Issue, LinearError> {
        let data: IssueUpdateData = self
            .request(
                ISSUE_UPDATE_MUTATION,
                IssueUpdateVariables {
                    id: issue_id.to_owned(),
                    input: IssueUpdateInput { state_id },
                },
            )
            .await?;

        if !data.issue_update.success {
            return Err(LinearError::Protocol {
                message: "Linear returned success=false for issueUpdate",
            });
        }

        data.issue_update
            .issue
            .map(Into::into)
            .ok_or(LinearError::Protocol {
                message: "issueUpdate did not return an issue",
            })
    }

    async fn resolve_team_id(&self, team: &str) -> Result<String, LinearError> {
        if looks_like_uuid(team) {
            return Ok(team.to_owned());
        }

        let team_key = team.trim().to_ascii_uppercase();
        self.fetch_teams()
            .await?
            .into_iter()
            .find(|candidate| candidate.key.eq_ignore_ascii_case(&team_key))
            .map(|candidate| candidate.id)
            .ok_or(LinearError::Protocol {
                message: "team key was not found for this token",
            })
    }

    async fn request<T, V>(&self, query: &'static str, variables: V) -> Result<T, LinearError>
    where
        T: DeserializeOwned,
        V: Serialize,
    {
        let response = self
            .http
            .post(LINEAR_GRAPHQL_ENDPOINT)
            .header(AUTHORIZATION, self.authorization.as_str())
            .json(&GraphQlRequest { query, variables })
            .send()
            .await
            .map_err(|source| LinearError::Transport { source })?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|source| LinearError::Transport { source })?;

        if !status.is_success() {
            return Err(LinearError::HttpStatus { status, body });
        }

        let payload: GraphQlResponse<T> =
            serde_json::from_str(&body).map_err(|source| LinearError::Decode { source })?;

        if !payload.errors.is_empty() {
            return Err(LinearError::GraphQl {
                errors: payload.errors,
            });
        }

        payload.data.ok_or(LinearError::Protocol {
            message: "GraphQL response did not include data",
        })
    }
}

#[derive(Debug)]
pub enum LinearError {
    Transport { source: reqwest::Error },
    HttpStatus { status: StatusCode, body: String },
    Decode { source: serde_json::Error },
    GraphQl { errors: Vec<GraphQlError> },
    Protocol { message: &'static str },
    InvalidAuth { message: &'static str },
}

impl Display for LinearError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport { source } => write!(formatter, "Linear request failed: {source}"),
            Self::HttpStatus { status, body } => {
                write!(
                    formatter,
                    "Linear returned HTTP {status}: {}",
                    trim_body(body)
                )
            }
            Self::Decode { source } => {
                write!(formatter, "failed to decode Linear response: {source}")
            }
            Self::GraphQl { errors } => {
                write!(
                    formatter,
                    "Linear GraphQL error: {}",
                    format_graphql_errors(errors)
                )
            }
            Self::Protocol { message } => write!(formatter, "Linear protocol error: {message}"),
            Self::InvalidAuth { message } => write!(formatter, "invalid Linear auth: {message}"),
        }
    }
}

impl Error for LinearError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Transport { source } => Some(source),
            Self::Decode { source } => Some(source),
            Self::HttpStatus { .. }
            | Self::GraphQl { .. }
            | Self::Protocol { .. }
            | Self::InvalidAuth { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinearAuthConfig {
    ApiKey(String),
    OAuthAccessToken(String),
    OAuthRefreshToken(OAuthRefreshConfig),
}

impl LinearAuthConfig {
    pub fn label(&self) -> &'static str {
        match self {
            Self::ApiKey(_) => "personal API key",
            Self::OAuthAccessToken(_) => "OAuth access token",
            Self::OAuthRefreshToken(_) => "OAuth refresh token",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OAuthRefreshConfig {
    pub refresh_token: String,
    pub client_id: String,
    pub client_secret: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LinearAuth {
    ApiKey(String),
    OAuthAccessToken(String),
}

impl LinearAuth {
    fn authorization_header_value(&self) -> Result<String, LinearError> {
        match self {
            Self::ApiKey(api_key) => {
                let api_key = api_key.trim();
                if api_key.is_empty() {
                    return Err(LinearError::InvalidAuth {
                        message: "personal API key is empty",
                    });
                }
                Ok(api_key.to_owned())
            }
            Self::OAuthAccessToken(access_token) => {
                let access_token = normalize_oauth_access_token(access_token);
                if access_token.is_empty() {
                    return Err(LinearError::InvalidAuth {
                        message: "OAuth access token is empty",
                    });
                }
                Ok(format!("Bearer {access_token}"))
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: i64,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    pub fetched_at: DateTime<Utc>,
    pub source: SnapshotSource,
    pub viewer: Option<Viewer>,
    pub organization: Option<Organization>,
    pub issues: Vec<Issue>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SnapshotSource {
    Linear,
    Cache,
    Demo,
}

impl SnapshotSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Cache => "cache",
            Self::Demo => "demo",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Viewer {
    pub id: String,
    pub name: String,
    pub email: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Organization {
    pub id: String,
    pub name: String,
    pub url_key: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub priority: Priority,
    pub estimate: Option<f64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub due_date: Option<NaiveDate>,
    pub url: Option<String>,
    pub team: Team,
    pub state: WorkflowState,
    pub assignee: Option<User>,
    pub labels: Vec<Label>,
}

impl Issue {
    pub fn is_done(&self) -> bool {
        self.completed_at.is_some() || self.state.kind.is_done()
    }

    pub fn is_overdue(&self, today: NaiveDate) -> bool {
        self.due_date
            .map(|due_date| due_date < today && !self.is_done())
            .unwrap_or(false)
    }

    pub fn searchable_text(&self) -> String {
        let assignee = self
            .assignee
            .as_ref()
            .map(|user| user.name.as_str())
            .unwrap_or("unassigned");
        let mut text = format!(
            "{} {} {} {} {} {}",
            self.identifier, self.title, self.team.key, self.team.name, self.state.name, assignee
        );

        for label in &self.labels {
            text.push(' ');
            text.push_str(&label.name);
        }

        text.to_ascii_lowercase()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub key: String,
    pub name: String,
    pub states: Vec<WorkflowState>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub display_name: Option<String>,
    pub email: String,
}

impl User {
    pub fn display_label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(self.name.as_str())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkflowState {
    pub id: String,
    pub name: String,
    pub color: Option<String>,
    pub kind: WorkflowStateType,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum WorkflowStateType {
    Backlog,
    Unstarted,
    Started,
    Completed,
    Canceled,
    Unknown(String),
}

impl WorkflowStateType {
    pub fn from_api(value: Option<String>) -> Self {
        match value
            .unwrap_or_else(|| "unknown".to_owned())
            .to_ascii_lowercase()
            .as_str()
        {
            "backlog" => Self::Backlog,
            "unstarted" => Self::Unstarted,
            "started" => Self::Started,
            "completed" => Self::Completed,
            "canceled" | "cancelled" => Self::Canceled,
            other => Self::Unknown(other.to_owned()),
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self, Self::Completed | Self::Canceled)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum Priority {
    None,
    Urgent,
    High,
    Normal,
    Low,
    Unknown(u8),
}

impl Priority {
    pub fn from_api(value: Option<i64>) -> Self {
        match value.unwrap_or(0) {
            0 => Self::None,
            1 => Self::Urgent,
            2 => Self::High,
            3 => Self::Normal,
            4 => Self::Low,
            other if other < 0 => Self::Unknown(0),
            other => {
                let value = u8::try_from(other).unwrap_or(u8::MAX);
                Self::Unknown(value)
            }
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Urgent => 1,
            Self::High => 2,
            Self::Normal => 3,
            Self::Low => 4,
            Self::Unknown(value) => value,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Urgent => "Urgent",
            Self::High => "High",
            Self::Normal => "Normal",
            Self::Low => "Low",
            Self::Unknown(_) => "Unknown",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CreateIssueInput {
    pub team: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<u8>,
}

pub fn demo_snapshot(limit: IssueLimit) -> WorkspaceSnapshot {
    let count = limit.get().min(DEMO_ISSUES_DEFAULT);
    let now = Utc::now();
    let today = now.date_naive();
    let teams = demo_teams();
    let states = demo_states();
    let users = demo_users();
    let labels = demo_labels();
    let titles = demo_titles();
    let mut issues = Vec::with_capacity(count);

    for index in 0..count {
        let state = states[(index * 7 + index / 3) % states.len()].clone();
        let team = teams[index % teams.len()].clone();
        let completed_at = if state.kind == WorkflowStateType::Completed {
            Some(now - Duration::days(i64::try_from(index % 30).unwrap_or(0)))
        } else {
            None
        };
        let due_offset_days = i64::try_from((index * 5) % 42).unwrap_or(0) - 12;
        let due_date = if index % 5 == 0 {
            None
        } else {
            Some(today + Duration::days(due_offset_days))
        };
        let issue_labels = vec![labels[index % labels.len()].clone()];

        issues.push(Issue {
            id: format!("demo-{index:03}"),
            identifier: format!("LV-{}", 100 + index),
            title: titles[index % titles.len()].to_owned(),
            priority: Priority::from_api(Some(i64::try_from(index % 5).unwrap_or(0))),
            estimate: Some(f64::from((index % 8) as u32 + 1)),
            created_at: now - Duration::days(i64::try_from((index * 3) % 100).unwrap_or(0)),
            updated_at: now - Duration::days(i64::try_from((index * 2) % 60).unwrap_or(0)),
            completed_at,
            due_date,
            url: Some(format!("https://linear.app/demo/issue/LV-{}", 100 + index)),
            team,
            state,
            assignee: if index % 6 == 0 {
                None
            } else {
                Some(users[index % users.len()].clone())
            },
            labels: issue_labels,
        });
    }

    WorkspaceSnapshot {
        fetched_at: now,
        source: SnapshotSource::Demo,
        viewer: Some(Viewer {
            id: "demo-viewer".to_owned(),
            name: "Demo Operator".to_owned(),
            email: "demo@example.com".to_owned(),
        }),
        organization: Some(Organization {
            id: "demo-org".to_owned(),
            name: "Demo Workspace".to_owned(),
            url_key: "demo".to_owned(),
        }),
        issues,
    }
}

fn demo_teams() -> Vec<Team> {
    vec![
        Team {
            id: "team-platform".to_owned(),
            key: "PLT".to_owned(),
            name: "Platform".to_owned(),
            states: demo_states(),
        },
        Team {
            id: "team-product".to_owned(),
            key: "PRD".to_owned(),
            name: "Product".to_owned(),
            states: demo_states(),
        },
        Team {
            id: "team-design".to_owned(),
            key: "DSN".to_owned(),
            name: "Design Systems".to_owned(),
            states: demo_states(),
        },
    ]
}

fn demo_states() -> Vec<WorkflowState> {
    vec![
        WorkflowState {
            id: "state-backlog".to_owned(),
            name: "Backlog".to_owned(),
            color: Some("#6b7280".to_owned()),
            kind: WorkflowStateType::Backlog,
        },
        WorkflowState {
            id: "state-ready".to_owned(),
            name: "Ready".to_owned(),
            color: Some("#38bdf8".to_owned()),
            kind: WorkflowStateType::Unstarted,
        },
        WorkflowState {
            id: "state-building".to_owned(),
            name: "Building".to_owned(),
            color: Some("#f59e0b".to_owned()),
            kind: WorkflowStateType::Started,
        },
        WorkflowState {
            id: "state-review".to_owned(),
            name: "Review".to_owned(),
            color: Some("#a78bfa".to_owned()),
            kind: WorkflowStateType::Started,
        },
        WorkflowState {
            id: "state-done".to_owned(),
            name: "Done".to_owned(),
            color: Some("#22c55e".to_owned()),
            kind: WorkflowStateType::Completed,
        },
    ]
}

fn demo_users() -> Vec<User> {
    vec![
        User {
            id: "user-ada".to_owned(),
            name: "Ada".to_owned(),
            display_name: Some("Ada".to_owned()),
            email: "ada@example.com".to_owned(),
        },
        User {
            id: "user-grace".to_owned(),
            name: "Grace".to_owned(),
            display_name: Some("Grace".to_owned()),
            email: "grace@example.com".to_owned(),
        },
        User {
            id: "user-katherine".to_owned(),
            name: "Katherine".to_owned(),
            display_name: Some("Katherine".to_owned()),
            email: "katherine@example.com".to_owned(),
        },
        User {
            id: "user-margaret".to_owned(),
            name: "Margaret".to_owned(),
            display_name: Some("Margaret".to_owned()),
            email: "margaret@example.com".to_owned(),
        },
    ]
}

fn demo_labels() -> Vec<Label> {
    vec![
        Label {
            id: "label-bug".to_owned(),
            name: "Bug".to_owned(),
            color: Some("#ef4444".to_owned()),
        },
        Label {
            id: "label-ux".to_owned(),
            name: "UX".to_owned(),
            color: Some("#06b6d4".to_owned()),
        },
        Label {
            id: "label-perf".to_owned(),
            name: "Performance".to_owned(),
            color: Some("#f97316".to_owned()),
        },
        Label {
            id: "label-api".to_owned(),
            name: "API".to_owned(),
            color: Some("#8b5cf6".to_owned()),
        },
    ]
}

fn demo_titles() -> [&'static str; 12] {
    [
        "Add keyboard-first project triage lane",
        "Visualize blocked work in the cockpit",
        "Design issue aging radar and risk meter",
        "Batch GraphQL refreshes behind cache",
        "Tune terminal palette for dark themes",
        "Create guided issue creation flow",
        "Render cycle throughput as sparkline",
        "Expose high-priority work queue",
        "Polish empty and offline states",
        "Add assignee load distribution chart",
        "Investigate stale label sync edge case",
        "Ship animated launch sequence",
    ]
}

fn looks_like_uuid(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.len() == 36
        && trimmed
            .chars()
            .all(|character| character.is_ascii_hexdigit() || character == '-')
}

fn trim_body(body: &str) -> String {
    const BODY_PREVIEW_MAX: usize = 500;
    if body.len() <= BODY_PREVIEW_MAX {
        return body.to_owned();
    }

    let mut preview = body.chars().take(BODY_PREVIEW_MAX).collect::<String>();
    preview.push_str("...");
    preview
}

fn format_graphql_errors(errors: &[GraphQlError]) -> String {
    let mut messages = Vec::with_capacity(errors.len());
    for error in errors {
        messages.push(error.message.clone());
    }
    messages.join("; ")
}

#[derive(Debug, Serialize)]
struct GraphQlRequest<V> {
    query: &'static str,
    variables: V,
}

#[derive(Debug, Serialize)]
struct OAuthRefreshForm<'a> {
    refresh_token: &'a str,
    grant_type: &'static str,
    client_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct GraphQlError {
    pub message: String,
    #[serde(default)]
    pub path: Option<serde_json::Value>,
    #[serde(default)]
    pub extensions: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct EmptyVariables {}

#[derive(Debug, Serialize)]
struct IssuesVariables {
    first: i32,
    after: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssuesData {
    viewer: Option<ApiViewer>,
    organization: Option<ApiOrganization>,
    issues: ApiIssueConnection,
}

#[derive(Debug, Deserialize)]
struct ViewerData {
    viewer: ApiViewer,
}

#[derive(Debug, Deserialize)]
struct TeamsData {
    teams: ApiTeamConnection,
}

#[derive(Debug, Deserialize)]
struct IssueLookupData {
    issue: Option<ApiIssue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueCreateData {
    issue_create: MutationIssuePayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IssueUpdateData {
    issue_update: MutationIssuePayload,
}

#[derive(Debug, Deserialize)]
struct MutationIssuePayload {
    success: bool,
    issue: Option<ApiIssue>,
}

#[derive(Debug, Serialize)]
struct IssueLookupVariables {
    id: String,
}

#[derive(Debug, Serialize)]
struct IssueCreateVariables {
    input: IssueCreateInput,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueCreateInput {
    team_id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<u8>,
}

#[derive(Debug, Serialize)]
struct IssueUpdateVariables {
    id: String,
    input: IssueUpdateInput,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IssueUpdateInput {
    state_id: String,
}

#[derive(Debug, Deserialize)]
struct ApiIssueConnection {
    nodes: Vec<ApiIssue>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Debug, Deserialize)]
struct ApiTeamConnection {
    nodes: Vec<ApiTeam>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiViewer {
    id: String,
    name: String,
    email: String,
}

impl From<ApiViewer> for Viewer {
    fn from(value: ApiViewer) -> Self {
        Self {
            id: value.id,
            name: value.name,
            email: value.email,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiOrganization {
    id: String,
    name: String,
    url_key: String,
}

impl From<ApiOrganization> for Organization {
    fn from(value: ApiOrganization) -> Self {
        Self {
            id: value.id,
            name: value.name,
            url_key: value.url_key,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiIssue {
    id: String,
    identifier: String,
    title: String,
    priority: Option<i64>,
    estimate: Option<f64>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    due_date: Option<String>,
    url: Option<String>,
    team: ApiTeam,
    state: ApiWorkflowState,
    assignee: Option<ApiUser>,
    labels: ApiLabelConnection,
}

impl From<ApiIssue> for Issue {
    fn from(value: ApiIssue) -> Self {
        Self {
            id: value.id,
            identifier: value.identifier,
            title: value.title,
            priority: Priority::from_api(value.priority),
            estimate: value.estimate,
            created_at: value.created_at,
            updated_at: value.updated_at,
            completed_at: value.completed_at,
            due_date: parse_due_date(value.due_date),
            url: value.url,
            team: value.team.into(),
            state: value.state.into(),
            assignee: value.assignee.map(Into::into),
            labels: value.labels.nodes.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiTeam {
    id: String,
    key: String,
    name: String,
    #[serde(default)]
    states: Option<ApiWorkflowStateConnection>,
}

impl From<ApiTeam> for Team {
    fn from(value: ApiTeam) -> Self {
        let states = value
            .states
            .map(|connection| connection.nodes.into_iter().map(Into::into).collect())
            .unwrap_or_default();

        Self {
            id: value.id,
            key: value.key,
            name: value.name,
            states,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiWorkflowStateConnection {
    nodes: Vec<ApiWorkflowState>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiWorkflowState {
    id: String,
    name: String,
    color: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

impl From<ApiWorkflowState> for WorkflowState {
    fn from(value: ApiWorkflowState) -> Self {
        Self {
            id: value.id,
            name: value.name,
            color: value.color,
            kind: WorkflowStateType::from_api(value.kind),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiUser {
    id: String,
    name: String,
    display_name: Option<String>,
    email: String,
}

impl From<ApiUser> for User {
    fn from(value: ApiUser) -> Self {
        Self {
            id: value.id,
            name: value.name,
            display_name: value.display_name,
            email: value.email,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ApiLabelConnection {
    nodes: Vec<ApiLabel>,
}

#[derive(Debug, Deserialize)]
struct ApiLabel {
    id: String,
    name: String,
    color: Option<String>,
}

impl From<ApiLabel> for Label {
    fn from(value: ApiLabel) -> Self {
        Self {
            id: value.id,
            name: value.name,
            color: value.color,
        }
    }
}

fn parse_due_date(value: Option<String>) -> Option<NaiveDate> {
    value.and_then(|text| NaiveDate::parse_from_str(&text, "%Y-%m-%d").ok())
}

fn normalize_oauth_access_token(token: &str) -> String {
    let trimmed = token.trim();
    if trimmed.eq_ignore_ascii_case("bearer") {
        return String::new();
    }

    trimmed
        .strip_prefix("Bearer ")
        .or_else(|| trimmed.strip_prefix("bearer "))
        .unwrap_or(trimmed)
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::{
        IssueLimit, IssueLimitError, LinearAuth, LinearError, Priority,
        normalize_oauth_access_token,
    };

    #[test]
    fn issue_limit_rejects_zero_and_overflow() {
        assert_eq!(IssueLimit::new(0), Err(IssueLimitError::Zero));
        assert_eq!(
            IssueLimit::new(251),
            Err(IssueLimitError::TooLarge {
                value: 251,
                max: 250
            })
        );
    }

    #[test]
    fn priority_maps_linear_values() {
        assert_eq!(Priority::from_api(Some(1)), Priority::Urgent);
        assert_eq!(Priority::from_api(Some(4)), Priority::Low);
        assert_eq!(Priority::from_api(None), Priority::None);
    }

    #[test]
    fn oauth_tokens_are_normalized_for_headers() {
        assert_eq!(
            normalize_oauth_access_token("Bearer access-token"),
            "access-token"
        );
        assert_eq!(
            LinearAuth::OAuthAccessToken(" access-token ".to_owned())
                .authorization_header_value()
                .expect("valid oauth token"),
            "Bearer access-token"
        );
    }

    #[test]
    fn empty_auth_is_rejected() {
        assert!(matches!(
            LinearAuth::ApiKey(" ".to_owned()).authorization_header_value(),
            Err(LinearError::InvalidAuth { .. })
        ));
        assert!(matches!(
            LinearAuth::OAuthAccessToken("Bearer ".to_owned()).authorization_header_value(),
            Err(LinearError::InvalidAuth { .. })
        ));
    }
}
