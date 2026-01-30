use active_win_pos_rs::{get_active_window, ActiveWindow};
use std::ops::Deref;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};
use std::thread::Thread;
use std::time::Duration;

pub static ACTIVE_WINDOW: RwLock<Option<ActiveWindow>> = RwLock::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);
static RUNNING_THREAD: Mutex<Option<Thread>> = Mutex::new(None);

pub fn run_thread() {
    RUNNING.store(true, Ordering::Release);
    let thread = std::thread::Builder::new()
        .name("Focus Manage Thread".to_string())
        .spawn(|| {
            while RUNNING.load(Ordering::Acquire) {
                if let Ok(focused_window) = get_active_window() {
                    if let Ok(mut now) = ACTIVE_WINDOW.write() {
                        *now = Some(focused_window);
                    }
                }
                if RUNNING.load(Ordering::Acquire) {
                    std::thread::park_timeout(Duration::from_secs_f32(1.0));
                }
            }
        });
    if let Ok(thread) = thread {
        *RUNNING_THREAD.lock().unwrap() = Some(thread.thread().clone());
    }
}

pub fn update_active() {
    if let Ok(thread) = RUNNING_THREAD.lock() {
        if let Some(thread) = thread.deref() {
            thread.unpark();
        }
    }
}
pub fn stop_thread() {
    RUNNING.store(false, Ordering::Release);
    update_active();
}
