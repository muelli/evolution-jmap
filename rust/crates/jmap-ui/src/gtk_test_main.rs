// SPDX-FileCopyrightText: 2026 Tobias Mueller <muelli@cryptobitch.de>
// SPDX-License-Identifier: GPL-3.0-or-later

//! The one thread that runs every GTK test of this crate.
//!
//! GTK is not thread-safe, and the default test harness gives every test its
//! own thread. Two problems follow, and this module closes both.
//!
//! First, two tests that build or destroy a GTK object at the same time can
//! corrupt process-wide GTK state. CTest hides this for the installed suite
//! with `--test-threads=1` (`cmake/Rust.cmake`); a plain `cargo test -p
//! jmap-ui` does not.
//!
//! Second, and worse, [`crate::dispatch::spawn_for`] sends a worker thread's
//! answer to the global default `GMainContext`. GLib runs an invoked
//! function on the calling thread when no thread owns that context. In
//! Evolution the GTK main loop owns it, so an answer always waits in a queue
//! and runs after the current main-loop callback. Under `cargo test` nothing
//! owns it, so a worker thread runs the answer itself, on its own thread, at
//! any time: during the destruction of the page it reports to, or during the
//! next test.
//!
//! The thread here supplies what the shell supplies. It takes the global
//! default context one time and keeps it for the life of the process, then
//! iterates it forever. [`run`] gives it a test body through that same
//! context and waits for the answer. Thus each test body and each worker
//! answer runs on this one thread, in the order the context queues them,
//! which is the order Evolution gives them.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use glib_sys::{GFALSE, GTRUE, g_main_context_acquire, g_main_context_iteration};

/// The test thread, started one time.
static GTK_THREAD: OnceLock<()> = OnceLock::new();

/// One test body at a time. A body can run a nested main loop of its own
/// (`gtk_dialog_run` does), and that loop dispatches the same context. Thus,
/// without this, a second body would start inside the first one.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// Eight megabytes, the stack the test harness gives a test thread. A GTK
/// test body must not get less because it moved to this thread.
const STACK_SIZE: usize = 8 * 1024 * 1024;

/// Starts the GTK thread, and waits until it owns the global main context.
fn start() {
    GTK_THREAD.get_or_init(|| {
        let (ready, wait) = mpsc::channel();
        std::thread::Builder::new()
            .name("jmap-ui-gtk-tests".to_owned())
            .stack_size(STACK_SIZE)
            .spawn(move || {
                // SAFETY: NULL is the global main context. This thread keeps
                // it until the process ends, so no other thread can take it.
                let taken = unsafe { g_main_context_acquire(ptr::null_mut()) };
                assert_ne!(taken, GFALSE, "another thread owns the main context");
                ready.send(()).expect("the starter waits for this");
                loop {
                    // SAFETY: this thread owns the context. The iteration
                    // blocks until a source, a test body included, is ready.
                    unsafe { g_main_context_iteration(ptr::null_mut(), GTRUE) };
                }
            })
            .expect("the GTK test thread must start");
        wait.recv().expect("the GTK test thread must report ready");
    });
}

/// Runs `body` on the GTK thread and waits for it.
///
/// A panic in `body`, which is what a failed assertion is, travels back to
/// the calling test thread as a panic with the same message. Thus the test
/// harness still reports the failure, and reports what failed. The message
/// travels, not the payload: the test harness reads the printed output of
/// the thread it started, so a panic that stays on the GTK thread prints
/// nowhere the harness looks.
pub(crate) fn run<F>(body: F)
where
    F: FnOnce() + Send + 'static,
{
    start();
    // A poisoned lock means an earlier body panicked. That body is over, so
    // the next one still gets its turn.
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let (done, wait) = mpsc::channel();
    crate::dispatch::on_main(move || {
        let outcome = catch_unwind(AssertUnwindSafe(body));
        done.send(outcome).expect("the caller waits for this");
    });
    match wait.recv() {
        Ok(Ok(())) => {}
        Ok(Err(panic)) => panic!("{}", message_of(&panic)),
        Err(_) => panic!("the GTK test thread lost the test body"),
    }
}

/// The message of a panic payload, as `panic!` and `assert!` make it.
fn message_of(panic: &Box<dyn Any + Send>) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    if let Some(text) = panic.downcast_ref::<String>() {
        return text.clone();
    }
    "the GTK test body panicked with no message".to_owned()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    use glib_sys::g_main_context_is_owner;

    use super::*;

    /// The body runs somewhere else, and always on the same thread.
    #[test]
    fn every_body_runs_on_the_one_gtk_thread() {
        static SEEN: Mutex<Vec<thread::ThreadId>> = Mutex::new(Vec::new());

        let here = thread::current().id();
        for _ in 0..3 {
            run(|| SEEN.lock().unwrap().push(thread::current().id()));
        }
        let seen = SEEN.lock().unwrap();
        assert_eq!(seen.len(), 3);
        assert!(seen.iter().all(|id| *id == seen[0]));
        assert_ne!(seen[0], here, "the body stayed on the calling thread");
    }

    /// The body owns the global main context, as a shell callback does.
    #[test]
    fn the_body_owns_the_global_main_context() {
        run(|| {
            // SAFETY: NULL is the global main context.
            assert_ne!(unsafe { g_main_context_is_owner(ptr::null_mut()) }, GFALSE);
        });
    }

    /// A failed assertion in the body still fails the test.
    #[test]
    fn a_panic_in_the_body_reaches_the_caller() {
        static COUNT: AtomicUsize = AtomicUsize::new(0);
        let outcome = catch_unwind(|| {
            run(|| {
                COUNT.fetch_add(1, Ordering::SeqCst);
                panic!("from the body");
            })
        });
        assert!(outcome.is_err(), "the panic did not reach the caller");
        assert_eq!(COUNT.load(Ordering::SeqCst), 1);
    }
}
