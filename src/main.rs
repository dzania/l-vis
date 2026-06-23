mod analytics;
mod app;
mod cli;
mod linear;
mod storage;
mod ui;

use anyhow::{Context, Result, bail};
use clap::Parser;

use crate::app::{App, AppConfig};
use crate::cli::{Cli, Command};
use crate::linear::{
    CreateIssueInput, IssueLimit, LinearAuthConfig, LinearClient, OAuthRefreshConfig, demo_snapshot,
};
use crate::storage::IssueCache;

#[tokio::main]
async fn main() -> Result<()> {
    let mut cli = Cli::parse();
    let limit = IssueLimit::new(usize::from(cli.limit))?;
    let command = cli.command.take().unwrap_or(Command::Tui);

    match command {
        Command::Tui => run_tui(cli, limit).await,
        Command::Sync { json } => sync_issues(cli, limit, json).await,
        Command::Teams { json } => list_teams(cli, json).await,
        Command::Create {
            team,
            title,
            description,
            priority,
        } => create_issue(cli, team, title, description, priority).await,
        Command::Complete { issue } => complete_issue(cli, issue).await,
        Command::Doctor => doctor(cli, limit).await,
    }
}

async fn run_tui(cli: Cli, limit: IssueLimit) -> Result<()> {
    let config = AppConfig {
        auth: optional_auth_config_from_cli(&cli)?,
        demo: cli.demo,
        limit,
        theme: cli.theme,
        team_filter: cli.team_filter,
    };
    let app = App::bootstrap(config).await?;
    ui::run(app).await
}

async fn sync_issues(cli: Cli, limit: IssueLimit, json: bool) -> Result<()> {
    let cache = IssueCache::new()?;
    let snapshot = if cli.demo {
        demo_snapshot(limit)
    } else {
        let client = client_from_cli(&cli).await?;
        client.fetch_workspace(limit).await?
    };

    cache.save(&snapshot)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&snapshot)?);
    } else {
        println!(
            "Synced {} issues from {} into {}",
            snapshot.issues.len(),
            snapshot.source.as_str(),
            cache.path().display()
        );
    }

    Ok(())
}

async fn list_teams(cli: Cli, json: bool) -> Result<()> {
    let client = client_from_cli(&cli).await?;
    let teams = client.fetch_teams().await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&teams)?);
    } else {
        for team in teams {
            println!("{:<8} {:<36} {}", team.key, team.id, team.name);
        }
    }

    Ok(())
}

async fn create_issue(
    cli: Cli,
    team: String,
    title: String,
    description: Option<String>,
    priority: Option<u8>,
) -> Result<()> {
    let client = client_from_cli(&cli).await?;
    let issue = client
        .create_issue(CreateIssueInput {
            team,
            title,
            description,
            priority,
        })
        .await?;

    println!(
        "Created {}: {}",
        issue.identifier,
        issue.url.unwrap_or_else(|| issue.id.clone())
    );
    Ok(())
}

async fn complete_issue(cli: Cli, issue: String) -> Result<()> {
    let client = client_from_cli(&cli).await?;
    let issue = client.complete_issue(&issue).await?;
    println!("Moved {} to {}", issue.identifier, issue.state.name);
    Ok(())
}

async fn doctor(cli: Cli, limit: IssueLimit) -> Result<()> {
    let cache = IssueCache::new()?;
    println!("Cache: {}", cache.path().display());
    println!("Limit: {}", limit.get());
    println!("Theme: {}", cli.theme);
    println!(
        "Team filter: {}",
        non_empty(cli.team_filter.as_deref()).unwrap_or("all")
    );

    if cli.demo {
        let snapshot = demo_snapshot(limit);
        println!("Demo mode: {} generated issues", snapshot.issues.len());
        return Ok(());
    }

    if let Some(auth) = optional_auth_config_from_cli(&cli)? {
        let auth_label = auth.label();
        let client = LinearClient::from_auth_config(auth).await?;
        let viewer = client.fetch_viewer().await?;
        println!(
            "Linear: authenticated with {auth_label} as {} <{}>",
            viewer.name, viewer.email
        );
    } else if cache.load()?.is_some() {
        println!("Linear: no credentials, cached issues are available");
    } else {
        println!("Linear: no credentials and no cache; the TUI will start in demo mode");
    }

    Ok(())
}

async fn client_from_cli(cli: &Cli) -> Result<LinearClient> {
    if cli.demo {
        bail!("this command needs Linear credentials; omit --demo");
    }

    let auth = optional_auth_config_from_cli(cli)?.context(
        "set LINEAR_API_KEY, LINEAR_OAUTH_ACCESS_TOKEN, or LINEAR_OAUTH_REFRESH_TOKEN with LINEAR_OAUTH_CLIENT_ID",
    )?;
    Ok(LinearClient::from_auth_config(auth).await?)
}

fn optional_auth_config_from_cli(cli: &Cli) -> Result<Option<LinearAuthConfig>> {
    let api_key = non_empty(cli.api_key.as_deref());
    let oauth_access_token = non_empty(cli.oauth_access_token.as_deref());
    let oauth_refresh_token = non_empty(cli.oauth_refresh_token.as_deref());
    let oauth_client_id = non_empty(cli.oauth_client_id.as_deref());
    let oauth_client_secret = non_empty(cli.oauth_client_secret.as_deref());

    let auth_modes = usize::from(api_key.is_some())
        + usize::from(oauth_access_token.is_some())
        + usize::from(oauth_refresh_token.is_some());
    if auth_modes > 1 {
        bail!(
            "choose only one Linear auth mode: API key, OAuth access token, or OAuth refresh token"
        );
    }

    if let Some(api_key) = api_key {
        if oauth_client_id.is_some() || oauth_client_secret.is_some() {
            bail!("OAuth client fields cannot be combined with --api-key");
        }
        return Ok(Some(LinearAuthConfig::ApiKey(api_key.to_owned())));
    }

    if let Some(access_token) = oauth_access_token {
        if oauth_client_id.is_some() || oauth_client_secret.is_some() {
            bail!("OAuth client fields are only used with --oauth-refresh-token");
        }
        return Ok(Some(LinearAuthConfig::OAuthAccessToken(
            access_token.to_owned(),
        )));
    }

    if let Some(refresh_token) = oauth_refresh_token {
        let client_id = oauth_client_id.context(
            "--oauth-refresh-token requires --oauth-client-id or LINEAR_OAUTH_CLIENT_ID",
        )?;
        return Ok(Some(LinearAuthConfig::OAuthRefreshToken(
            OAuthRefreshConfig {
                refresh_token: refresh_token.to_owned(),
                client_id: client_id.to_owned(),
                client_secret: oauth_client_secret.map(str::to_owned),
            },
        )));
    }

    if oauth_client_id.is_some() || oauth_client_secret.is_some() {
        bail!("OAuth client fields require --oauth-refresh-token");
    }

    Ok(None)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}
