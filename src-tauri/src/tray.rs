//! Menu-bar (macOS) / tray (Windows) icon. Menu items are "설정 열기" and "종료" only.
//! Closing the window hides it (see lib.rs); "종료" goes through the same exit gate as Cmd+Q,
//! so a study-time quit is rejected and recorded in one place.

use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager};

use crate::platform::{self, PlatformError};
use crate::timer::model::{Phase, TimerState};

const TRAY_ID: &str = "main-tray";
const IDLE_TOOLTIP: &str = "Study Manager";

/// What the tray shows. `title` is the menu-bar text (macOS only); `tooltip` is the fallback
/// everywhere else and carries the same information.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayLabel {
    pub title: Option<String>,
    pub tooltip: String,
}

/// `MM:SS` under an hour, `H:MM` from an hour on (seconds are noise at that size). Seconds round
/// up like the in-window countdown, so both show the same value.
pub fn format_remaining(ms: i64) -> String {
    let total = (ms.max(0) + 999) / 1000;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 { format!("{hours}:{minutes:02}") } else { format!("{minutes:02}:{seconds:02}") }
}

pub fn tray_label(state: &TimerState) -> TrayLabel {
    let name = match state.phase {
        Phase::Idle => return TrayLabel { title: None, tooltip: IDLE_TOOLTIP.to_owned() },
        Phase::Study => "공부",
        Phase::Break => "휴식",
        Phase::LongBreak => "긴 휴식",
    };
    let left = format_remaining(state.planned_end_ms.map_or(0, |end| end - state.now_ms));
    TrayLabel {
        title: Some(format!("{left} {name}")),
        tooltip: format!("{name} · {left} 남음 ({}/{}세트)", state.set_index, state.sets_total),
    }
}

/// Pushes `label` to the tray when it differs from the last one applied. The tooltip always
/// works; a platform without tray text (Windows) is reported as `Unsupported` and keeps the
/// tooltip only.
pub fn apply_label(app: &AppHandle, applied: &mut Option<TrayLabel>, label: TrayLabel) {
    if applied.as_ref() == Some(&label) {
        return;
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        tracing::warn!("tray icon missing; remaining time not shown");
        return;
    };
    if let Err(err) = tray.set_tooltip(Some(label.tooltip.as_str())) {
        tracing::warn!(error = %err, "failed to update tray tooltip");
    }
    match platform::set_tray_title(&tray, label.title.as_deref()) {
        Ok(()) | Err(PlatformError::Unsupported(_)) => {}
        Err(err) => tracing::warn!(error = %err, "failed to update tray title"),
    }
    *applied = Some(label);
}

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let open_settings = MenuItem::with_id(app, "open_settings", "설정 열기", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "종료", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_settings, &quit])?;
    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(IDLE_TOOLTIP)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "open_settings" => show_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

pub fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if let Err(err) = window.show() {
            tracing::error!(error = %err, "failed to show main window");
        }
        if let Err(err) = window.set_focus() {
            tracing::warn!(error = %err, "failed to focus main window");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(phase: Phase, end: Option<i64>, now: i64) -> TimerState {
        TimerState {
            phase,
            run_id: Some(1),
            segment_id: Some(1),
            set_index: 2,
            sets_total: 3,
            planned_end_ms: end,
            lock_held: true,
            now_ms: now,
            next: None,
        }
    }

    #[test]
    fn remaining_time_format_table() {
        let table = [
            (0, "00:00"),
            (-5_000, "00:00"),
            (1, "00:01"),
            (59_000, "00:59"),
            (59_001, "01:00"),
            (60_000, "01:00"),
            (25 * 60_000, "25:00"),
            (3_599_000, "59:59"),
            (3_600_000, "1:00"),
            (3_661_000, "1:01"),
            (90 * 60_000, "1:30"),
            (3 * 3_600_000, "3:00"),
        ];
        for (ms, expected) in table {
            assert_eq!(format_remaining(ms), expected, "{ms}");
        }
    }

    #[test]
    fn label_names_the_phase_and_idle_clears_the_title() {
        let study = tray_label(&state(Phase::Study, Some(125_000), 0));
        assert_eq!(study.title.as_deref(), Some("02:05 공부"));
        assert_eq!(study.tooltip, "공부 · 02:05 남음 (2/3세트)");
        let rest = tray_label(&state(Phase::Break, Some(30_000), 0));
        assert_eq!(rest.title.as_deref(), Some("00:30 휴식"));
        let long = tray_label(&state(Phase::LongBreak, Some(30_000), 0));
        assert_eq!(long.title.as_deref(), Some("00:30 긴 휴식"));
        let idle = tray_label(&state(Phase::Idle, None, 0));
        assert_eq!((idle.title, idle.tooltip.as_str()), (None, "Study Manager"));
    }

    #[test]
    fn label_text_carries_no_subject_or_plan_name() {
        let mut s = state(Phase::Study, Some(60_000), 0);
        s.next = Some(crate::timer::model::NextPlan {
            plan_id: 1,
            name: "비밀계획".into(),
            subject: "비밀과목".into(),
            date: "2026-11-05".into(),
            sets: 1,
            planned_start_ms: 0,
            planned_end_ms: 1,
        });
        let label = tray_label(&s);
        assert!(!label.tooltip.contains("비밀") && !label.title.unwrap_or_default().contains("비밀"));
    }
}
