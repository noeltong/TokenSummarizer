# TokenSummarizer — Token Usage Dashboard

> Language / 语言：**English** | [简体中文](README_CN.md)

A pure token-usage statistics tool that lives in the Windows system tray. It collects usage
**per platform**: SSH servers are always scanned with the Linux rules, the local machine uses the
Windows app rules (and additionally re-scans Linux-side agents inside WSL with the Linux rules).
Each CLI / app's local logs and SQLite databases are either read directly or mirrored over SFTP
incrementally, then aggregated by **model × Beijing-time calendar day** into a local history
database and charted in a bubble panel that sits next to the tray.

- Supported agents (13): Codex / Claude Code / OpenCode / Qwen Code / Hermes / Qoder / QoderCN /
  QoderCN CLI / ZCode / Pi / **WorkBuddy** / **Doubao Work (豆包工作)** / **Command Code**
- Platforms: Windows 10 1809+ / Windows 11, x64 (WSL distros supported on the local machine)
- Stack: Rust + Win32 + GDI+ self-drawn UI (logical pixels @96dpi, everything scaled via
  `S(v)=round(v*dpi/96)`)
- SSH: fully built in (russh + SFTP) — no system `ssh.exe`, no ssh-agent dependency, and no
  python3 required on servers
- Look: the bubble and dialogs use system DWM rounded corners on Windows 11; Windows 10 falls
  back to a rounded Region

## Main Panel

The panel always appears in the **bottom-right of the work area** (above the taskbar, right
aligned). Tabs: **All** · **Local** · one tab per server after that (status dot:
online/connecting/error), `+` to add a server.

- **All**: all servers + local machine merged
- **Local**: usage of the agents on this Windows machine (subtitle shows `COMPUTERNAME\USERNAME`).
  If WSL is installed, usage from agents inside WSL is also merged into this tab (data source is
  exactly the Linux layout)

Each tab contains two panels:

| Panel | Content | Range switch |
| --- | --- | --- |
| Token Usage Top 20 | Horizontal bars: one model per row (name on the left, bar length ∝ tokens, value at the end of the row); fewer than 20 rows are top-aligned with blank space below | All · 30d · 7d · Today |
| Daily Token Usage | Horizontal time-axis columns: x = date (old → new), y = relative token count, gridlines + value labels | 7d · 30d |

Qoder / QoderCN credits are converted into equivalent tokens with the "tokens per credit" setting
(default 20000) and merged into the statistics; the other sources (including QoderCN CLI /
WorkBuddy) count tokens only, with no credit conversion.

Drawing rules: the header (title + range switcher) is fixed, the content area below scrolls
independently, and the scrollbar overlays the right padding without taking extra width.

## Model Aliases

Settings → "Aliases…": map an original model name (exact match) to a friendlier display name
(e.g. `claude-sonnet-4-5-20250929` → `Sonnet 4.5`); usage of models that share an alias is merged
in the charts. Changes take effect immediately, no monitor restart needed.

## UI Language

- Settings → "Language" offers **简体中文 / English**; clicking "Save" switches the whole UI
  immediately (main panel, dialogs, tray menu and tooltip) — no restart needed
- In English mode token values use **K/M/B** units (e.g. 12.3K / 12.3M / 3.45B); Chinese uses
  万/亿. Display only — it never affects what is counted
- Stored as `"language": "zh" | "en"` in `config.json`; older configs without the field default
  to Chinese

## Platforms & Scan Rules

| Platform | Scan rules | Sources included |
| --- | --- | --- |
| Linux (SSH servers) | Linux home layout | Codex / Claude / OpenCode / Qwen / Hermes / Qoder / QoderCN / QoderCN CLI / ZCode / Pi / **WorkBuddy** / Command Code |
| Windows (local) | Windows app layout (`%USERPROFILE%\.xxx` and `%LOCALAPPDATA%\xxx`) | Codex / Claude / Qoder / QoderCN / QoderCN CLI / ZCode / **WorkBuddy** / **Doubao Work** / Command Code |
| WSL (local distros) | **Exactly** the Linux rules, scan root `\\wsl$\<distro>\home\<user>` | same as Linux |

