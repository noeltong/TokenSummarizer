# AGENTS.md — token_summarizer_rust

Windows 系统托盘应用（Rust + Win32 + GDI+ 自绘 UI）：按**平台**采集 coding agent 的模型 token 用量，
按「模型 × 北京时间自然日」写入**本地历史库**，气泡面板以双 Panel 图表展示
（Top20 模型横向柱状图 + 每日柱状图），支持模型别名合并。**纯 token 工具——无 GPU/通知代码。**

平台 = Linux（SSH 服务器，按 Linux home 规则）/ Windows（本机，按 app 规则 + WSL）/
WSL（本机发行版，按 Linux 规则）。13 个源：Codex/Claude/OpenCode/Qwen/Hermes/Qoder/QoderCN/
QoderCN CLI/ZCode/Pi/WorkBuddy/豆包工作/Command Code。

## 参考实现（改口径前必读）

- **`<reference-workspace>/gpu_observe_app`（C# WinForms）**：架构镜像来源——托盘气泡、选项卡、TOFU、DPAPI、自绘 UI 同源。GPU/通知部分已删除，勿参照其硬件代码回填。
- **`<reference-workspace>/token_summary/remote_collect.py`**：token 统计口径来源（各源路径/SQL/字段/去重键/北京日规则）。改 `src/parsing/token/*.rs` 前先读对应函数。
- `<reference-workspace>/gpu_observer_rust` 是**空目录**（不是参考实现）。

## 构建与测试

```bash
export PATH="/c/Users/24416/.cargo/bin:$PATH"   # cargo 不在默认 PATH（Git Bash）
cargo check/test/build --offline                 # 网络不稳；依赖已全部缓存
cargo test --offline                             # 113 个单测
cargo build --release --offline
./target/release/token_summarizer.exe --check    # 无 GUI 跑一遍 本机/WSL/SSH/SFTP/同步/建库，失败退出码非 0
powershell -File tools/make-icon.ps1             # 重新生成 assets/app.ico
# 截图采集已禁用：截图可能包含主机、用户名和用量数据，不得生成或保存
```

- 本机到 crates.io 极不稳定；缺 crate 时把 `~/.cargo/registry` 下 `mirrors.ustc.edu.cn-*` 的缓存合并进 `index.crates.io-*` 再 `--offline`（历史做法）。
- russh **必须** `default-features = false, features = ["flate2","ring","rsa"]`——默认 aws-lc-rs 后端需要 NASM。
- 构建期 `build.rs` 直接调用 Windows SDK `rc.exe`（查找 `C:\Program Files (x86)\Windows Kits\10\bin\<最新>\x64\rc.exe`）把 `assets/app.ico` 与 `assets/app.manifest`（`1 24` 资源）编成 .res 并经 `cargo:rustc-link-arg-bins` 链接；失败仅 warning，托盘回退运行时绘制。

## 架构分层（lib + 薄 bin）

