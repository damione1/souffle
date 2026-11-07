//! Main-thread AppKit integration; no microphone, model or user profile.
#[cfg(target_os = "macos")]
fn main() {
    use objc2::{MainThreadOnly, define_class, msg_send, rc::Retained, runtime::ProtocolObject};
    use objc2_app_kit::{NSApplication, NSApplicationDelegate};
    use objc2_foundation::{NSObject, NSObjectProtocol};
    use souffle_lib::native::{bridge, termination};

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        struct SouffleQuitTestDelegate;
        unsafe impl NSObjectProtocol for SouffleQuitTestDelegate {}
        unsafe impl NSApplicationDelegate for SouffleQuitTestDelegate {}
    );

    let mtm = objc2::MainThreadMarker::new().expect("OS main thread");
    let app = NSApplication::sharedApplication(mtm);
    assert!(termination::install(mtm).is_err(), "delegate is required");
    let delegate: Retained<SouffleQuitTestDelegate> =
        unsafe { msg_send![SouffleQuitTestDelegate::alloc(mtm), init] };
    let protocol = ProtocolObject::from_ref(&*delegate);
    app.setDelegate(Some(protocol));
    let (tx, rx) = crossbeam_channel::unbounded();
    bridge::set_sink(tx);
    termination::install(mtm).unwrap();
    termination::install(mtm).expect("idempotent install");
    assert!(std::ptr::eq(&*app.delegate().unwrap(), protocol));
    for _ in 0..2 {
        // Invokes the same terminate: entry as Cmd+Q, menu and AppleEvent.
        // If AppKit exits immediately, the success marker below is absent.
        app.terminate(None);
        assert_eq!(rx.try_recv().unwrap(), bridge::NativeAction::Quit);
    }
    app.setDelegate(None);
    println!("native_termination_main_thread: terminate intercepted twice; delegate preserved");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
