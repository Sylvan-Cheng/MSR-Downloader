mod args;
pub mod clean;
mod commands;
pub(crate) mod config;

pub use args::Cli;
pub use clean::{clean_partial_files, CleanPartsResult};
pub use commands::{
    download_albums_by_id, download_albums_by_name, download_all, no_cli_action_error,
    validate_cli_action,
};
pub use config::{init_config_file, print_config_summary};
