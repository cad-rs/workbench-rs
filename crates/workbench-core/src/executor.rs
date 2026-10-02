//! 平台共享异步执行器（design.md §10.1“异步执行器”一档）。
//!
//! 基于 `async-executor` 的小型固定线程池：异步命令的 future 在池上轮转，
//! 避免为每个轻量异步命令独占线程（那是 `TaskKind::Thread` 后台任务的职责）。
//! 应用关闭时通过 shutdown 标志让工作线程退出。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_executor::Executor;

pub struct AsyncExecutor {
    inner: Arc<Executor<'static>>,
    _threads: usize,
}

impl AsyncExecutor {
    /// 创建执行器并启动工作线程。
    pub fn new(threads: usize, shutdown: Arc<AtomicBool>) -> Self {
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

    /// 提交一个 fire-and-forget 异步任务（完成后自行发送终态事件）。
    pub fn spawn_detached<F>(&self, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.inner.spawn(future).detach();
    }
}