- `src/main.rs`：入口（DPI PerMonitorV2 → COM STA → GDI+ → 单实例 → tokio 线程 → 窗口/托盘 → 消息循环）；`#![windows_subsystem = "windows"]`（删掉会弹控制台）。
- `src/app/`：托盘（资源图标优先/运行时绘制回退）、单实例、HKCU 自启、`WM_APP*` 常量。
- `src/config/`：AppConfig（原子写）+ DPAPI（`secret.rs`）+ `model_aliases`（AliasEntry{from,to}）。
- `src/ssh/`：russh 会话（密码+键盘交互回退/OpenSSH·PEM 私钥/TOFU/退避重连）、SFTP、tester（只验证登录/主机密钥/SFTP）。
- `src/monitoring/`：supervisor + `local_loop`（Windows home + 各 WSL 发行版，各自一个 target）+ 每服务器 `token_loop`（SFTP 增量镜像 → 窗口解析 → 写库 → 从库重建快照）。**phase/error 由 token_loop 独占维护**。`wsl.rs` 负责注册表枚举发行版与 home 定位。
- `src/store/mod.rs`：**本地历史库**（`%LOCALAPPDATA%\TokenSummarizer\history.sqlite3`）。`ScanTarget`（local / wsl:distro:user / server-uuid）+ `ScanPlan`（每源窗口、是否全量）+ `prepare`/`apply`/`load`。见下方「历史库与增量」。
- `src/model.rs`：`Platform`/`StorageMode`/`Source`（13 变体）+ ServerData + TokenSnapshot 聚合 API：`merge_all`（合计选项卡）、`model_totals`（别名归并 + 积分折算）、`daily_totals`（补零）、`source_totals`（每源合计，供 `--check`）。**唯一折算入口 `Source::effective_tokens`**——改口径只改这里。
- `src/parsing/token/`：每源一文件；输入是本地镜像目录 / 本机 home / WSL home；`probe.rs` 读写镜像根 `.probe.json`（远端目录探测结果）+ 本地有界计数；`sqlite.rs` 统一 SQLite 打开策略（WSL 走复制副本）。
- `src/ui/`：`gdip.rs`（GDI+ RAII，含 `set_clip_rect`/`reset_clip`）、`theme.rs`（色板+DPI 字体缓存）、`chart.rs`（柱状图+分段切换器）、`content.rs`（双 Panel 仪表盘）、`main_window.rs`（气泡主窗口：选项卡条 + 滚动内容区 + 圆角）、`settings.rs`/`server_edit.rs`/`alias_edit.rs`（无边框对话框）、`button.rs`（自绘按钮 Primary/Secondary/Danger）。

## 关键规则

- **UI 布局全部逻辑像素 @96dpi**，经 `theme.scale(v) = round(v*dpi/96)` 换算；DPI 变化重建 Theme。
- **新增 agent 数据源**（顺序固定，漏一步就会静默漏统计）：
  1. `model.rs`：`Source` 变体 + `ALL`（长度守卫 `assert!(len == N)` 会报错）+ `label` + `key`/`from_key` + `storage_mode` + `record_unit` + `index_in_all`（全部无通配 match）。
  2. `parsing/token/<源>.rs`：`scan_with(root, ctx)` + `scan(root)` 全量包装；窗口内过滤走 `ctx.in_window`，文件级跳过走 `ctx.should_read_jsonl`；读失败置 `partial = true`。
  3. `parsing/token/mod.rs`：加进 `WINDOWS_SCAN_PLAN` / `LINUX_SCAN_PLAN`（会有单测检查「每个源至少在一个计划里」）+ `scan_one` 分支。
  4. `token_sync.rs`：只有**需要镜像的远端源**才加 `LINUX_JSONL_BASES` / `LINUX_SQLITE_DBS` / `LINUX_PROBE_SPECS`；计划与规格的双向一致性由 `token_sync.rs::linux_plan_and_sync_specs_cover_each_other` 守卫（漏加会静默漏采）。
  5. README 数据源表。