- Platform differences stay in the background: the UI only shows "Totals / Local / per server",
  and WSL usage is merged into "Local".
- Only **Doubao Work** is Windows-only (not in the server scan plan). **WorkBuddy** is
  cross-platform: servers mirror `~/.workbuddy/projects` only, so the rest of the app directory
  never leaves the server.
- WSL distros are enumerated via the registry key
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss` — **`wsl.exe` is never called**
  (local security policy blocks it); the home directory is taken from `/etc/passwd` for the
  `DefaultUid`, falling back to `/etc/wsl.conf`, `home/*`, `/root`.

## Data Sources & Counting Rules

Mirrors `D:\Documents\token_summary\remote_collect.py` item by item:

| Agent | Location | Notes |
| --- | --- | --- |
| Codex CLI | `~/.codex/state_5.sqlite` | threads table model/tokens_used/created_at. **Cumulative per thread**, attributed to the creation day, so the whole source is recomputed every round |
| Claude Code | `~/.claude/projects/**/*.jsonl`, `~/.claude/sessions/**/*.jsonl` | assistant records: four usage token kinds from `message.usage`; skips `<synthetic>`; deduped by `message.id` etc., keeping the max |
| OpenCode | `~/.local/share/opencode/opencode.db` | new `session_message` (`type=assistant`, tokens have no total → sum the parts) / legacy `message` (`role=assistant`, has total). Upgraded databases keep the old table as a migration copy — **dedupe by id, new table wins**, otherwise usage is double-counted |
| Qwen Code | `~/.qwen/projects/**/chats/*.jsonl` | `total_token_count` of uiEvent `qwen-code.api_response` (falls back to the three-part sum) |
| Hermes | `~/.hermes/state.db` | sessions table dynamic columns, in+out+cache. **Cumulative per session**, recomputed every round |
| Qoder | `~/.qoder/projects/**` | assistant record `usage.credits` (international build); empty model falls back to runtime-config; credits converted per settings |
| QoderCN | `~/.qoder-cn/projects/**` | same (China build). Converted under the same rules as Qoder; the two never overwrite each other |
| QoderCN CLI | `~/.qoder-cli/ai-stats` | counts files only (token format not yet supported). The directory contains a full source-code snapshot — **counted remotely, never mirrored** |
| ZCode | `~/.zcode/cli/db/db.sqlite` (`model_usage`) primary; `~/.zcode/cli/rollout/**/*.jsonl` as a supplement only | usage is `computed_total_tokens` (= input+output+reasoning; `cache_read` is already inside `input_tokens`, **do not add it**); attributed to the Beijing day by `completed_at` (falls back to `started_at`). The rollout files only backfill records **after** the database watermark (`max(completed_at)` + 60s); the two sides are never summed |
| Pi | `~/.pi/agent/sessions` | session-file count only (token format not yet supported); when the directory is missing remotely the probe still counts it |
| WorkBuddy | `~/.workbuddy/projects/**/*.jsonl` (incl. `subagents/`) | `providerData.usage.totalTokens` (falls back to in+out). `cached_tokens` / `reasoning_tokens` are **subsets** of input/output — never added on top; dedupe key is `record.id`, **never `conversationRequestId`** (one request, many calls); `credit-usage` records are ignored. On servers only `~/.workbuddy/projects` is mirrored — the rest of `~/.workbuddy` is never downloaded |
| Doubao Work (豆包工作) | `%LOCALAPPDATA%\DoubaoWork` | **install detection + file count only**: in practice it is just a Chromium shell and installer logs, nothing parseable |
| Command Code | `~/.commandcode` (also accepts `~/.command-code`) | **install detection + file count only** (format unknown, no samples yet) |

### Probing for Count-Only Sources

`QoderCN CLI`, `Pi` and `Command Code` do not need file contents mirrored. Each sync round walks
the corresponding directory on the server with fixed depth/count limits and writes
"installed?, file count, dir count, truncated?" into `.probe.json` at the mirror root; the local
parser reads its counts from that file. Doubao Work is scanned locally only (no SFTP).
Directories missing remotely produce no mirror, and an old full-directory mirror of
`~/.qoder-cli` is cleaned up automatically on the next round.

All timestamps are normalized to **Beijing time (UTC+8) calendar days**; records without a
timestamp are filed by file mtime; deduplication only applies within a single round.

## Local History DB & Incremental Refresh

History DB: `%LOCALAPPDATA%\TokenSummarizer\history.sqlite3` (SQLite, next to `cache\`).

- Main table `day_usage(platform, target, agent, model, day, tokens, credits, messages, requests)` holds
  the primary-key aggregation (`requests` = the finest request-level record count each source
  offers; schema V2, V1 DBs are migrated automatically); two status tables record per-source
  detection state, coverage summary and the **incremental watermark**.
- Dates are read old → new: queries always `ORDER BY day ASC` using the
  `(target_id, day, source, model)` index.
- **First run**: no DB → a full scan of every platform/target builds it (WSL Claude can take a
  few minutes the first time).
- **Every later refresh**: only re-parses "each source's last successfully covered Beijing day →
  today", writes it back, then rebuilds the full snapshot from the DB for the UI — so the charts
  always show the full history while each round parses only about a day.
- Refresh interval: see next section.

Three write modes (decide whether per-day incremental windows are possible):

| Mode | Criterion | Sources |
| --- | --- | --- |
| Per-day window replace | record-level data, one record one timestamp | Claude / OpenCode / Qwen / Qoder / QoderCN / WorkBuddy |
| Full-source recompute | **cumulative values**: one row per thread/session that keeps growing | Codex / Hermes / ZCode |
| Count only | nothing parseable as usage | QoderCN CLI / Pi / Doubao Work / Command Code |

Cumulative sources must never be filtered by date, or the later growth of a cross-day long thread
would be lost permanently (this is the established rule, not something new).

### Failures Never Overwrite History

Sources that are `not detected` / returned only partial data this round / errored while reading /
failed to sync **only get their status fields updated; history rows are untouched** and the
watermark does not advance; the next round retries automatically. The mirror root carries a
`.mirror_epoch` marker — after "Clear cache" in settings the marker disappears → the next round
treats it as a new cache and forces a full re-scan, preventing a half-set mirror from overwriting
history against a stale watermark.

## Refresh

- Token usage: **every 1 hour** by default (adjustable 30–3600 s in settings; the tray's
  "Refresh now" triggers immediately)
- With the history DB in place each round only re-parses the last day until now; the first DB
  build needs one complete scan (all remote logs / all local app data)

## Configuration & Privacy

| Item | Location |
| --- | --- |
| Config | `%APPDATA%\TokenSummarizer\config.json` (atomic write; corrupt files are backed up automatically) |
| Logs | `%APPDATA%\TokenSummarizer\logs\app.log` (errors and key events only, never credentials) |
| History DB | `%LOCALAPPDATA%\TokenSummarizer\history.sqlite3` (usage by platform × agent × model × date; **clearing the cache does not delete it**) |
| Mirror cache | `%LOCALAPPDATA%\TokenSummarizer\cache\<server id>\` (copies of agent logs; clearable in settings, fully re-downloaded next time) |
| Temp copies | `%LOCALAPPDATA%\TokenSummarizer\tmp\` (scan copies of WSL SQLite files, deleted after use) |

- Passwords and key passphrases are encrypted with Windows DPAPI (CurrentUser); after switching
  users/machines they must be re-entered.
- Auth supports passwords (with keyboard-interactive fallback) and OpenSSH/PEM private keys
  (optional passphrase; `.ppk` not supported).
- Host-key TOFU: the SHA-256 fingerprint is recorded on first connection; a changed key blocks
  the connection and can be re-trusted in the server editor.
- Remote operations are only static commands and SFTP reads; configuration is never interpolated
  into a shell.

## Build

```bash
export PATH="/c/Users/24416/.cargo/bin:$PATH"   # cargo is not on the default PATH (Git Bash)
cargo test --offline                             # 113 unit tests
cargo build --release --offline                  # → target/release/token_summarizer.exe
powershell -File tools/make-icon.ps1             # regenerate assets/app.ico + preview
```

- Icon and app manifest are embedded into the exe at build time via the Windows SDK `rc.exe`
  (located automatically by build.rs; without it the tray falls back to runtime drawing).
  `assets/app.manifest` declares Common-Controls 6.0 (required for checkbox/radio visual styles)
  and PerMonitorV2 DPI awareness.
- russh must use the ring backend (`default-features = false`); when the network is unstable use
  `--offline` everywhere.

## Diagnostics: `--check`

Runs the full pipeline once without the GUI (config / per-server SSH + host key + SFTP / one sync
round / per-source summary) — handy for troubleshooting and CI:

```bash
./target/release/token_summarizer.exe --check
```

- Prints `[OK]` / `[FAIL]` per server and per step; any failure makes the exit code non-zero,
  everything passing exits 0
- The tail prints "probe hit N count-only sources" and each source's
  `N threads / N usage records / converted tokens`, plus the sources not detected
- The output **contains no credentials** (passwords, passphrases, private keys)

## Screenshot Walkthrough

`tools/capture-screens.ps1` is retained only as a disabled historical reference; screenshot capture is intentionally blocked because screenshots can contain hosts, usernames, and usage data
and dialog (used for regression walkthroughs):

```bash
pwsh -File tools/capture-screens.ps1 -OutDir shots -WarmupSeconds 40
```

- `-WarmupSeconds` waits for the first sync to finish; while warming up the script keeps the
  window visible (the bubble auto-hides when it loses focus)
- Captures use `PrintWindow(PW_RENDERFULLCONTENT)`; blank images are detected and treated as
  failures

## Usage

1. On first run the panel pops up automatically and opens "Add Server".
2. Fill in host/port/username and the auth method, click "Test" (verifies login, host key, SFTP).
3. After saving, scheduled syncing starts; use the switchers in the panel to change the chart
   time ranges.

> **Windows 11 puts new tray icons into the hidden-icons area**: click the `^` on the taskbar and
> drag "Token Usage Dashboard" onto the taskbar to pin it. The icon rebuilds itself automatically
> after an Explorer restart.

## Relation to the Reference Implementations

- Counting rules: `D:\Documents\token_summary` (Node.js + remote python3) → this project parses
  **natively in Rust** (SFTP incremental mirroring + local rusqlite/serde_json).
- Architecture & UI: `D:\Documents\gpu_observe_app` (C# WinForms) → this project is its Rust
  mirror (tray bubble, tabs, TOFU, DPAPI, self-drawn UI all from the same lineage). GPU
  monitoring and idle notifications were removed per requirements.

## Known Limitations

- Linux servers only; PuTTY `.ppk` not supported; no connection to the system ssh-agent.
- The mirror cache holds copies of logs (including token records) — clear it in settings if that
  is a concern (re-downloaded in full next time).
- No token-history export/archive; aggregation is based on the current log inventory only.
- **Deleting a ZCode session inside the app cascades away its usage records**
  (`model_usage.session_id` is `on delete cascade`); the corresponding history statistics drop
  accordingly — this is ZCode's own database behavior, not data discarded by this tool.
- ZCode usage comes from SQLite; read-only access leaves `-wal` (0 B) and `-shm` (32 KB) files in
  `~/.zcode/cli/db/` — a normal part of SQLite's WAL read path; do not delete them while ZCode is
  running.
- `QoderCN CLI` (`~/.qoder-cli`) and `Pi` (`~/.pi`) can only be counted as file numbers: their
  token formats are not supported yet, so these two always show 0 in the charts — but a notice is
  given when they are detected, so they are not mistaken for "not installed".

## License

MIT
