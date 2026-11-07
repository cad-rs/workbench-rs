//! The platform's shared async executor (the "async executor" tier of design.md §10.1).
//!
//! Desktop: a small fixed thread pool built on `async-executor`; async command futures are
//! polled on the pool, so no lightweight async command holds a thread of its own (that is
//! what `TaskKind::Thread` background tasks are for). On app shutdown a shutdown flag makes
//! the worker threads exit.
//!
//! Web (wasm32): no threads are available, so futures go straight to `wasm-bindgen-futures`
//! (driven by the main-thread event loop; async command bodies must not do blocking work).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

#[cfg(not(target_arch = "wasm32"))]
use {
    std::sync::atomic::Ordering,
    std::time::Duration,
    async_executor::Executor,
};

pub struct AsyncExecutor {
    #[cfg(not(target_arch = "wasm32"))]
    inner: Arc<Executor<'static>>,
    _threads: usize,
}

impl AsyncExecutor {
    /// Create the executor and start its worker threads (the web build creates no threads).
    pub fn new(threads: usize, shutdown: Arc<AtomicBool>) -> Self {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (threads, shutdown);
            Self { _threads: 0 }
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            let inner = Arc::new(Executor::<'static>::new());
            let workers = threads.max(1);
            for i in 0..workers {
                let ex = inner.clone();
                let flag = shutdown.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("wb-async-{i}"))
                    .spawn(move || {
                        // Drive the executor until shutdown is set, polling the submitted
                        // tasks meanwhile.
                        futures_lite::future::block_on(ex.run(async move {
                            loop {
                                if flag.load(Ordering::Relaxed) {
                                    return;
                                }
                                async_io::Timer::after(Duration::from_millis(50)).await;
                            }
                        }));
                    });
            }
            Self {
                inner,
                _threads: workers,
            }
        }
    }

    /// Submit a fire-and-forget async task (it sends its own final-state event when done).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn spawn_detached<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.inner.spawn(future).detach();
    }

    /// Submit a fire-and-forget async task. The web has only the page's main thread, so the
    /// future need not be `Send` (the JS promises storage operations await are not `Send`).
    #[cfg(target_arch = "wasm32")]
    pub fn spawn_detached<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + 'static,
    {
        let _ = &self._threads;
        wasm_bindgen_futures::spawn_local(future);
    }
}
