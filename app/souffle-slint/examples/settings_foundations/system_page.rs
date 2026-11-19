//! The System page with the real host (`SystemHost`) over an in-memory backend:
//! the log file, the clipboard, the folder dialog, the sidecar, the model files and
//! the network are replaced, the archive export is the real one on a temporary folder.
use super::{save_frame, send_key, settle, system_ui};
use crate::{
    ExportStatus, MainWindow, McpStatus, SettingsTab, TranscriptionPhase, UpdateCheckStatus,
};
use i_slint_backend_testing::{AccessibleRole, ElementHandle, ElementQuery};
use slint::{ComponentHandle, platform::Key};
use slint_kit::{DirectoryHandle, FileLocation, SelectionOutcome};
use souffle_lib::update_check::UpdateCheckResult;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    task::{Context, Poll, Waker},
};
use system_ui::{LocalFuture, SystemBackend, SystemHost};

fn items(w: &MainWindow, kind: &str) -> Vec<ElementHandle> {
    ElementQuery::from_root(w).match_inherits(kind).find_all()
}

fn frame(w: &MainWindow, path: &Path, scene: &str) {
    let stem = path.file_stem().unwrap().to_str().unwrap();
    save_frame(w, &path.with_file_name(format!("{stem}-{scene}.ppm")));
}

fn button(w: &MainWindow, label: &str) -> ElementHandle {
    ElementQuery::from_root(w)
        .match_accessible_role(AccessibleRole::Button)
        .match_accessible_label(label)
        .find_first()
        .unwrap_or_else(|| panic!("no button {label:?}"))
}

/// One layout and paint pass: enough between scroll steps, where the full settle only costs time.
fn quick(w: &MainWindow) {
    slint::platform::update_timers_and_animations();
    w.window().take_snapshot().unwrap();
}

/// Wheels at the page's left edge, where no nested scroller sits under the pointer.
fn page_wheel(w: &MainWindow, page: &ElementHandle, dy: f32) {
    let p = page.absolute_position();
    w.window()
        .dispatch_event(slint::platform::WindowEvent::PointerScrolled {
            position: slint::LogicalPosition::new(p.x + 2., p.y + page.size().height / 2.),
            delta_x: 0.,
            delta_y: dy,
        });
    quick(w);
}

/// Wheels over an element's centre, as a pointer resting on it would.
fn wheel(w: &MainWindow, element: &ElementHandle, dy: f32) {
    element.scroll(0., dy);
    quick(w);
}

fn page(w: &MainWindow) -> ElementHandle {
    ElementHandle::find_by_element_id(w, "SettingsView::system-page")
        .next()
        .expect("the System page")
}

fn reveal(w: &MainWindow, element: &ElementHandle) {
    let page = page(w);
    for _ in 0..80 {
        let (p, ps) = (page.absolute_position(), page.size());
        let (e, es) = (element.absolute_position(), element.size());
        let (above, below) = (p.y - e.y, e.y + es.height - (p.y + ps.height));
        if above <= 0. && below <= 0. {
            settle(w);
            return;
        }
        let need = if above > 0. { above } else { -below };
        page_wheel(
            w,
            &page,
            need.clamp(-300., 300.).signum() * need.abs().clamp(4., 300.),
        );
    }
    panic!(
        "never revealed {:?}: element {:?} {:?}, page {:?} {:?}",
        element.accessible_label(),
        element.absolute_position(),
        element.size(),
        page.absolute_position(),
        page.size()
    );
}

