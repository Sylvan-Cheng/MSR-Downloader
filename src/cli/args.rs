use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "msr-downloader",
    version,
    about = "Monster Siren Music Downloader",
    after_help = "Examples:\n  msr-downloader\n  msr-downloader --init-config\n  msr-downloader --check-config\n  msr-downloader --cli --list\n  msr-downloader --cli --album \"春弦\"\n  msr-downloader --cli --album \"春弦\" --tracks 1,3,5-8\n  msr-downloader --cli --album \"春弦\" --exact --dry-run\n  msr-downloader --cli --album-id 123456\n  msr-downloader --cli --all --dry-run\n  msr-downloader --cli --all --output ./music\n  msr-downloader --clean-parts --dry-run\n  msr-downloader --clean-parts --yes"
)]
pub struct Cli {
    #[arg(
        short,
        long,
        value_name = "FILE",
        help_heading = "Config",
        help = "Path to msr.toml config file"
    )]
    pub config: Option<PathBuf>,
    #[arg(
        short,
        long,
        value_name = "DIR",
        help_heading = "Download",
        help = "Override download output directory"
    )]
    pub output: Option<PathBuf>,
    #[arg(short, long, num_args = 1.., value_name = "NAME", help_heading = "Download", help = "Download albums whose names contain the given text")]
    pub album: Option<Vec<String>>,
    #[arg(long, num_args = 1.., value_name = "CID", help_heading = "Download", help = "Download albums by exact album CID from --list")]
    pub album_id: Option<Vec<String>>,
    #[arg(
        long,
        help_heading = "Download",
        help = "Require --album to match album names exactly"
    )]
    pub exact: bool,
    #[arg(
        short,
        long,
        help_heading = "Download",
        help = "List available albums and exit"
    )]
    pub list: bool,
    #[arg(
        long,
        help_heading = "Download",
        help = "Download all albums; required for full-library CLI downloads"
    )]
    pub all: bool,
    #[arg(
        long,
        value_name = "LIST",
        help_heading = "Download",
        help = "Download selected 1-based tracks, e.g. 1,3,5-8"
    )]
    pub tracks: Option<String>,
    #[arg(
        long,
        help_heading = "General",
        help = "Use command-line mode instead of the default TUI"
    )]
    pub cli: bool,
    #[arg(
        long,
        help_heading = "Output",
        help = "Print periodic line-based progress; no cursor control"
    )]
    pub plain: bool,
    #[arg(
        long,
        help_heading = "Output",
        help = "Suppress progress updates; print final summaries only"
    )]
    pub no_progress: bool,
    #[arg(
        long,
        value_name = "N",
        help_heading = "Download",
        help = "Override concurrent track downloads"
    )]
    pub concurrency: Option<usize>,
    #[arg(
        long,
        help_heading = "Config",
        help = "Print resolved configuration and exit"
    )]
    pub print_config: bool,
    #[arg(
        long,
        help_heading = "Config",
        help = "Create a sample config file at --config path or msr.toml"
    )]
    pub init_config: bool,
    #[arg(
        long,
        help_heading = "Config",
        help = "Validate resolved configuration and exit"
    )]
    pub check_config: bool,
    #[arg(
        long,
        help_heading = "Maintenance",
        help = "Clean .part files from the output directory"
    )]
    pub clean_parts: bool,
    #[arg(
        long,
        help_heading = "Maintenance",
        help = "Preview cleanup targets or matched downloads without changing files"
    )]
    pub dry_run: bool,
    #[arg(
        long,
        help_heading = "Maintenance",
        help = "Confirm destructive cleanup actions"
    )]
    pub yes: bool,
}
