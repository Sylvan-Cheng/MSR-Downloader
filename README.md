# MSR Downloader

Monster Siren Music Downloader - 塞壬唱片音乐下载器。

用于下载 Monster Siren 塞壬唱片专辑，支持交互式界面、命令行下载、断点续传、歌词/封面/专辑信息保存和音频元数据写入。

## 安装

从 GitHub Release 下载对应平台的预编译文件，解压后直接运行可执行文件：

```bash
msr-downloader
```

如果你已经安装 Rust，也可以从源码安装：

```bash
cargo install --path .
```

## 交互式下载

直接运行会进入 TUI 交互界面：

```bash
msr-downloader
```

常用操作：

| 操作 | 功能 |
|---|---|
| `↑` / `↓` | 移动焦点 |
| 鼠标点击 / `Space` | 选择或取消选择专辑/曲目 |
| `A` | 选择或取消选择当前过滤结果 |
| `C` | 清空选择队列 |
| `Enter` | 展开/折叠专辑，查看并选择单首曲目 |
| `d` | 开始下载选中的专辑 |
| `/` | 搜索专辑 |
| `Esc` | 清空搜索 / 关闭帮助浮层 |
| `?` | 打开帮助浮层 |
| `Tab` | 切换专辑页和下载页 |
| `Q` | 退出（传输中需确认） |

## 命令行下载

查看帮助：

```bash
msr-downloader --help
```

列出专辑：

```bash
msr-downloader --cli --list
```

下载指定专辑：

```bash
msr-downloader --cli --album "春弦" "Innocence"
```

预览匹配结果，不下载：

```bash
msr-downloader --cli --album "春弦" --dry-run
```

下载全部专辑：

```bash
msr-downloader --cli --all
```

选择专辑中的部分曲目：

```bash
msr-downloader --cli --album "春弦" --tracks 1,3,5-8
```

指定输出目录：

```bash
msr-downloader --cli --album "春弦" --output ./music
```

常用参数：

| 参数 | 说明 |
|---|---|
| `--album <name>` | 按专辑名下载，可传多个 |
| `--album-id <cid>` | 按专辑 CID 下载 |
| `--exact` | 专辑名精确匹配 |
| `--all` | 下载全部专辑 |
| `--tracks <list>` | 选择曲目，如 `1,3,5-8`（仅限 `--album`/`--album-id`） |
| `--dry-run` | 只预览，不下载或删除文件 |
| `--output <dir>` | 临时指定输出目录 |
| `--concurrency <n>` | 临时指定并发数 |
| `--plain` | 纯文本输出，适合日志 |
| `--no-progress` | 不显示实时进度，只输出摘要 |

`--cli` 本身不会开始下载，必须搭配 `--album`、`--album-id` 或 `--all`。

按 `Ctrl+C` 中止下载时，未完成文件会保留为 `.part`，下次下载同一文件会尝试续传。

## 配置

生成默认配置文件：

```bash
msr-downloader --init-config
```

检查当前配置：

```bash
msr-downloader --check-config
```

默认配置文件名为 `msr.toml`。常用配置示例：

```toml
[download]
output_dir = "./MSR_Albums"
concurrency = 2

[download.include]
lyrics = true
covers = true
album_info = true
metadata = true

[download.convert]
enabled = true
wav_to_flac = true
delete_original = true
flac_compression = 5

[naming]
album_folder = "{album_name}"
song_file = "{song_name}.{ext}"
```

查看最终生效配置：

```bash
msr-downloader --print-config
```

## 清理断点文件

预览输出目录中的 `.part` 文件：

```bash
msr-downloader --clean-parts --dry-run
```

删除输出目录中的 `.part` 文件：

```bash
msr-downloader --clean-parts --yes
```

## 注意事项

- 默认下载到 `./MSR_Albums`。
- 网络错误会自动重试，下载中断后通常可以续传。
- 默认会把 WAV 自动转换为 FLAC，并写入 FLAC 元数据，避免 WAV 标签兼容性问题。
- WAV 转 FLAC 不需要额外安装工具。

## 许可证

MIT License
