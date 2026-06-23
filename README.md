# l-vis

`l-vis` is a Rust terminal cockpit for Linear issue management. It uses
Ratatui for an animated TUI, talks to Linear through the GraphQL API with either
personal API keys or OAuth tokens, and falls back to deterministic demo data
when no credentials are available.

![l-vis terminal demo](assets/l-vis-demo.gif)

## Features

- Animated dashboard with workspace pulse, completion, overdue risk, load, and focus queue.
- My Issues cockpit with personal backlog, todo/active lanes, workflow chart, and activity timeline.
- Issue browser with keyboard filtering, selection, details, labels, due dates, and priorities.
- Update heatmap for recent activity.
- Status and priority charts plus throughput sparklines.
- Offline demo mode for trying the interface without a Linear account.
- Linear-backed commands for syncing, listing teams, creating issues, and completing issues.
- Global team filtering across the dashboard, issue list, heatmap, and charts.
- Light, dark, and auto terminal color themes.
- Local cache at the platform cache directory so the TUI can still open after a failed refresh.

## Run

```bash
cargo run -- --demo
```

With a Linear personal API key:

```bash
export LINEAR_API_KEY="lin_api_..."
cargo run --
```

With an OAuth access token:

```bash
export LINEAR_OAUTH_ACCESS_TOKEN="..."
cargo run --
```

With an OAuth refresh token:

```bash
export LINEAR_OAUTH_REFRESH_TOKEN="..."
export LINEAR_OAUTH_CLIENT_ID="..."
export LINEAR_OAUTH_CLIENT_SECRET="..." # optional for PKCE/public clients
cargo run --
```

Linear personal API keys are sent as `Authorization: <API_KEY>`. OAuth access
tokens are sent as `Authorization: Bearer <ACCESS_TOKEN>`.

Codex or MCP connector authentication is separate from this standalone binary;
`l-vis` cannot read those private connector credentials directly.

## CLI

```bash
cargo run -- doctor
cargo run -- sync
cargo run -- teams
cargo run -- --team-filter ENG --theme light
cargo run -- create --team ENG "Investigate terminal workflow"
cargo run -- complete ENG-123
```

Use `--demo` with `doctor`, `sync`, or the TUI to avoid network access:

```bash
cargo run -- --demo sync
```

## TUI Keys

- `1`: Dashboard.
- `2`: My Issues.
- `3`: Issue browser.
- `4`: Heatmap.
- `5`: Charts.
- `tab`: Next view.
- `j` / `k` or arrow keys: Move issue selection.
- `/`: Filter issues by identifier, title, state, team, assignee, or label.
- `x`: Clear filter.
- `t`: Cycle the global team filter.
- `T`: Clear the global team filter.
- `r`: Refresh Linear data or regenerate demo data.
- `m`: Move the selected issue to the team's completed workflow state.
- `d`: Toggle demo mode.
- `?`: Help.
- `q` or `esc`: Quit.