/// Every focus ring on screen must sit inside the page viewport.
fn rings_fit(w: &MainWindow, what: &str) -> usize {
    let page = page(w);
    let (p, ps) = (page.absolute_position(), page.size());
    // A ring whose centre is outside the page belongs to the header, not to the page.
    let rings: Vec<_> = items(w, "FocusOutline")
        .into_iter()
        .filter(|ring| {
            let (r, rs) = (ring.absolute_position(), ring.size());
            let (cx, cy) = (r.x + rs.width / 2., r.y + rs.height / 2.);
            cx >= p.x && cx <= p.x + ps.width && cy >= p.y && cy <= p.y + ps.height
        })
        .collect();
    for ring in &rings {
        let (r, rs) = (ring.absolute_position(), ring.size());
        assert!(
            r.y >= p.y - 1. && r.y + rs.height <= p.y + ps.height + 1.,
            "{what}: ring clipped vertically {r:?} {rs:?} in {p:?} {ps:?}"
        );
        assert!(
            r.x >= p.x - 1. && r.x + rs.width <= p.x + ps.width + 1.,
            "{what}: ring clipped horizontally {r:?} {rs:?} in {p:?} {ps:?}"
        );
    }
    rings.len()
}

fn focused(w: &MainWindow) -> Option<String> {
    ["KitSwitch", "KitSelectTrigger", "KitTextField", "KitButton"]
        .into_iter()
        .flat_map(|kind| items(w, kind))
        .find(|e| {
            e.query_descendants()
                .match_inherits("FocusOutline")
                .find_first()
                .is_some()
        })
        .and_then(|e| e.accessible_label())
        .map(|label| label.to_string())
}

fn assert_widths(w: &MainWindow) {
    for kind in ["KitTextArea", "KitSelectTrigger", "KitButton"] {
        for el in items(w, kind) {
            assert!(
                el.size().width > 24.,
                "{kind} collapsed: {:?} {:?}",
                el.accessible_label(),
                el.size()
            );
        }
    }
}

fn numbered(range: std::ops::Range<usize>) -> Vec<String> {
    range
        .map(|n| {
            format!(
                "2026-11-19T10:00:{:02}Z INFO souffle: ligne {n:04} évènement",
                n % 60
            )
        })
        .collect()
}

