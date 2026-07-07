use tauri::State;

use crate::calendar::{self, CalendarInfo, TodayCalendar};
use crate::permissions::PermState;
use crate::settings::AppSettings;
use crate::state::AppState;

/// Calendars available for the settings picker. Errors when access is not
/// granted (the picker is only reachable once the permission flow succeeded).
#[tauri::command]
#[specta::specta]
pub async fn list_calendars() -> Result<Vec<CalendarInfo>, String> {
    tauri::async_runtime::spawn_blocking(calendar::list_calendars)
        .await
        .map_err(|e| format!("Calendar query failed: {e}"))?
}

/// Today's timed events for the home view. Missing permission is a state the
/// UI renders, not an error, so it comes back inside the payload.
#[tauri::command]
#[specta::specta]
pub async fn list_todays_calendar_events(
    state: State<'_, AppState>,
) -> Result<TodayCalendar, String> {
    let settings = AppSettings::load(&state.db)?;

    let permission = calendar::authorization_state();
    if permission != PermState::Granted {
        return Ok(TodayCalendar {
            permission,
            events: Vec::new(),
        });
    }

    let events = tauri::async_runtime::spawn_blocking(move || {
        calendar::fetch_todays_events(&settings.calendar_selected_ids)
    })
    .await
    .map_err(|e| format!("Calendar query failed: {e}"))??;

    Ok(TodayCalendar {
        permission: PermState::Granted,
        events,
    })
}
