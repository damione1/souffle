use objc2_app_kit::NSApplication;
use objc2::rc::Retained;

fn main() {
    let app = NSApplication::sharedApplication();
    let is_active = app.isActive();
    println!("active: {}", is_active);
}
