# TokenSummarizer — Token 用量观察台

> 语言 / Language：[English](README.md) | **简体中文**

常驻 Windows 系统托盘的纯 Token 统计工具。按**平台**分别采集：SSH 服务器一律按 Linux 规则，
本机按 Windows app 规则（并额外把 WSL 里的 Linux 侧 agent 按 Linux 规则再扫一遍）。
各 CLI / app 的本地日志与 SQLite 经 SFTP 增量镜像或直接读取后，按**模型 × 北京时间自然日**聚合，
写入本地历史库，在贴近托盘的气泡面板中以图表展示。

- 支持的 agent（13 个）：Codex / Claude Code / OpenCode / Qwen Code / Hermes / Qoder / QoderCN /
  QoderCN CLI / ZCode / Pi / **WorkBuddy** / **豆包工作** / **Command Code**
- 平台：Windows 10 1809+ / Windows 11，x64（本机额外支持 WSL 发行版）
- 技术栈：Rust + Win32 + GDI+ 自绘 UI（逻辑像素 @96dpi，全部经 `S(v)=round(v*dpi/96)` 缩放）
- SSH：完全内置（russh + SFTP），不调用系统 `ssh.exe`，不依赖 ssh-agent；不要求服务器安装 python3
- 外观：Windows 11 下气泡与对话框使用系统 DWM 圆角；Win10 回退圆角 Region

## 主界面

面板固定出现在**工作区右下角**（任务栏上方靠右）。选项卡：**合计** · **本机** · 其后每台服务器一个标签
（状态圆点：在线/连接中/错误），`+` 添加服务器。

- **合计**：全部服务器 + 本机合并统计
- **本机**：当前这台 Windows 机器上各 agent 的用量（副标题显示 `计算机名\用户名`）。
  若装了 WSL，WSL 里 agent 的用量也并入本选项卡（数据来源与 Linux 完全同构）

每个选项卡内两个 Panel：

| Panel | 内容 | 切换 |
| --- | --- | --- |
| Token 用量 Top 20 | 横向柱状图：每行一个模型（左名称、条长 ∝ tokens、行尾数值）；不足 20 个顶端对齐下方留白 | 合计 · 30天 · 7天 · 今天 |
| 每日 Token 用量 | 横向时间轴柱状图：x=日期（旧→新）、y=相对 token 数、网格线 + 数值标注 | 7天 · 30天 |

Qoder / QoderCN 的积分按设置中的"每积分折算 Token 数"（默认 20000）折算为等效 token 并入统计；
其余源（含 QoderCN CLI / WorkBuddy）只计 token，不做积分折算。

面板绘制规则：头部（标题 + 时间范围切换器）固定不动，下方内容区独立滚动，
滚动条叠在右侧白内，不额外占用宽度。

## 模型别名

设置 → 「模型别名…」：把原始模型名（精确匹配）映射为更友好的名称
（例如 `claude-sonnet-4-5-20250929` → `Sonnet 4.5`）；相同别名的用量在图表中合并计算。
修改即时生效，无需重启监控。

## 平台与扫描规则

| 平台 | 扫描规则 | 包含的源 |
| --- | --- | --- |
| Linux（SSH 服务器） | 按 Linux home 结构 | Codex / Claude / OpenCode / Qwen / Hermes / Qoder / QoderCN / QoderCN CLI / ZCode / Pi / **WorkBuddy** / Command Code |
| Windows（本机） | 按 Windows app 结构（`%USERPROFILE%\.xxx` 与 `%LOCALAPPDATA%\xxx`） | Codex / Claude / Qoder / QoderCN / QoderCN CLI / ZCode / **WorkBuddy** / **豆包工作** / Command Code |
| WSL（本机发行版） | 与 Linux **完全相同**的规则，扫描根是 `\\wsl$\<distro>\home\<user>` | 同 Linux |

- 平台差异只在后台：UI 仍然是「合计 / 本机 / 每台服务器」，WSL 的用量并入「本机」。
- 只有**豆包工作**是 Windows-only（不在服务器扫描计划内）。**WorkBuddy** 跨平台：远端只镜像
  `~/.workbuddy/projects`，`.workbuddy` 其它目录不会离开服务器。