- **平台规则不可混用**：远端服务器**永远**按 Linux 规则扫（`ServerConfig` 不设平台字段）；Windows 规则只用于本机；WSL 复用 `LINUX_SCAN_PLAN`。UI 只有「合计/本机/每台服务器」，平台概念不得泄漏到 `src/ui/**`。WorkBuddy 跨平台（Linux 服务器同样写 `~/.workbuddy/projects`，远端只镜像该子目录），因此同时出现在两个计划；只有豆包工作是 Windows-only。
- **WSL 只走注册表 + `\\wsl$\<distro>`**：`HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss` 枚举发行版，home 取 `/etc/passwd`（`DefaultUid`）→ `/etc/wsl.conf` → `home/*` → `/root`。**禁止调用 `wsl.exe`**（本机安全策略直接拦）与 `reg.exe`。`/mnt/*` 必须拒绝（那是 Windows 盘，会与 Windows 扫描重复计）。
- **WSL 的 SQLite 必须复制副本再读**（`parsing/token/sqlite.rs`）：`\\wsl$` 走 9P，WAL/文件锁不可靠。复制 `db`+`-wal`+`-shm` 到 `%LOCALAPPDATA%\TokenSummarizer\tmp\`，**读写**方式打开让 SQLite 做 WAL 恢复，`Drop` 时先关连接再删目录（顺序反了 Windows 删不掉）。
- **只计数、不解析用量的源走探测**：不要塞进 `JSONL_BASES`。在 `LINUX_PROBE_SPECS` 加一条（`source` + `detect_rel` + `count_rel`），远端只 `read_dir` 计数，结果写镜像根 `.probe.json`，解析器在该目录未镜像时回退读 probe（`probe::scan_counts` / `scan_from_probe`）。`~/.qoder-cli` 含完整源码快照，**禁止整目录镜像**（下轮 `prune_legacy_mirrors` 会清理旧镜像）。豆包工作在 `%LOCALAPPDATA%`，只在本机计数。
- **积分折算只对 `counts_credits()` 为真的源生效**（Qoder / QoderCN）；`QoderCN CLI` / `Pi` / `WorkBuddy` / 豆包 / Command Code 只计数或只算 token，不折算。两处聚合（`model_totals`/`daily_totals`）必须都走 `effective_tokens`，否则 UI 与 `--check` 口径会分叉。
- **WorkBuddy 口径**：usage 只在 `type=function_call` 与 `type=message&&role=assistant` 的 `providerData.usage` 上，取 `totalTokens`（缺则 in+out）；`cached_tokens`/`reasoning_tokens` 是 input/output 的**子集，不得叠加**；去重键是 `record.id`，**`conversationRequestId` 是一对多**，用它去重会把多次模型调用压成一次；`credit-usage` 记录无 model，不参与统计。
- **OpenCode 有两代库结构，必须都认**：新版是 `session_message` 表（`type='assistant'`，tokens **没有 total**，模型在 `model.id`，`time_created` 列是**毫秒**）；
  旧版是 `message` 表（`role='assistant'`，有 `tokens.total`，模型在顶层 `modelID`）。升级后的库会把旧表留成**迁移副本**（实测 id 与新表 100% 重叠），
  所以 `opencode.rs` 按 **id 去重、新表优先**：只读新表会让未升级的机器读挂，两表相加会双计。
  新版 total 用 `input+output+reasoning+cache.read+cache.write`（旧库 total 实测恒等于该分项和）。**改动读取逻辑后必须把 `store::parser_revision` 里该源 +1**，否则已入库的历史不会被补算（水位线只表达"覆盖到哪天"，表达不了"表换了"）。
- **同一 agent 同时有 SQLite 与 JSONL 时：SQLite 优先，JSONL 只做「水位线增量」，禁止整份相加**（ZCode 范式，`zcode.rs`）。
  ZCode 的权威源是 `~/.zcode/cli/db/db.sqlite::model_usage`（请求级、全量）；`rollout` 是滚动窗口（仅最近约 2 小时、3 个会话），
  只补 `ts > max(completed_at) + 60s` 的记录。两侧 ID 空间不相交（rollout 裸 UUID vs DB `msg_*`，实测交集 0），
  **不要试图按 id 关联**；已入库请求的 rollout 时间戳恒早于水位线（实测早 7~98ms），所以该判据不会双计。
- **ZCode 用量列只能用 `computed_total_tokens`**（= input + output + reasoning，实测 1442/1442 成立）；
  `cache_read_input_tokens` **已包含在** `input_tokens` 内，再叠加会让绝大多数行虚高（SQL 里干脆不 SELECT cache 列）。
  `provider_total_tokens` 有 NULL，不可直接 SUM。归日 `completed_at`（缺则 `started_at`），ms 时间戳由 `parse_timestamp` 自动识别。
- **ZCode DB 的读取与同步成本**：整库约 30MB（`part` 表占 19MB，真正要用的 `model_usage` 不到 1MB），
  WAL checkpoint 会改主库 mtime → 活跃期最坏每轮重下约 34MB/台（`SQLITE_DBS` 里有注释）。
  对实机库用 `SQLITE_OPEN_READ_ONLY` + `busy_timeout`，会留下 `-wal`(0B)/`-shm`(32KB)（WAL 读路径的正常产物）；
  只读连接无法做 WAL 恢复，失败时复制三件套到临时目录再以读写方式打开重试一次。
- **聚合图表的时间范围**：超大天数（≥365_000）视为全部历史（min_day 用空串）——`chrono::Duration::days(巨大值)` 会内部溢出 panic 且发生在窗口过程里直接 abort 进程（曾因此"token 一到就崩溃"）。
- **气泡内容区分层**：头部（标题 + 副标题 + 新鲜度）固定不滚，其下才是滚动区（两个 Panel 的范围切换器在滚动区内，随内容滚动）；`content.rs` 用 `GdipSetClipRect` 裁 body，命中区经 `in_content(rect, body_top, scroll)` 换成内容坐标。别把内容从 y=0 画起（会压在选项卡条上）。
- **命中坐标基准只能有一个来源**：body 顶 = `strip_h + content::header_height(dpi)`，反算用 `y - body_top() + scroll`。曾因 `body_top()` 只返回 `strip_h`（漏掉 54px 头部）导致两个范围切换器整体错位、完全点不动——任何"外部算一遍、内部再算一遍"的布局量都必须抽成共享函数。
- **绘图函数的坐标单位要写清**：`chart::draw_segmented(right, y, …)` 的 `right`/`y` 是物理像素（调用方已 `s()`），内部再对 `y` 做一次 `s()` 会在 125%/150% 缩放下把控件画低 30~40px（96dpi 下恒等，看不出来）。
- **日期轴标签按组宽绘制、对齐到柱**：30 天视图每格约 12px，按单格宽画会把 `10-19` 裁成单个字符；标签框取 `slot * label_step`，中心仍落在本柱。
- **每日柱状图的顶部留白**：`top_pad` 给最高柱的数值标签留位；柱有标签时省略轴顶刻度，否则同一数值会在同一行出现两次。
- **选项卡文字垂直居中要按墨迹类别补偿**：中英文共享基线但视觉重心不同——实测 96dpi 下 CJK 标签墨迹中心比卡片几何中心高 1.5px、纯 ASCII（服务器名/数字）低 0.5px，相差 2px。`tab_strip::tab_text_dy` 反向各移 1px；分类必须用**原始标签**（省略号 "…" 是非 ASCII，会翻转分类）。
- **Top20 面板高度恒定**：按 `content::TOP20_MAX_ROWS`（20）行计算，行数不足时顶端对齐、下方留白，切换选项卡时两个 Panel 总高度不变；名字列宽按面板内最宽名字自适应（钳制 118~160 逻辑 px），柱轨道相应收窄。选项卡条左缘用 `s(12)` 与内容区 pad 对齐；翻页箭头盒（22px）垂直居中须单独算，不能复用 28px 图标盒的 `icon_y`。
- **UI 对齐验证工具**：`tools/measure-tab-ink.ps1`（按 x 窗口测文字墨迹上下界）、`tools/scan-left-edge.ps1`（扫描左边界 x）、`tools/crop-tabs.ps1`（裁剪放大）——GDI+ 对齐问题肉眼难断，用它们拿数字。
- **圆角**：优先 `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND=2)`（Win11），失败回退 `CreateRoundRectRgn` + `SetWindowRgn`（Win10）。`SetWindowRgn` 成功后 HRGN 归系统所有，**不要 DeleteObject**。`WM_DPICHANGED` 后需重新应用。
- 对话框按钮一律用自绘 `ui/button.rs::DrawnButton`；EDIT 扁平化（边框由父窗口画）；子控件**必须经 hMenu 传 ID**（曾因此对话框全线失灵）；新对话框类必须 `register_class()`——漏了 `CreateWindowExW` 直接失败。
- **字段显隐要成对**：隐藏原生控件时对应的自绘标签也要跳过，否则留下悬空标签（密码模式下曾残留在"私钥口令"）。
- 日志只用 `log_info!/log_error!`（`util/log.rs`），不记录凭据。
- 配置 `%APPDATA%\TokenSummarizer\config.json`；镜像缓存 `%LOCALAPPDATA%\TokenSummarizer\cache\<id>\`（隐私敏感）；
  历史库 `%LOCALAPPDATA%\TokenSummarizer\history.sqlite3`；临时副本 `%LOCALAPPDATA%\TokenSummarizer\tmp\`。
- **测试夹具与截图不得带真实环境值**：单测里的 host / 用户名一律用文档地址（`192.0.2.x`，RFC 5737）等假值，
  不要从本机 `config.json` 等真实配置里复制粘贴；界面截图不入库（`/screenshots/`、`/shots*/` 已忽略，
  `tools/capture-screens.ps1` 的默认输出目录也在忽略范围内）。曾把真实内网 IP + SSH 用户名写进单测夹具并推上公开仓库。

## 历史库与增量（改刷新逻辑前必读）

- **流水线只有一条**：`prepare`（定窗口）→ 扫描（只解析窗口内）→ `apply`（写库）→ `load`（读回全历史）→ UI。UI 拿到的**永远是全历史**，每轮真正解析的只有约一天。
- **窗口起点用 `source_state.last_synced_day`（每源最后成功覆盖到的北京日），不要用 `SELECT MAX(day)`**：某源今天没用量、或上次失败时，`MAX(day)` 会给出错误起点。水位线在成功后置为**本轮开始日**（跨午夜宁可下轮多扫一天）。
- **三种 StorageMode 决定能否按天窗口**：`Window`（记录级：Claude/OpenCode/Qwen/Qoder/QoderCN/WorkBuddy）按 `day >= since` 删再插；`Snapshot`（**累积值**：Codex 线程 / Hermes 会话 / ZCode 请求）必须整源重算——按日期过滤会永久丢掉跨天长线程的后续增长；`ProbeOnly` 不产生 bucket。
- **失败绝不覆盖历史**：`!detected` / `partial` / `error.is_some()` / 同步 `failed_sources` 的源只更新状态字段，不删不插、水位线不推进。`Snapshot` 源返回空 buckets 时也不删旧行（那通常是"库暂时读不到"）。
- **镜像 epoch**：镜像根 `.mirror_epoch` 是缓存身份。清空缓存后标记消失 → `MirrorSync` 清空内存 offset 并生成新 epoch → 历史库 `prepare` 发现 epoch 变化强制全量。**缺了这条会用半套镜像按旧水位线覆盖历史。**
- 本地 target 没有镜像，`local_epoch()` 直接用身份串（`home:<路径>` / `wsl:<distro>:<unix_home>`）；路径或发行版身份变化即全量。
- **JSONL `append_complete_lines` 必须把 mtime 设回远端 mtime**：否则增量追加后 mtime 变成"现在"，看起来无害，但文件级窗口判定会永远为真，增量退化成全量重读；反过来不更新则会把仍在追加的文件误判成旧文件。文件级跳过只是省 I/O，**必须再做记录级 `ctx.in_window` 过滤**。
- 事务边界：一个 target 一轮 = 一个 `BEGIN IMMEDIATE` 事务（扫描在事务外）。中途崩溃即整体回滚，`full_scan_done` 不推进，下次重来，不会留半套历史。
- 并发：`Arc<TokenStore>` + 内部 `Mutex<Connection>`，SQL 一律在 `spawn_blocking` 里跑。不要改成"每任务一个连接"——写事务很短，串行化最简单且不会 `SQLITE_BUSY`。

## windows crate 0.62 / Win32 已知坑（本仓库踩过）

- 托盘结构是 `NOTIFYICONDATAW`；`PostMessageW` 参数是裸 WPARAM/LPARAM（不是 Option）；ES_*/BS_* 是裸 i32 需与 WINDOW_STYLE newtype 组合。
- `ReleaseCapture/SetCapture/VK_ESCAPE/EnableWindow` 在 `UI::Input::KeyboardAndMouse`；`GetMonitorInfoW/MONITORINFO/CreateBitmap` 在 `Graphics::Gdi`；`BST_*` 在 `UI::Controls`；`GdiplusStartup/GdiplusShutdown/GdipDeletePath` 命名（非 Gdip 前缀 Startup）。
- `CreateFontIndirectW/CreateBitmap` 直接返回句柄（非 Result）；`CreateCompatibleDC/CreateDIBSection/GetDlgItem` 接受/返回 Option/Result。
- `CreateWindowExW` 对子控件的 hMenu 参数即控件 ID（`HMENU(id as *mut c_void)`）。
- **DWM 圆角枚举**：`DWM_WINDOW_CORNER_PREFERENCE(2)` 才是 `DWMWCP_ROUND`；直接传 33 之类的猜测值在所有系统上静默无效（`DwmSetWindowAttribute` 返回 `Err`，必须检查）。
- **Common-Controls v6 视觉样式**：不嵌 app manifest 时复选框/单选框**完全不绘制**（只剩文字）。需要 `assets/app.manifest` 里的 `Microsoft.Windows.Common-Controls` 6.0 依赖 + `InitCommonControlsEx(ICC_STANDARD_CLASSES)`，两者缺一不可。build.rs 用 `1 24 "app.manifest"` 把它编进 exe。
- edition 2024：lib/main 顶部已 `#![allow(unsafe_op_in_unsafe_fn)]`。

