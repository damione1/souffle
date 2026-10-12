//! Route AppKit termination through the application's asynchronous save/quit
//! barrier. Keep winit's delegate: replacing it breaks its event-loop identity.
use objc2::{
    Encode,
    runtime::{AnyObject, Imp, Sel},
    sel,
};
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};
use objc2_foundation::MainThreadMarker;
use std::ffi::CString;

extern "C-unwind" fn should_terminate(
    _delegate: &AnyObject,
    _selector: Sel,
    _application: &NSApplication,
) -> NSApplicationTerminateReply {
    super::bridge::dispatch(super::bridge::NativeAction::Quit);
    // TerminateLater enters AppKit's modal run loop, where winit does not
    // drain the UI tasks needed by the save barrier. Cancel keeps its normal
    // loop running; the quit worker exits only after saving and joining.
    NSApplicationTerminateReply::TerminateCancel
}

/// Install after the native action sink and winit backend exist, before
/// running the event loop. Reinstallation is safe; an existing foreign
/// implementation is refused rather than silently replaced.
pub fn install(mtm: MainThreadMarker) -> Result<(), String> {
    let app = NSApplication::sharedApplication(mtm);
    let delegate = app.delegate().ok_or("AppKit delegate is unavailable")?;
    let object: &AnyObject = delegate.as_ref();
    let class = object.class();
    let selector = sel!(applicationShouldTerminate:);
    type Callback =
        extern "C-unwind" fn(&AnyObject, Sel, &NSApplication) -> NSApplicationTerminateReply;
    // SAFETY: Objective-C IMP erases the signature. The encoding below and
    // callback match applicationShouldTerminate: (NSUInteger, self, SEL, app).
    let implementation: Imp = unsafe { std::mem::transmute(should_terminate as Callback) };
    if let Some(method) = class.instance_method(selector) {
        if !std::ptr::fn_addr_eq(method.implementation(), implementation) {
            return Err("AppKit delegate already implements applicationShouldTerminate:".into());
        }
    } else {
        let encoding = CString::new(format!("{}@:@", NSApplicationTerminateReply::ENCODING))
            .map_err(|error| error.to_string())?;
        // SAFETY: Runtime mutation is performed on main before event dispatch;
        // no Rust layout is changed. Method signature and IMP agree above.
        let added = unsafe {
            objc2::ffi::class_addMethod(
                std::ptr::from_ref(class).cast_mut(),
                selector,
                implementation,
                encoding.as_ptr(),
            )
        };
        if !added.as_bool() {
            return Err("Could not install AppKit quit interception".into());
        }
    }
    // Preserve the exact delegate instance and refresh AppKit's optional
    // method lookup after adding the method to its already-registered class.
    app.setDelegate(Some(&delegate));
    tracing::info!("Native macOS quit routes through the application save barrier");
    Ok(())
}
