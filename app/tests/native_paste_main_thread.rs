//! Runs on the real OS main thread, unlike libtest's per-test workers.
//! Resolving a Unicode key through Carbon from a worker killed the Nightly
//! after dictation Stop. Exercise the exact production dispatch without
//! posting input events, requesting permissions, or touching the pasteboard.

#[cfg(target_os = "macos")]
fn main() {
    use core_foundation::runloop::{CFRunLoop, kCFRunLoopDefaultMode};
    use std::sync::mpsc::{TryRecvError, channel};
    use std::time::{Duration, Instant};

    assert!(objc2::MainThreadMarker::new().is_some());
    let expected = souffle_lib::clipboard::test_paste_keycode().expect("main-thread layout");
    let (tx, rx) = channel();
    let worker = std::thread::spawn(move || {
        assert!(objc2::MainThreadMarker::new().is_none());
        tx.send(souffle_lib::clipboard::test_paste_keycode())
            .expect("result receiver");
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match rx.try_recv() {
            Ok(result) => {
                assert_eq!(result.expect("worker-to-main layout"), expected);
                break;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => panic!("layout lookup worker disconnected"),
        }
        assert!(
            Instant::now() < deadline,
            "main-queue layout lookup timed out"
        );
        CFRunLoop::run_in_mode(
            unsafe { kCFRunLoopDefaultMode },
            Duration::from_millis(10),
            true,
        );
    }
    worker.join().expect("layout lookup worker");
    println!("native_paste_main_thread: main + worker layout lookup passed (no input posted)");
}

#[cfg(not(target_os = "macos"))]
fn main() {}
