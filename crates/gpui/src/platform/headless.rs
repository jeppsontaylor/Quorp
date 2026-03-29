//! Headless platform for production use (TUI, CI): GPUI entity loop without native windows.
//!
//! Uses a main-thread queue and a single background worker thread instead of
//! Cocoa / Win32 / Wayland message pumps.

use crate::{
    BackgroundExecutor, ForegroundExecutor, NoopTextSystem, Platform, PlatformDispatcher, Priority,
    RunnableVariant, ThreadTaskTimings,
};
use super::TestPlatform;
use parking_lot::{Condvar, Mutex};
use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

fn headless_trace(msg: &str) {
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/headless_trace.txt")
    {
        let _ = writeln!(file, "[HEADLESS] {}", msg);
        let _ = file.sync_all();
    }
}

struct HeadlessInner {
    main_queue: Mutex<VecDeque<RunnableVariant>>,
    main_cv: Condvar,
    shutdown: AtomicBool,
    main_thread_id: Mutex<Option<thread::ThreadId>>,
    background_sender: mpsc::Sender<RunnableVariant>,
}

/// Dispatches foreground work to a blocking main loop and background work to a dedicated thread.
pub struct HeadlessLiveDispatcher {
    inner: Arc<HeadlessInner>,
}

impl HeadlessLiveDispatcher {
    pub fn new() -> Self {
        let (background_sender, background_receiver) = mpsc::channel::<RunnableVariant>();
        let inner = Arc::new(HeadlessInner {
            main_queue: Mutex::new(VecDeque::new()),
            main_cv: Condvar::new(),
            shutdown: AtomicBool::new(false),
            main_thread_id: Mutex::new(None),
            background_sender,
        });
        thread::spawn(move || {
            while let Ok(runnable) = background_receiver.recv() {
                runnable.run();
            }
        });
        Self { inner }
    }

    fn enqueue_main(&self, runnable: RunnableVariant) {
        let mut queue = self.inner.main_queue.lock();
        queue.push_back(runnable);
        headless_trace(&format!("enqueue_main: queue len = {}", queue.len()));
        self.inner.main_cv.notify_all();
    }

    /// Processes the main queue until [`HeadlessLiveDispatcher::request_stop`] is called
    /// (typically via [`Platform::quit`]).
    ///
    /// `on_finish_launching` is enqueued as the first task rather than invoked
    /// synchronously so that foreground tasks dispatched during initialization
    /// (e.g. via `block_on`) can be serviced by the loop — preventing the
    /// deadlock that occurs when init code dispatches-and-waits while the loop
    /// hasn't started yet.
    pub fn run_main_loop(&self, on_finish_launching: Box<dyn FnOnce()>) {
        *self.inner.main_thread_id.lock() = Some(thread::current().id());

        headless_trace("run_main_loop: enqueuing on_finish_launching as first task");
        {
            let (runnable, task) = unsafe {
                async_task::Builder::new()
                    .metadata(crate::RunnableMeta {
                        location: std::panic::Location::caller(),
                    })
                    .spawn_unchecked(
                        move |_| async move {
                            on_finish_launching();
                        },
                        |runnable| {
                            // no-op schedule: we manually push the runnable below
                            drop(runnable);
                        },
                    )
            };
            task.detach();
            let mut queue = self.inner.main_queue.lock();
            queue.push_front(runnable);
        }

        headless_trace("run_main_loop: entering loop");
        loop {
            loop {
                let runnable = self.inner.main_queue.lock().pop_front();
                match runnable {
                    Some(runnable) => {
                        headless_trace("run_main_loop: running a task");
                        runnable.run();
                    }
                    None => break,
                }
            }
            if self.inner.shutdown.load(Ordering::SeqCst) {
                headless_trace("run_main_loop: shutdown requested");
                break;
            }
            let mut queue = self.inner.main_queue.lock();
            while queue.is_empty() && !self.inner.shutdown.load(Ordering::SeqCst) {
                self.inner.main_cv.wait(&mut queue);
            }
        }
    }

    pub fn request_stop(&self) {
        self.inner.shutdown.store(true, Ordering::SeqCst);
        self.inner.main_cv.notify_all();
    }

    /// App construction asserts the background executor’s “main thread” before the platform
    /// run loop starts. Pin the current thread so `Application::with_platform(headless_platform())`
    /// succeeds; the same thread should call `Application::run` (then `run_main_loop` reaffirms it).
    pub fn pin_main_thread_for_app_construction(&self) {
        *self.inner.main_thread_id.lock() = Some(thread::current().id());
    }
}

impl PlatformDispatcher for HeadlessLiveDispatcher {
    fn get_all_timings(&self) -> Vec<ThreadTaskTimings> {
        Vec::new()
    }

    fn get_current_thread_timings(&self) -> ThreadTaskTimings {
        ThreadTaskTimings {
            thread_name: None,
            thread_id: thread::current().id(),
            timings: Vec::new(),
            total_pushed: 0,
        }
    }

    fn is_main_thread(&self) -> bool {
        let current = thread::current().id();
        *self.inner.main_thread_id.lock() == Some(current)
    }

    fn dispatch(&self, runnable: RunnableVariant, _priority: Priority) {
        let _ = self.inner.background_sender.send(runnable);
    }

    fn dispatch_on_main_thread(&self, runnable: RunnableVariant, _priority: Priority) {
        headless_trace("dispatch_on_main_thread called");
        self.enqueue_main(runnable);
    }

    fn dispatch_after(&self, duration: Duration, runnable: RunnableVariant) {
        let inner = self.inner.clone();
        thread::spawn(move || {
            thread::sleep(duration);
            if inner.shutdown.load(Ordering::SeqCst) {
                return;
            }
            let mut queue = inner.main_queue.lock();
            queue.push_back(runnable);
            inner.main_cv.notify_all();
        });
    }

    fn spawn_realtime(&self, f: Box<dyn FnOnce() + Send>) {
        thread::spawn(move || {
            f();
        });
    }

    fn run_one_main_thread_task(&self) -> bool {
        let runnable = self.inner.main_queue.lock().pop_front();
        if let Some(runnable) = runnable {
            runnable.run();
            true
        } else {
            false
        }
    }
}

/// Builds a [`Platform`] suitable for headless applications (no GPU windowing).
pub fn headless_platform() -> std::rc::Rc<dyn Platform> {
    let dispatcher = Arc::new(HeadlessLiveDispatcher::new());
    dispatcher.pin_main_thread_for_app_construction();
    let background_executor = BackgroundExecutor::new(dispatcher.clone());
    let foreground_executor = ForegroundExecutor::new(dispatcher.clone());
    TestPlatform::with_headless_live(
        dispatcher,
        background_executor,
        foreground_executor,
        std::sync::Arc::new(NoopTextSystem::new()),
        None,
    )
}

#[cfg(test)]
mod run_tests {
    use super::headless_platform;
    use crate::Application;
    use std::time::Duration;

    /// Guards the TUI path: [`Application::run`] with [`headless_platform`] must return after
    /// [`App::quit`] is invoked from an async task (same shape as Quorp’s TUI shutdown).
    #[test]
    fn headless_platform_run_exits_after_quit_from_spawned_task() {
        Application::with_platform(headless_platform()).run(|cx| {
            cx.spawn(async move |async_cx| {
                async_cx.background_executor.timer(Duration::from_millis(50)).await;
                async_cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}
