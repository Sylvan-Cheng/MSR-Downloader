# MSR Downloader

Monster Siren Music Downloader - 塞壬唱片音乐下载器

## 安装

```bash
cargo build --release
```

## 使用

### TUI 模式（默认）

```bash
msr-downloader
```

### CLI 模式

```bash
# 查看帮助和版本
msr-downloader --help
msr-downloader --version

# 列出所有专辑
msr-downloader --cli --list

# 下载指定专辑
msr-downloader --cli --album "相变临界" "Innocence"

# 下载全部专辑（必须显式确认）
msr-downloader --cli --all

# 指定输出目录
msr-downloader --cli --all --output ./music

# 日志友好的纯文本输出，不使用 ANSI 光标控制
msr-downloader --cli --plain --album "相变临界"

# 只输出最终摘要
msr-downloader --cli --no-progress --album "相变临界"

# 临时覆盖并发数
msr-downloader --cli --concurrency 2 --album "相变临界"

# 查看最终配置
msr-downloader --print-config

# 预览输出目录中的 .part 断点文件
msr-downloader --clean-parts --dry-run

# 删除输出目录中的 .part 断点文件
msr-downloader --clean-parts --yes
```

`--cli` 本身不会开始下载。下载全部专辑必须显式传入 `--all`，避免误操作下载完整曲库。

### TUI 快捷键

| 快捷键 | 功能 |
|---|---|
| `↑` / `↓` | 移动专辑焦点 |
| `Space` | 选择或取消选择专辑 |
| `A` | 选择或取消选择当前过滤结果 |
| `/` | 搜索/过滤专辑 |
| `Esc` | 清空搜索 |
| `Enter` | 开始下载队列 |
| `Tab` | 在专辑页和传输页之间切换 |
| `1` / `2` | 直接切换到专辑页/传输页 |
| `Q` | 退出 |

## 配置文件

创建 `msr.toml`：

```toml
[api]
base_url = "https://monster-siren.hypergryph.com/api"
timeout = 30

[download]
output_dir = "./MSR_Albums"
concurrency = 2

[download.include]
lyrics = true        # 下载歌词
covers = true        # 下载封面
album_info = true    # 保存专辑信息
metadata = true      # 写入音频元数据

[download.convert]
enabled = false      # 启用格式转换
wav_to_flac = false  # WAV 转 FLAC
delete_original = true  # 转换后删除原文件
flac_compression = 5    # FLAC 压缩级别 (0-8)

[naming]
album_folder = "{album_name}"
song_file = "{song_name}.{ext}"
```

## 功能

- **TUI 模式**：交互式界面，多选专辑，搜索过滤，实时进度条
- **CLI 模式**：命令行操作，自动识别 TTY，日志场景使用纯文本输出
- **流式下载**：实时显示多任务下载进度、速度、ETA 和完成摘要
- **断点续传**：下载中断后保留 `.part` 文件并从断点继续
- **重试机制**：网络错误自动重试（最多 6 次）
- **状态反馈**：统一显示 `QUE/CHK/GET/RES/TAG/SKP/OK/ERR` 状态
- **元数据写入**：自动写入 ID3 标签（MP3/WAV）
- **格式转换**：WAV → FLAC（纯 Rust，无外部依赖）

## 状态码

| 状态 | 含义 |
|---|---|
| `QUE` | 已加入队列，等待处理 |
| `CHK` | 正在检查本地文件完整性 |
| `GET` | 正在下载 |
| `RES` | 从 `.part` 断点文件续传 |
| `TAG` | 正在写入元数据 |
| `SKP` | 本地文件完整，已跳过 |
| `OK` | 完成 |
| `ERR` | 失败 |

## 构建建议

开发迭代优先使用：

```bash
cargo check
cargo build
```

发布构建使用：

```bash
cargo build --release
```

`release` 配置启用了 `opt-level = 3` 和 `lto = true`，构建会明显更慢。如果 Windows 提示无法删除 `target/release/msr-downloader.exe`，通常是该程序仍在运行，先关闭后再构建。

## 音频格式统计

| 格式 | 数量 | 占比 |
|------|------|------|
| WAV | 684 首 | 78.71% |
| MP3 | 183 首 | 21.06% |

## 项目结构

```
src/
├── main.rs         # 程序入口
├── api.rs          # API 客户端
├── config.rs       # TOML 配置
├── downloader.rs   # 下载逻辑
├── metadata.rs     # 元数据写入 + 格式转换
└── models.rs       # 数据模型
```

## 依赖

| 库 | 用途 |
|---|---|
| reqwest | HTTP 客户端 |
| tokio | 异步运行时 |
| futures-util | 异步流处理 |
| serde | 序列化 |
| serde_json | JSON 解析 |
| toml | TOML 配置解析 |
| clap | 命令行参数 |
| anyhow | 错误处理 |
| ratatui | TUI 框架 |
| crossterm | 终端控制 |
| owo-colors | 终端颜色 |
| id3 | MP3/WAV 元数据 |
| flacx | WAV → FLAC 转换 |

## 许可证

MIT License