/// A future held back while its gate is closed.
fn gated<T: 'static>(gate: &Rc<Cell<bool>>, value: impl FnOnce() -> T + 'static) -> LocalFuture<T> {
    let gate = gate.clone();
    Box::pin(async move {
        std::future::poll_fn(|_| {
            if gate.get() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        value()
    })
}

struct Fixture {
    tasks: Rc<RefCell<Vec<LocalFuture<()>>>>,
    tail: Rc<RefCell<String>>,
    copied: Rc<RefCell<Vec<String>>>,
    picks: Rc<RefCell<Vec<SelectionOutcome<DirectoryHandle>>>>,
    exports: Rc<RefCell<Vec<String>>>,
    mcp: Rc<RefCell<Result<String, String>>>,
    update: Rc<RefCell<Result<UpdateCheckResult, String>>>,
    deletes: Rc<Cell<u32>>,
    refused: Rc<RefCell<Option<String>>>,
    holds: Rc<Cell<bool>>,
}

impl Fixture {
    fn drive(&self, w: &MainWindow) {
        for _ in 0..20 {
            let pending: Vec<_> = self.tasks.borrow_mut().drain(..).collect();
            if pending.is_empty() {
                break;
            }
            let mut waiting = Vec::new();
            for mut task in pending {
                let mut context = Context::from_waker(Waker::noop());
                if task.as_mut().poll(&mut context).is_pending() {
                    waiting.push(task);
                }
            }
            let spawned = !self.tasks.borrow().is_empty();
            self.tasks.borrow_mut().extend(waiting);
            settle(w);
            if !spawned {
                break;
            }
        }
        settle(w);
    }

    fn refresh_log(&self, w: &MainWindow) {
        // Leaving the page and coming back reads the tail at once.
        let tab = w.get_settings_tab();
        w.set_settings_tab(SettingsTab::Ai);
        settle(w);
        w.set_settings_tab(tab);
        settle(w);
        self.drive(w);
    }
}

/// Scrolls the page until the control exists in the tree and sits inside its viewport.
fn seek(w: &MainWindow, find: &dyn Fn(&MainWindow) -> Option<ElementHandle>) -> ElementHandle {
    let page = page(w);
    for from_top in [false, true] {
        if from_top {
            page_wheel(w, &page, 100000.);
        }
        for _ in 0..80 {
            if let Some(element) = find(w) {
                reveal(w, &element);
                return find(w).unwrap_or(element);
            }
            page_wheel(w, &page, -240.);
        }
    }
    panic!("the control never came into the tree");
}

fn by_label(w: &MainWindow, labels: &[&str]) -> Option<ElementHandle> {
    items(w, "KitButton").into_iter().find(|e| {
        e.accessible_label()
            .is_some_and(|l| labels.contains(&l.as_str()))
    })
}

/// The log list only exists in the tree while it is near the viewport.
fn find_log_list(w: &MainWindow) -> ElementHandle {
    let page = page(w);
    page_wheel(w, &page, 100000.);
    for _ in 0..300 {
        if let Some(list) = items(w, "ListView").into_iter().next() {
            return list;
        }
        page_wheel(w, &page, -240.);
    }
    panic!("the log list never came into view");
}

fn long_path() -> String {
    format!(
        "/Applications/Soufflé.app/Contents/MacOS/{}/souffle-mcp",
        "dossier-de-sidecar-très-long/".repeat(6)
    )
}

fn wire_host(w: &MainWindow, scratch: &Path) -> (Fixture, Rc<SystemHost>) {
    let tasks: Rc<RefCell<Vec<LocalFuture<()>>>> = Rc::default();
    let tail = Rc::new(RefCell::new(String::new()));
    let copied: Rc<RefCell<Vec<String>>> = Rc::default();
    let picks: Rc<RefCell<Vec<SelectionOutcome<DirectoryHandle>>>> = Rc::default();
    let exports: Rc<RefCell<Vec<String>>> = Rc::default();
    let mcp = Rc::new(RefCell::new(Ok("search_meetings, get_meeting".to_string())));
    let update = Rc::new(RefCell::new(Ok(UpdateCheckResult {
        current_version: "1.0.0".into(),
        latest_version: None,
        update_available: false,
        release_notes: None,
        release_url: None,
        check_error: None,
    })));
    let deletes = Rc::new(Cell::new(0));
    let refused: Rc<RefCell<Option<String>>> = Rc::default();
    let holds = Rc::new(Cell::new(false));
    let database = Arc::new(souffle_lib::db::Database::open(&scratch.join("fixture.db")).unwrap());
    let backend = SystemBackend {
        fetch_log_tail: {
            let tail = tail.clone();
            Rc::new(move || {
                let tail = tail.borrow().clone();
                Box::pin(async move { Ok(tail) })
            })
        },
        diagnostics_text: Rc::new(|| {
            Box::pin(async { Ok("diagnostics de la fixture".to_string()) })
        }),
        copy_text: {
            let copied = copied.clone();
            Rc::new(move |text| copied.borrow_mut().push(text.to_string()))
        },
        choose_export_folder: {
            let (picks, holds) = (picks.clone(), holds.clone());
            Rc::new(move |_| {
                let picks = picks.clone();
                gated(&holds, move || {
                    picks
                        .borrow_mut()
                        .pop()
                        .unwrap_or(SelectionOutcome::Cancelled)
                })
            })
        },
        export_archive: {
            let (exports, database) = (exports.clone(), database.clone());
            Rc::new(move |destination| {
                exports.borrow_mut().push(destination.clone());
                let database = database.clone();
                Box::pin(async move {
                    // The command checks the destination before it starts.
                    if !Path::new(&destination).is_dir() {
                        return Err(format!("Destination is not a directory: {destination}"));
                    }
                    souffle_lib::archive::run_archive_export(
                        &database,
                        Path::new(&destination),
                        chrono::Utc::now(),
                        |_, _| {},
                    )
                    .map(|_| ())
                })
            })
        },
        reveal_data_dir: Rc::new(|| Ok(())),
        test_mcp: {
            let (mcp, holds) = (mcp.clone(), holds.clone());
            Rc::new(move || {
                let mcp = mcp.clone();
                gated(&holds, move || mcp.borrow().clone())
            })
        },
        check_updates: {
            let (update, holds) = (update.clone(), holds.clone());
            Rc::new(move || {
                let update = update.clone();
                gated(&holds, move || update.borrow().clone())
            })
        },
        delete_model: {
            let (deletes, refused) = (deletes.clone(), refused.clone());
            Rc::new(move || {
                deletes.set(deletes.get() + 1);
                refused.borrow().clone().map_or(Ok(()), Err)
            })
        },
        model_deleted: Rc::new(|| {}),
        spawn: {
            let tasks = tasks.clone();
            Rc::new(move |future| tasks.borrow_mut().push(future))
        },
        poll_interval: system_ui::LOG_POLL,
    };
    let host = SystemHost::new(w, backend);
    system_ui::wire(w, &host);
    (
        Fixture {
            tasks,
            tail,
            copied,
            picks,
            exports,
            mcp,
            update,
            deletes,
            refused,
            holds,
        },
        host,
    )
}

fn directory(path: &Path) -> DirectoryHandle {
    DirectoryHandle {
        location: FileLocation::Path(PathBuf::from(path)),
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
    }
}

pub(super) async fn system(w: &MainWindow, path: &Path) {
    let en = w.get_settings_locale() == crate::AppLocale::En;
    let t = |fr: &'static str, english: &'static str| if en { english } else { fr };
    let scratch = tempfile::tempdir().unwrap();
    let (fx, _host) = wire_host(w, scratch.path());

    // The state macOS reports for the login item decides what the switch shows.
    let macos_login_item = Rc::new(Cell::new(false));
    let (weak, acknowledged) = (w.as_weak(), macos_login_item.clone());
    w.on_settings_autostart_changed(move |asked| {
        let w = weak.unwrap();
        w.set_settings_autostart_enabled(asked && acknowledged.get());
    });

    w.set_settings_tab(SettingsTab::System);
    w.set_settings_data_stats_line(
        t(
            "12 Mo · 3 réunions · 5 dictées",
            "12 MB · 3 meetings · 5 dictations",
        )
        .into(),
    );
    w.set_settings_data_recordings_line(
        t("Enregistrements audio : 4 Mo", "Audio recordings: 4 MB").into(),
    );
    w.set_settings_app_version("1.4.2".into());
    w.set_settings_about_transcription_label("Whisper Large V3 Turbo".into());
    w.set_settings_about_summary_label("Apple Intelligence".into());
    w.set_settings_mcp_binary_path(long_path().as_str().into());
    w.set_settings_mcp_claude_desktop_snippet(
        format!(
            "{{\n  \"mcpServers\": {{\n    \"souffle\": {{ \"command\": \"{}\" }}\n  }}\n}}",
            long_path()
        )
        .as_str()
        .into(),
    );
    w.set_settings_mcp_claude_code_command(
        format!("claude mcp add souffle '{}'", long_path())
            .as_str()
            .into(),
    );
    w.set_settings_mcp_status(McpStatus::Absent);
    w.set_model_runtime_phase(TranscriptionPhase::Ready);
    w.set_settings_selected_model_label("Whisper Large V3 Turbo".into());
    settle(w);
    fx.drive(w);
    let page = page(w);
    assert_widths(w);
    frame(w, path, "system-top");

    // Launch at login: refused by macOS, then accepted; the switch follows macOS.
    let switch = |w: &MainWindow| {
        ElementQuery::from_root(w)
            .match_accessible_role(AccessibleRole::Switch)
            .match_accessible_label(t(
                "Lancer Soufflé à l'ouverture de session",
                "Launch Soufflé at login",
            ))
            .find_first()
    };
    seek(w, &switch).invoke_accessible_default_action();
    settle(w);
    assert_eq!(
        switch(w).unwrap().accessible_checked(),
        Some(false),
        "macOS refused"
    );
    macos_login_item.set(true);
    switch(w).unwrap().invoke_accessible_default_action();
    settle(w);
    assert_eq!(
        switch(w).unwrap().accessible_checked(),
        Some(true),
        "macOS accepted"
    );
    println!("login item: refused then accepted, the switch followed the state macOS reported");

    // Permissions: the dialog opens from the trigger and the focus comes back to it.
    let trigger = seek(w, &|w| {
        by_label(w, &[t("Vérifier les permissions", "Check permissions")])
    });
    trigger.invoke_accessible_default_action();
    settle(w);
    assert!(w.get_settings_permissions_dialog_open());
    frame(w, path, "system-permissions");
    send_key(w, Key::Escape.into());
    assert!(!w.get_settings_permissions_dialog_open());
    send_key(w, " ".into());
    assert!(
        w.get_settings_permissions_dialog_open(),
        "focus returned to the permissions trigger"
    );
    send_key(w, Key::Escape.into());
    println!("permissions dialog: opened, closed with Escape, focus returned (Space reopened it)");

    // Diagnostics: a long log with one very long line, only a few rows mounted.
    let mut lines = numbered(0..300);
    lines[3] = format!("ligne très longue {}", "mot ".repeat(250));
    *fx.tail.borrow_mut() = lines.join("\n");
    fx.refresh_log(w);
    let list = find_log_list(w);
    reveal(w, &list);
    let mounted = items(w, "LogLine").len();
    println!("log rows mounted for 300 lines: {mounted}");
    assert!((1..40).contains(&mounted));
    assert_widths(w);
    for line in items(w, "LogLine") {
        let right = page.absolute_position().x + page.size().width;
        assert!(
            line.absolute_position().x + line.size().width <= right + 1.,
            "log line wider than its page"
        );
    }
    frame(w, path, "system-log-top");
    for _ in 0..8 {
        wheel(w, &list, -120.);
    }
    frame(w, path, "system-log-scrolled");
    let first_visible = |w: &MainWindow| {
        let list = find_log_list(w);
        reveal(w, &list);
        let top = list.absolute_position().y;
        let mut visible: Vec<_> = items(w, "LogLine")
            .into_iter()
            .filter(|e| e.absolute_position().y + e.size().height > top)
            .collect();
        visible.sort_by(|a, b| a.absolute_position().y.total_cmp(&b.absolute_position().y));
        let first = visible.first().expect("a line in view").clone();
        (
            first.accessible_value().unwrap_or_default().to_string(),
            first.absolute_position().y,
        )
    };
    let (before_text, before_y) = first_visible(w);
    // The file grows: the lines already shown, and the reader's place, stay.
    *fx.tail.borrow_mut() = numbered(60..340).join("\n");
    fx.refresh_log(w);
    let (after_text, after_y) = first_visible(w);
    assert_eq!(after_text, before_text, "the reader's line moved");
    assert!((after_y - before_y).abs() < 1., "{before_y} then {after_y}");
    println!("log append: first visible line and its position unchanged ({before_y})");

    // Whole copy: the host's text, whatever is mounted.
    seek(w, &|w| {
        by_label(w, &[t("Copier les diagnostics", "Copy diagnostics")])
    })
    .invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(
        *fx.copied.borrow(),
        ["diagnostics de la fixture".to_string()]
    );

    // Data: cancelled, failed-free export, then the real archive export into a temporary folder.
    let export = |w: &MainWindow| by_label(w, &[t("Exporter…", "Export...")]);
    seek(w, &export).invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(w.get_settings_data_export_status(), ExportStatus::Idle);
    assert!(fx.exports.borrow().is_empty());
    let destination = scratch.path().join("exports");
    std::fs::create_dir_all(&destination).unwrap();
    fx.picks
        .borrow_mut()
        .push(SelectionOutcome::Selected(directory(&destination)));
    seek(w, &export).invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(w.get_settings_data_export_status(), ExportStatus::Succeeded);
    assert_eq!(fx.exports.borrow().len(), 1);
    assert!(
        std::fs::read_dir(&destination)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e
                .file_name()
                .to_string_lossy()
                .starts_with("souffle-export-")),
        "the archive folder is on disk"
    );
    let dest = destination.to_string_lossy().into_owned();
    seek(w, &|w| {
        items(w, "KitAlert").into_iter().find(|e| {
            e.accessible_label()
                .is_some_and(|l| l.contains(dest.as_str()))
        })
    });
    frame(w, path, "system-data-exported");
    let missing = scratch.path().join("absent");
    fx.picks
        .borrow_mut()
        .push(SelectionOutcome::Selected(directory(&missing)));
    seek(w, &export).invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(w.get_settings_data_export_status(), ExportStatus::Failed);
    seek(w, &|w| {
        items(w, "KitAlert")
            .into_iter()
            .find(|e| e.accessible_label().is_some_and(|l| l.contains("absent")))
    });
    frame(w, path, "system-data-failed");
    println!("export: cancelled, real archive written, failure shown");

    // MCP: absent, idle, failed, succeeded; the exact JSON and command are copied.
    let test = |w: &MainWindow| {
        by_label(
            w,
            &[
                "Tester la connexion",
                "Test connection",
                "Test en cours…",
                "Testing...",
            ],
        )
    };
    assert_eq!(seek(w, &test).accessible_enabled(), Some(false));
    seek(w, &|w| {
        items(w, "KitAlert").into_iter().find(|e| {
            e.accessible_label()
                .is_some_and(|l| l.starts_with(t("Binaire introuvable", "Binary not found")))
        })
    });
    frame(w, path, "system-mcp-absent");
    w.set_settings_mcp_status(McpStatus::Idle);
    settle(w);
    assert_eq!(seek(w, &test).accessible_enabled(), Some(true));
    *fx.mcp.borrow_mut() = Err("Spawn sidecar: permission refusée par le système, avec un message assez long pour passer à la ligne sur deux lignes".into());
    seek(w, &test).invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(w.get_settings_mcp_status(), McpStatus::Failed);
    frame(w, path, "system-mcp-failed");
    *fx.mcp.borrow_mut() = Ok("search_meetings, get_meeting, list_dictations".into());
    seek(w, &test).invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(w.get_settings_mcp_status(), McpStatus::Succeeded);
    frame(w, path, "system-mcp-succeeded");
    fx.copied.borrow_mut().clear();
    w.invoke_settings_mcp_copy_desktop_snippet_requested();
    w.invoke_settings_mcp_copy_code_command_requested();
    assert_eq!(
        *fx.copied.borrow(),
        [
            w.get_settings_mcp_claude_desktop_snippet().to_string(),
            w.get_settings_mcp_claude_code_command().to_string()
        ]
    );
    for block in items(w, "KitTextArea") {
        let right = page.absolute_position().x + page.size().width;
        assert!(
            block.absolute_position().x + block.size().width <= right + 1.,
            "{:?}",
            block.accessible_label()
        );
    }

    // The model deletion: not offered while loaded, a modal confirmation otherwise.
    let delete = |w: &MainWindow| {
        ElementQuery::from_root(w)
            .match_inherits("KitFieldSet")
            .match_accessible_label(t("STOCKAGE DU MODÈLE", "MODEL STORAGE"))
            .find_first()?
            .query_descendants()
            .match_accessible_role(AccessibleRole::Button)
            .match_accessible_label(t("Supprimer", "Delete"))
            .find_first()
    };
    assert_eq!(seek(w, &delete).accessible_enabled(), Some(false));
    w.set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    settle(w);
    assert_eq!(seek(w, &delete).accessible_enabled(), Some(true));
    seek(w, &delete).invoke_accessible_default_action();
    settle(w);
    assert!(w.get_settings_model_delete_confirm_open());
    frame(w, path, "system-delete-dialog");
    button(w, t("Annuler", "Cancel")).invoke_accessible_default_action();
    settle(w);
    assert!(!w.get_settings_model_delete_confirm_open());
    assert_eq!(fx.deletes.get(), 0);
    send_key(w, " ".into());
    assert!(
        w.get_settings_model_delete_confirm_open(),
        "focus returned to the delete button"
    );
    *fx.refused.borrow_mut() = Some("Cannot delete the currently loaded model.".into());
    button(w, t("Confirmer", "Confirm")).invoke_accessible_default_action();
    settle(w);
    assert_eq!(fx.deletes.get(), 1);
    seek(w, &|w| {
        items(w, "KitAlert").into_iter().find(|e| {
            e.accessible_label().as_deref() == Some("Cannot delete the currently loaded model.")
        })
    });
    frame(w, path, "system-delete-refused");
    println!(
        "model deletion: not offered while loaded, cancelled, refused verbatim, focus returned"
    );

    // About: every outcome of the update check.
    let check = |w: &MainWindow| {
        by_label(
            w,
            &[
                "Vérifier les mises à jour",
                "Check for updates",
                "Vérification…",
                "Checking...",
            ],
        )
    };
    fx.holds.set(true);
    seek(w, &check).invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(w.get_settings_update_status(), UpdateCheckStatus::Checking);
    assert_eq!(seek(w, &check).accessible_enabled(), Some(false));
    frame(w, path, "system-update-busy");
    fx.holds.set(false);
    fx.drive(w);
    assert_eq!(w.get_settings_update_status(), UpdateCheckStatus::UpToDate);
    *fx.update.borrow_mut() = Ok(UpdateCheckResult {
        current_version: "1.4.2".into(),
        latest_version: Some("1.5.0".into()),
        update_available: true,
        release_notes: None,
        release_url: None,
        check_error: None,
    });
    seek(w, &check).invoke_accessible_default_action();
    fx.drive(w);
    assert_eq!(w.get_settings_update_status(), UpdateCheckStatus::Available);
    seek(w, &check);
    frame(w, path, "system-update-available");

    // Keyboard: Tab through every stop of the page; each ring stays inside the viewport.
    *fx.tail.borrow_mut() = String::new();
    fx.refresh_log(w);
    w.set_settings_mcp_status(McpStatus::Idle);
    w.set_model_runtime_phase(TranscriptionPhase::LoadRequired);
    settle(w);
    page_wheel(w, &page, 100000.);
    let mut reached = Vec::new();
    for _ in 0..60 {
        send_key(w, Key::Tab.into());
        let rings = rings_fit(w, "tab walk");
        if rings == 0 {
            if reached.is_empty() {
                continue;
            }
            break;
        }
        reached.push(focused(w).unwrap_or_default());
    }
    println!("focus stops reached: {reached:?}");
    assert!(reached.len() >= 14, "{reached:?}");

    // A log in the tab order: how many presses to cross it.
    *fx.tail.borrow_mut() = numbered(0..60).join("\n");
    fx.refresh_log(w);
    seek(w, &|w| {
        items(w, "KitSelectTrigger")
            .into_iter()
            .find(|e| e.accessible_label().as_deref() == Some(t("Niveau de journal", "Log level")))
    });
    let mut presses = 0;
    loop {
        send_key(w, Key::Tab.into());
        presses += 1;
        if focused(w).as_deref() == Some(t("Copier les diagnostics", "Copy diagnostics"))
            || presses > 400
        {
            break;
        }
    }
    println!("tab presses from the log level to the copy button across a 60 line log: {presses}");
    frame(w, path, "system-final");
}