- WSL 通过注册表 `HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss` 枚举，**不调用 `wsl.exe`**
  （本机安全策略会拦），home 优先取 `/etc/passwd` 里 `DefaultUid` 对应的目录。

## 数据源与统计口径

与 `D:\Documents\token_summary\remote_collect.py` 逐条对应：

| Agent | 位置 | 说明 |
| --- | --- | --- |
| Codex CLI | `~/.codex/state_5.sqlite` | threads 表 model/tokens_used/created_at。**线程累计值**，归创建日，因此整源每次重算 |
| Claude Code | `~/.claude/projects/**/*.jsonl`、`~/.claude/sessions/**/*.jsonl` | assistant 记录 message.usage 四类 token；跳过 `<synthetic>`；按 message.id 等去重取最大 |
| OpenCode | `~/.local/share/opencode/opencode.db` | 新版 `session_message`（`type=assistant`，tokens 无 total → 分项求和）/ 旧版 `message`（`role=assistant`，有 total）。升级后的库会把旧表留成迁移副本，**按 id 去重、新表优先**，否则双计 |
| Qwen Code | `~/.qwen/projects/**/chats/*.jsonl` | uiEvent `qwen-code.api_response` 的 total_token_count（缺则三和） |
| Hermes | `~/.hermes/state.db` | sessions 表动态列，in+out+cache。**会话累计值**，整源每次重算 |
| Qoder | `~/.qoder/projects/**` | assistant 记录 usage.credits（国际版）；模型空则回退 runtime-config；积分按设置折算 |
| QoderCN | `~/.qoder-cn/projects/**` | 同上（国内版）。与 Qoder 同口径折算积分，二者互不覆盖 |
| QoderCN CLI | `~/.qoder-cli/ai-stats` | 仅统计文件数（token 格式暂未支持）。该目录含完整源码快照，**只在远端计数、不镜像内容** |
| ZCode | `~/.zcode/cli/db/db.sqlite`（`model_usage` 表）为主；`~/.zcode/cli/rollout/**/*.jsonl` 仅作补充 | 用量取 `computed_total_tokens`（= input+output+reasoning；`cache_read` 已含在 `input_tokens` 内，**不叠加**）；按 `completed_at` 归北京日（缺则 `started_at`）。rollout 只补「数据库水位线（`max(completed_at)` + 60s）之后」的记录，两者不相加 |
| Pi | `~/.pi/agent/sessions` | 仅会话文件计数（token 格式暂未支持）；远端无该目录时同样经探测计数 |
| WorkBuddy | `~/.workbuddy/projects/**/*.jsonl`（含 `subagents/`） | `providerData.usage.totalTokens`（缺则 in+out）。`cached_tokens` / `reasoning_tokens` 分别是 input/output 的**子集，不叠加**；去重键是 `record.id`，**不能用 `conversationRequestId`**（一次请求多次调用）；忽略 `credit-usage` 记录。远端只镜像 `~/.workbuddy/projects`，`.workbuddy` 其它目录不下载 |
| 豆包工作 | `%LOCALAPPDATA%\DoubaoWork` | **仅检测安装 + 文件计数**：实测只有 Chromium 壳与安装器日志，无可解析用量 |
| Command Code | `~/.commandcode`（兼容 `~/.command-code`） | **仅检测安装 + 文件计数**（格式未知，暂无样本） |

### 仅计数源的探测

`QoderCN CLI`、`Pi`、`Command Code` 不需要镜像文件内容。每轮同步会在远端按固定深度/条数上限
遍历对应目录，把「是否安装、文件数、目录数、是否被截断」写入镜像根目录的 `.probe.json`，
本地解析器从该文件读取计数。豆包工作只在本机扫描（不走 SFTP）。
远端不存在的目录不产生镜像，`~/.qoder-cli` 的旧整目录镜像会在下一轮自动清理。

