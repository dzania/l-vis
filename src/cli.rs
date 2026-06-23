use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "l-vis",
    version,
    about = "A Ratatui workspace cockpit for Linear issues",
    long_about = "l-vis turns Linear issues into a terminal dashboard with heatmaps, charts, filters, and a fast issue list. It can run against Linear with LINEAR_API_KEY, LINEAR_OAUTH_ACCESS_TOKEN, or an OAuth refresh token, and it also supports offline demo mode."
)]
pub struct Cli {
    #[arg(long, env = "LINEAR_API_KEY", global = true, hide_env_values = true)]
    pub api_key: Option<String>,

    #[arg(
        long,
        env = "LINEAR_OAUTH_ACCESS_TOKEN",
        global = true,
        hide_env_values = true
    )]
    pub oauth_access_token: Option<String>,

    #[arg(
        long,
        env = "LINEAR_OAUTH_REFRESH_TOKEN",
        global = true,
        hide_env_values = true
    )]
    pub oauth_refresh_token: Option<String>,

    #[arg(
        long,
        env = "LINEAR_OAUTH_CLIENT_ID",
        global = true,
        hide_env_values = true
    )]
    pub oauth_client_id: Option<String>,

    #[arg(
        long,
        env = "LINEAR_OAUTH_CLIENT_SECRET",
        global = true,
        hide_env_values = true
    )]
    pub oauth_client_secret: Option<String>,

    #[arg(long, global = true, default_value_t = 120, value_parser = clap::value_parser!(u16).range(1..=250))]
    pub limit: u16,

    #[arg(long, global = true, default_value_t = ThemeMode::Auto)]
    pub theme: ThemeMode,

    #[arg(
        long = "team-filter",
        alias = "filter-team",
        global = true,
        help = "Initial team filter by key, name, or UUID"
    )]
    pub team_filter: Option<String>,

    #[arg(long, global = true)]
    pub demo: bool,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    #[command(about = "Launch the animated terminal UI")]
    Tui,

    #[command(about = "Fetch issues into the local cache")]
    Sync {
        #[arg(long)]
        json: bool,
    },

    #[command(about = "List Linear teams available to the token")]
    Teams {
        #[arg(long)]
        json: bool,
    },

    #[command(about = "Create a Linear issue")]
    Create {
        #[arg(short, long, help = "Team key or UUID")]
        team: String,

        #[arg(help = "Issue title")]
        title: String,

        #[arg(short, long)]
        description: Option<String>,

        #[arg(short, long, value_parser = clap::value_parser!(u8).range(0..=4))]
        priority: Option<u8>,
    },

    #[command(about = "Move an issue to the first completed workflow state")]
    Complete {
        #[arg(help = "Issue identifier such as ENG-123 or a Linear UUID")]
        issue: String,
    },

    #[command(about = "Check credentials, cache, and demo availability")]
    Doctor,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ThemeMode {
    #[default]
    Auto,
    Dark,
    Light,
}

impl std::fmt::Display for ThemeMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(formatter, "auto"),
            Self::Dark => write!(formatter, "dark"),
            Self::Light => write!(formatter, "light"),
        }
    }
}
