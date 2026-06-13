pub fn progress_bar(ratio: f64, width: usize) -> String {
    let ratio = ratio.clamp(0.0, 1.0);
    let units = (ratio * width as f64).floor() as usize;
    let filled = if ratio >= 1.0 {
        width
    } else {
        units.min(width.saturating_sub(1))
    };
    let head = if ratio > 0.0 && ratio < 1.0 {
        "╸"
    } else {
        ""
    };
    let empty = width.saturating_sub(filled + head.chars().count());
    format!("{}{}{}", "─".repeat(filled), head, "·".repeat(empty))
}

pub fn progress_ratio(bytes_downloaded: u64, total_bytes: u64) -> f64 {
    if total_bytes > 0 {
        (bytes_downloaded as f64 / total_bytes as f64).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

pub fn format_bytes(bytes: u64) -> String {
    let mb = bytes as f64 / 1024.0 / 1024.0;
    format!("{:.1} MB", mb)
}

pub fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let seconds = seconds % 60;
    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_ratio_handles_empty_totals() {
        assert_eq!(progress_ratio(10, 0), 0.0);
        assert_eq!(progress_ratio(5, 10), 0.5);
        assert_eq!(progress_ratio(20, 10), 1.0);
    }

    #[test]
    fn progress_bar_has_requested_width() {
        assert_eq!(progress_bar(0.0, 4).chars().count(), 4);
        assert_eq!(progress_bar(0.5, 4).chars().count(), 4);
        assert_eq!(progress_bar(1.0, 4).chars().count(), 4);
    }
}
