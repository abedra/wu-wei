pub mod ai_chat;
pub mod archive_confirm;
pub mod due_date_picker;
pub mod estimate_picker;
pub mod icon;
pub mod new_project;
pub mod project_picker;
pub mod project_view;
pub mod quick_capture;
pub mod settings;
pub mod shortcuts;
pub mod sidebar;
pub mod task_detail;
pub mod task_list;
pub mod theme;

/// Formats a whole-minute time estimate compactly for display: `45m`, `2h`,
/// `1h 30m`. Shared by the task list's Estimate column and the detail
/// panel's inline readout.
pub fn format_estimate(minutes: i64) -> String {
    let minutes = minutes.max(0);
    let (hours, mins) = (minutes / 60, minutes % 60);
    match (hours, mins) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::format_estimate;

    #[test]
    fn format_estimate_reads_compactly() {
        assert_eq!(format_estimate(45), "45m");
        assert_eq!(format_estimate(120), "2h");
        assert_eq!(format_estimate(90), "1h 30m");
        assert_eq!(format_estimate(0), "0m");
    }
}
