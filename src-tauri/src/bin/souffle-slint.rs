slint::include_modules!();

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSString};
use slint::VecModel;
use std::rc::Rc;

fn setup_macos_menu(_mtm: MainThreadMarker) {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::runtime::AnyClass;

        let ns_app_class = AnyClass::get(c"NSApplication").unwrap();
        let app: Retained<objc2::runtime::AnyObject> = msg_send![ns_app_class, sharedApplication];
        let _: () = msg_send![&app, setActivationPolicy: 0isize];
        let ns_menu_class = AnyClass::get(c"NSMenu").unwrap();
        let ns_menu_item_class = AnyClass::get(c"NSMenuItem").unwrap();

        let main_menu: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_class, alloc], initWithTitle: &*NSString::from_str("Main Menu")];

        // App Menu
        let app_menu_item: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_item_class, alloc], initWithTitle: &*NSString::from_str("App"), action: None::<objc2::runtime::Sel>, keyEquivalent: &*NSString::from_str("")];
        let app_menu: Retained<objc2::runtime::AnyObject> =
            msg_send![msg_send![ns_menu_class, alloc], initWithTitle: &*NSString::from_str("App")];

        let quit_item: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_item_class, alloc], initWithTitle: &*NSString::from_str("Quit Soufflé"), action: objc2::sel!(terminate:), keyEquivalent: &*NSString::from_str("q")];
        let _: () = msg_send![&app_menu, addItem: &*quit_item];
        let _: () = msg_send![&app_menu_item, setSubmenu: &*app_menu];
        let _: () = msg_send![&main_menu, addItem: &*app_menu_item];

        let _: () = msg_send![&app, setMainMenu: &*main_menu];
    }
}

fn main() -> Result<(), slint::PlatformError> {
    if let Some(mtm) = MainThreadMarker::new() {
        setup_macos_menu(mtm);
    }

    let ui = AppWindow::new()?;

    // State bindings
    let ui_handle = ui.as_weak();

    ui.on_start_dictation({
        let ui_handle = ui_handle.clone();
        move || {
            if let Some(ui) = ui_handle.upgrade() {
                ui.set_current_view("dictation".into());
                ui.set_dict_final_text("Ceci est un test de dictée. ".into());
                ui.set_dict_draft_text("Et voici le texte en cours...".into());
            }
        }
    });

    ui.on_start_meeting({
        let ui_handle = ui_handle.clone();
        move || {
            if let Some(ui) = ui_handle.upgrade() {
                ui.set_current_view("meeting".into());
                let paras = Rc::new(VecModel::from(vec![
                    Paragraph {
                        speaker: "Damien".into(),
                        text: "Bonjour à tous".into(),
                        is_editing: false,
                    },
                    Paragraph {
                        speaker: "Alice".into(),
                        text: "Salut Damien".into(),
                        is_editing: false,
                    },
                ]));
                ui.set_meeting_paragraphs(paras.into());
            }
        }
    });

    ui.on_toggle_theme({
        let ui_handle = ui_handle.clone();
        move || {
            if let Some(ui) = ui_handle.upgrade() {
                ui.set_is_dark(!ui.get_is_dark());
            }
        }
    });

    ui.on_open_settings({
        move || {
            println!("Settings opened");
        }
    });

    ui.on_cancel_edit({
        move || {
            println!("Edit cancelled");
        }
    });

    ui.run()
}