时间统一折算到**北京时间（UTC+8）自然日**；缺时间戳的记录按文件 mtime 归档；
去重仅单轮内生效。

## 本地历史库与增量刷新

历史库：`%LOCALAPPDATA%\TokenSummarizer\history.sqlite3`（SQLite，与 `cache\` 同级）。

- 表结构：`day_usage(platform, target, agent, model, day, tokens, credits, messages)` 为主键聚合，
  另有两张状态表记录每个源的检出状态、覆盖摘要与**增量水位线**。
- 日期由远及近：读取一律 `ORDER BY day ASC`，走 `(target_id, day, source, model)` 索引。
- **首次运行**：库不存在 → 各平台各目标全量扫描一遍建库（WSL 里 Claude 体积大时首次可能几分钟）。
- **之后每次刷新**：只重算「每个源最后一次成功覆盖到的北京日 → 今天」这一段，写回后
  再从库里重建完整快照交给界面 —— 所以图表始终是全历史，而每轮解析量只有一天左右。
- 刷新间隔见下节。

三种写入模式（决定能否按天窗口增量）：

| 模式 | 判定依据 | 源 |
| --- | --- | --- |
| 按天窗口替换 | 记录级数据，一条记录一个时间戳 | Claude / OpenCode / Qwen / Qoder / QoderCN / WorkBuddy |
| 整源重算 | **累积值**：一行一个线程/会话且数值持续增长 | Codex / Hermes / ZCode |
| 只计数 | 没有可解析的用量 | QoderCN CLI / Pi / 豆包工作 / Command Code |

累积值源不能按日期过滤，否则跨天长线程的后续增长会永久丢失（这是既有口径，非本次引入）。

### 失败不覆盖历史

`未检测到` / 本轮只拿到部分数据 / 读取报错 / 同步失败的源，**只更新状态字段，不动历史行**，
水位线也不推进；下一轮自动重试。镜像根有 `.mirror_epoch` 标记，设置里「清空缓存」后标记消失
→ 下一轮识别为新缓存并强制全量重扫，避免用半套镜像按旧水位线覆盖历史。

## 刷新

- Token 用量：默认**每 1 小时**刷新一次（设置中可调 30–3600 秒；托盘右键"立即刷新"即时触发）
- 有历史库后，每轮只解析最后一天到现在的数据；首次建库需完整扫一遍（远端全部日志 / 本机全部 app 数据）

## 界面语言

- 设置 → 「语言」可选 **简体中文 / English**，点「保存」后全界面立即切换（主面板、对话框、托盘菜单与 tooltip），无需重启
- 英文模式下 token 数值单位为 **K/M/B**（如 12.3K / 12.3M / 3.45B），中文为 万/亿；仅影响显示，不影响统计口径
- 配置存于 `config.json` 的 `"language": "zh" | "en"`，旧配置缺该字段时默认中文

## 配置与隐私

| 内容 | 位置 |
| --- | --- |
| 配置 | `%APPDATA%\TokenSummarizer\config.json`（原子写；损坏自动备份） |
| 日志 | `%APPDATA%\TokenSummarizer\logs\app.log`（只记错误与关键事件，不记凭据） |
| 历史库 | `%LOCALAPPDATA%\TokenSummarizer\history.sqlite3`（平台 × agent × 模型 × 日期的用量；**清空缓存不会删它**） |
| 镜像缓存 | `%LOCALAPPDATA%\TokenSummarizer\cache\<服务器id>\`（各 Agent 日志副本；设置中可清空，下次全量重下） |
| 临时副本 | `%LOCALAPPDATA%\TokenSummarizer\tmp\`（WSL 上 SQLite 的扫描副本，用完即删） |

- 密码与私钥口令使用 Windows DPAPI（CurrentUser）加密保存；换用户/换机器后需重新输入。
- 认证支持密码（含键盘交互回退）与 OpenSSH/PEM 私钥（可选口令，不支持 .ppk）。
- 主机密钥 TOFU：首次连接记录 SHA-256 指纹，变更阻断，可在编辑服务器中重新信任。
- 远端操作只有静态命令与 SFTP 读取，配置不拼接进 shell。

## 构建

```bash
export PATH="/c/Users/24416/.cargo/bin:$PATH"   # cargo 不在默认 PATH（Git Bash）
cargo test --offline                             # 113 个单测
cargo build --release --offline                  # → target/release/token_summarizer.exe
powershell -File tools/make-icon.ps1             # 重新生成 assets/app.ico + 预览图
```

- 图标与 app 清单在构建期经 Windows SDK `rc.exe` 嵌入 exe（build.rs 自动查找；缺失时托盘回退运行时绘制）。
  `assets/app.manifest` 声明 Common-Controls 6.0（复选框/单选框视觉样式必需）与 PerMonitorV2 DPI 感知。
- russh 必须用 ring 后端（`default-features = false`）；网络不稳时全部 `--offline`。

## 诊断：`--check`

不带 GUI 跑一轮完整链路（配置 / 每台服务器 SSH + 主机密钥 + SFTP / 一轮同步 / 各源汇总），
适合排障与 CI：

```bash
./target/release/token_summarizer.exe --check
```

- 每台服务器逐项打印 `[OK]` / `[FAIL]`；任一失败则退出码非 0，全部通过退出码 0
- 末尾打印「探测命中 N 个仅计数源」与每个源的 `N 线程 / N 条用量 / 折算 token`，以及未检测到的源
- 输出**不含任何凭据**（密码、口令、私钥内容）

## 界面截图走查

`tools/capture-screens.ps1` 仅作为已禁用的历史参考保留；由于截图可能包含主机、用户名和用量数据，截图采集已永久阻止：

```bash
pwsh -File tools/capture-screens.ps1 -OutDir shots -WarmupSeconds 40
```

- `-WarmupSeconds` 等待首次同步完成；预热期间脚本会持续保持窗口可见（气泡失焦会自动隐藏）
- 截图用 `PrintWindow(PW_RENDERFULLCONTENT)`；空白图会被检测并判为失败

## 使用

1. 首次运行自动弹出面板并打开"添加服务器"。
2. 填写主机/端口/用户名与认证方式，点「测试连接」（验证登录、主机密钥、SFTP）。
3. 保存后开始定时同步；点击面板中的切换器查看不同时间范围的图表。

> **Windows 11 会把新托盘图标收进隐藏图标区**：点击任务栏 `^`，把"Token 用量观察台"拖到任务栏固定显示。Explorer 重启后图标会自动重建。

## 与参考实现的关系

- 统计口径：`D:\Documents\token_summary`（Node.js + 远端 python3）→ 本项目 **Rust 原生解析**（SFTP 增量镜像 + 本地 rusqlite/serde_json）。
- 架构与 UI：`D:\Documents\gpu_observe_app`（C# WinForms）→ 本项目为其 Rust 镜像（托盘气泡、选项卡、TOFU、DPAPI、自绘 UI 同源）。GPU 监控与空闲通知已按需求移除。

## 已知限制

- 仅支持 Linux 服务器；不支持 PuTTY `.ppk`；不连接系统 ssh-agent。
- 镜像缓存保存日志副本（含 token 记录），介意可在设置中清空（下次全量重下）。
- 不支持 token 历史导出/归档；聚合只基于当前日志存量。
- **ZCode 在应用内删除会话会级联删除其用量记录**（`model_usage.session_id` 是 `on delete cascade`），
  对应历史统计会随之下降；这是 ZCode 库自身的行为，非本工具丢弃。
- ZCode 用量来自 SQLite，只读打开会在 `~/.zcode/cli/db/` 留下 `-wal`(0B) 与 `-shm`(32KB)，
  属 SQLite 的 WAL 读路径正常产物；ZCode 运行期间请勿删除。
- `QoderCN CLI`（`~/.qoder-cli`）与 `Pi`（`~/.pi`）只能统计文件数：其 token 格式尚未支持，
  图表中这两项恒为 0，但会在检测到时给出提示，避免被误认为「没装」。

## 许可

MIT
