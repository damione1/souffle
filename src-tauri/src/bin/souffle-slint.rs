slint::include_modules!();

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_foundation::{MainThreadMarker, NSString};

fn setup_macos_menu(_mtm: MainThreadMarker) {
    #[cfg(target_os = "macos")]
    unsafe {
        use objc2::runtime::AnyClass;

        let ns_app_class = AnyClass::get(c"NSApplication").unwrap();
        let app: Retained<objc2::runtime::AnyObject> = msg_send![ns_app_class, sharedApplication];

        // NSApplicationActivationPolicyRegular = 0
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

        // Edit Menu
        let edit_menu_item: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_item_class, alloc], initWithTitle: &*NSString::from_str("Edit"), action: None::<objc2::runtime::Sel>, keyEquivalent: &*NSString::from_str("")];
        let edit_menu: Retained<objc2::runtime::AnyObject> =
            msg_send![msg_send![ns_menu_class, alloc], initWithTitle: &*NSString::from_str("Edit")];

        let cut_item: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_item_class, alloc], initWithTitle: &*NSString::from_str("Cut"), action: objc2::sel!(cut:), keyEquivalent: &*NSString::from_str("x")];
        let _: () = msg_send![&edit_menu, addItem: &*cut_item];

        let copy_item: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_item_class, alloc], initWithTitle: &*NSString::from_str("Copy"), action: objc2::sel!(copy:), keyEquivalent: &*NSString::from_str("c")];
        let _: () = msg_send![&edit_menu, addItem: &*copy_item];

        let paste_item: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_item_class, alloc], initWithTitle: &*NSString::from_str("Paste"), action: objc2::sel!(paste:), keyEquivalent: &*NSString::from_str("v")];
        let _: () = msg_send![&edit_menu, addItem: &*paste_item];

        let select_all_item: Retained<objc2::runtime::AnyObject> = msg_send![msg_send![ns_menu_item_class, alloc], initWithTitle: &*NSString::from_str("Select All"), action: objc2::sel!(selectAll:), keyEquivalent: &*NSString::from_str("a")];
        let _: () = msg_send![&edit_menu, addItem: &*select_all_item];

        let _: () = msg_send![&edit_menu_item, setSubmenu: &*edit_menu];
        let _: () = msg_send![&main_menu, addItem: &*edit_menu_item];

        let _: () = msg_send![&app, setMainMenu: &*main_menu];
    }
}

fn main() -> Result<(), slint::PlatformError> {
    if let Some(mtm) = MainThreadMarker::new() {
        setup_macos_menu(mtm);
    }

    let ui = AppWindow::new()?;

    let ui_handle = ui.as_weak();
    ui.on_button_clicked(move || {
        if let Some(ui) = ui_handle.upgrade() {
            ui.set_text_state("Clicked!".into());
        }
    });

    ui.run()
}
