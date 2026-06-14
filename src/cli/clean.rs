use std::path::{Path, PathBuf};

use crate::cli_style;

pub enum CleanPartsResult {
    DryRun(usize),
    Removed(usize),
}

pub fn collect_partial_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    if !dir.try_exists()? {
        return Ok(());
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let metadata = std::fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            continue;
        }

        if metadata.is_dir() {
            collect_partial_files(&path, files)?;
        } else if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".part") || name.ends_with(".part.meta"))
        {
            files.push(path);
        }
    }
    Ok(())
}

pub fn clean_partial_files(
    dir: &Path,
    dry_run: bool,
    yes: bool,
) -> anyhow::Result<CleanPartsResult> {
    let mut partial_files = Vec::new();
    collect_partial_files(dir, &mut partial_files)?;

    println!(
        "{} SCANNED {} / {} PARTIAL FILE{} FOUND",
        cli_style::msr(),
        dir.display(),
        partial_files.len(),
        if partial_files.len() == 1 { "" } else { "S" }
    );

    if dry_run {
        for file in &partial_files {
            println!("  {}", file.display());
        }
        return Ok(CleanPartsResult::DryRun(partial_files.len()));
    }

    if !partial_files.is_empty() && !yes {
        anyhow::bail!("refusing to delete partial files without --yes; use --dry-run to preview");
    }

    for file in &partial_files {
        std::fs::remove_file(file)?;
    }
    Ok(CleanPartsResult::Removed(partial_files.len()))
}
