use owo_colors::OwoColorize;

pub(crate) fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some()
}

pub(crate) fn msr() -> String {
    if no_color() {
        "MSR//".to_string()
    } else {
        "MSR//".cyan().bold().to_string()
    }
}

pub(crate) fn title(text: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        text.cyan().bold().to_string()
    }
}

pub(crate) fn value(text: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        text.white().bold().to_string()
    }
}

pub(crate) fn dimmed(text: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        text.dimmed().to_string()
    }
}

pub(crate) fn error(text: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        text.red().bold().to_string()
    }
}

pub(crate) fn success(text: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        text.green().bold().to_string()
    }
}

pub(crate) fn warning(text: &str) -> String {
    if no_color() {
        text.to_string()
    } else {
        text.yellow().bold().to_string()
    }
}
