//! Install kit services and project Soufflé's catalogues and enums into kit options.
use crate::{
    CatalogChoices, MainWindow, MeetingExportFormat, MeetingExportMenu, PickerLabels,
    SettingsOptionList, SettingsOptions,
};
use slint::{ComponentHandle, Model, ModelExt, SharedString, VecModel};
use slint_kit::Choices;
use slint_kit::ui::{OptionRow, SelectionRef};
use strum::VariantArray;

pub(crate) fn new_window() -> Result<MainWindow, slint::PlatformError> {
    let window = MainWindow::new()?;
    slint_kit::install(&window);

    let weak = window.as_weak();
    let export_menu = window.global::<MeetingExportMenu>();
    export_menu.set_formats(VecModel::from_slice(MeetingExportFormat::VARIANTS));
    export_menu.on_rows(move |has_audio, audio_label| {
        let Some(window) = weak.upgrade() else {
            return Default::default();
        };
        let menu = window.global::<MeetingExportMenu>();
        let labels = MeetingExportFormat::VARIANTS
            .iter()
            .map(|&format| menu.invoke_label(format));
        let rows = labels
            .chain(has_audio.then_some(audio_label))
            .enumerate()
            .map(|(index, label)| OptionRow {
                token: i32::try_from(index).expect("meeting export formats fit a kit option token"),
                generation: 1,
                label,
                ..Default::default()
            })
            .collect::<Vec<_>>();
        VecModel::from_slice(&rows)
    });
    let weak = window.as_weak();
    let choices = window.global::<CatalogChoices>();
    choices.on_snapshot(move |labels, ids, current, default_device| {
        let Some(window) = weak.upgrade() else {
            return Default::default();
        };
        labels.model_tracker().track_row_count_changes();
        ids.model_tracker().track_row_count_changes();
        if labels.row_count() != ids.row_count() {
            return Default::default();
        }
        let translations = window.global::<PickerLabels>();
        let rows = (0..ids.row_count()).map(|index| {
            let id = ids.row_data_tracked(index).expect("catalogue id");
            let label = labels.row_data_tracked(index).expect("catalogue label");
            let text = if usize::try_from(default_device) == Ok(index) {
                translations.invoke_device_label(label, true)
            } else {
                translations.invoke_display(label)
            };
            (id, text.to_string())
        });
        let Ok(choices) = Choices::new(rows) else {
            return Default::default();
        };
        let selected = choices.selection(&current).or_else(|| {
            if current.is_empty() {
                return None;
            }
            // Keep the accepted key when a refresh omits it. KitSelect then
            // exposes its unavailable state without substituting a visible row.
            choices
                .retain(slint_kit::SelectOption::new(current, ""))
                .ok()
        });
        SettingsOptionList {
            rows: choices.model(),
            selected: selected.unwrap_or_default(),
        }
    });
    choices.on_index_of(|options, row| {
        options
            .iter()
            .position(|candidate| {
                !candidate.disabled
                    && candidate.token == row.token
                    && candidate.generation == row.generation
            })
            .and_then(|index| i32::try_from(index).ok())
            .unwrap_or(-1)
    });

    // Bind each contract enum to its localized projection and its checked inverse.
    let options = window.global::<SettingsOptions>();
    macro_rules! bind_options {
        ($project:ident, $label:ident, $decode:ident) => {{
            let weak = window.as_weak();
            options.$project(move |current| {
                let Some(window) = weak.upgrade() else {
                    return Default::default();
                };
                let labels = window.global::<SettingsOptions>();
                enum_options(current, |value| labels.$label(value))
            });
            options.$decode(enum_value);
        }};
    }
    bind_options!(on_locale_options, invoke_locale_label, on_locale_value);
    bind_options!(on_theme_options, invoke_theme_label, on_theme_value);
    bind_options!(
        on_paste_method_options,
        invoke_paste_method_label,
        on_paste_method_value
    );
    bind_options!(
        on_retention_options,
        invoke_retention_label,
        on_retention_value
    );
    bind_options!(
        on_meeting_language_options,
        invoke_meeting_language_label,
        on_meeting_language_value
    );
    bind_options!(
        on_provider_options,
        invoke_provider_label,
        on_provider_value
    );
    bind_options!(
        on_log_level_options,
        invoke_log_level_label,
        on_log_level_value
    );
    bind_options!(on_tab_options, invoke_tab_label, on_tab_value);
    Ok(window)
}

fn enum_options<T: Copy + PartialEq + VariantArray>(
    current: T,
    label: impl Fn(T) -> SharedString,
) -> SettingsOptionList {
    let mut selected = SelectionRef::default();
    let rows = T::VARIANTS
        .iter()
        .copied()
        .enumerate()
        .map(|(index, value)| {
            let token = i32::try_from(index).expect("settings enum fits a kit option token");
            if value == current {
                selected = SelectionRef {
                    present: true,
                    token,
                };
            }
            OptionRow {
                token,
                generation: 1,
                label: label(value),
                ..Default::default()
            }
        })
        .collect::<Vec<_>>();
    SettingsOptionList {
        rows: VecModel::from_slice(&rows),
        selected,
    }
}

fn enum_value<T: Copy + VariantArray>(row: OptionRow, current: T) -> T {
    if row.generation == 1
        && !row.disabled
        && let Ok(index) = usize::try_from(row.token)
        && let Some(value) = T::VARIANTS.get(index)
    {
        return *value;
    }
    current
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AppTheme;

    #[test]
    fn window_services_are_installed_isolated_and_released_with_the_window() {
        use slint_kit::ui::{KitFieldService, KitFormValidation};
        i_slint_backend_testing::init_no_event_loop();
        let first = new_window().unwrap();
        let second = new_window().unwrap();
        assert!(first.global::<KitFormValidation>().get_ready());
        assert!(second.global::<KitFormValidation>().get_ready());
        let fields = first.global::<KitFieldService>();
        let ticket = fields.invoke_begin_shortcut_session();
        let other = second.global::<KitFieldService>();
        other.invoke_begin_shortcut_session();
        other.invoke_invalidate_shortcut_session();
        assert!(ticket > 0);
        assert_eq!(fields.get_shortcut_session(), ticket);
        let weak = first.as_weak();
        drop(first);
        assert!(weak.upgrade().is_none());
        assert_eq!(other.get_shortcut_session(), 0);
    }

    #[test]
    fn a_translated_enum_choice_round_trips_without_comparing_labels() {
        for &theme in AppTheme::VARIANTS {
            let options = enum_options(theme, |_| "Same label".into());
            let selected = options
                .rows
                .iter()
                .find(|row| row.token == options.selected.token)
                .unwrap();
            assert_eq!(enum_value(selected, AppTheme::System), theme);
        }
    }
}