## 工具脚本坑（本机环境）

- **PowerShell 5.1 脚本被安全软件静默查杀**：脚本里用 `Marshal.ReadByte/WriteByte` 逐字节写内存会触发 AV，进程无输出直接退出（exit 0/2、无任何报错）。写二进制改用纯托管 API（`Bitmap.Save(Bmp)` + 数组切片）。
- **截图的位图格式**：`PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT=2)` 必须配 `Format24bppRgb`；用 `Format32bppArgb` 接 GDI 内容时 alpha 全为 0，PNG 会整张透明（看起来纯白）。
- **`FindWindow(cls, NULL)` 在 PowerShell 里不可靠**（明明存在也返回 0）：改用 `EnumWindows` + `GetClassName` 自己筛。
- **托盘气泡失焦会自动隐藏**：截图脚本预热期间必须每秒 `SetForegroundWindow` + 判 `IsWindowVisible` 后重发 `WM_APP_SHOW`（该消息是 toggle，不能盲发）。
- **`Add-Type` 内联编译被安全策略拦截**（"compiles and loads .NET code at runtime"）：把调试脚本写成 `.ps1` 文件再 `powershell -File` 执行。
- **ICO 格式**：DIB 的 `biHeight` 必须是 `h*2`（XOR+AND 两段）——只写 h 会渲染成一半高度；AND 掩码 1bpp 行按 32 位对齐、全零表示不透明（透明度由 32bpp alpha 决定）。
- .ps1 由 heredoc 生成时注意 CRLF/编码（中文注释在无 BOM UTF-8 下会被 PS5.1 当 ANSI 解析炸掉）。截图脚本因此保持**纯 ASCII**。

## 已知限制（改前知悉）

- SSH / SFTP 已有端到端回归手段：`token_summarizer.exe --check` 会对配置里的每台服务器跑登录 + 主机密钥 + SFTP + 一轮同步，并按源汇总（失败退出码非 0）。改连接/同步逻辑后**必须**跑它。
- 主机密钥指纹可能带/不带 `SHA256:` 前缀，比较前统一走 `config::normalize_fingerprint`，否则会误报「密钥已更改」。
- Win11 新托盘图标默认在隐藏图标区，需用户手动拖出；TaskbarCreated 消息已处理（Explorer 重启自动重建）。
- 详细文档：`README.md`。
