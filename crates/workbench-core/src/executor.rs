//! 平台共享异步执行器（design.md §10.1“异步执行器”一档）。
//!
//! 桌面：基于 `async-executor` 的小型固定线程池：异步命令的 future 在池上轮转，
//! 避免为每个轻量异步命令独占线程（那是 `TaskKind::Thread` 后台任务的职责）。
//! 应用关闭时通过 shutdown 标志让工作线程退出。
//!
//! Web（wasm32）：无线程可用，future 直接交给 `wasm-bindgen-futures`
//! （主线程事件循环驱动；async 命令体内不得做阻塞计算）。

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
    /// 创建执行器并启动工作线程（Web 构建不创建线程）。
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
                        // 驱动执行器直到 shutdown 置位；期间持续轮转已提交的任务。
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

    /// 提交一个 fire-and-forget 异步任务（完成后自行发送终态事件）。
    pub fn spawn_detached<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = &self._threads;
            wasm_bindgen_futures::spawn_local(future);
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.inner.spawn(future).detach();
        }
    }
}
