#![cfg_attr(target_family = "wasm", allow(dead_code))]

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fedimint_core::time::now;
use fedimint_logging::LOG_TASK;
#[cfg(target_family = "wasm")]
use futures::channel::oneshot;
use futures::future::BoxFuture;
use futures::lock::Mutex;
pub use imp::*;
use thiserror::Error;
#[cfg(not(target_family = "wasm"))]
use tokio::sync::oneshot;
#[cfg(not(target_family = "wasm"))]
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

#[cfg(target_family = "wasm")]
type JoinHandle<T> = futures::future::Ready<anyhow::Result<T>>;

#[derive(Debug, Error)]
#[error("deadline has elapsed")]
pub struct Elapsed;

#[derive(Debug, Default)]
struct TaskGroupInner {
    /// Was the shutdown requested, either externally or due to any task
    /// failure?
    is_shutting_down: AtomicBool,
    #[allow(clippy::type_complexity)]
    on_shutdown: Mutex<Vec<Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send + 'static>>>,
    join: Mutex<VecDeque<(String, JoinHandle<()>)>>,
}

impl TaskGroupInner {
    pub async fn shutdown(&self) {
        // Note: set the flag before starting to call shutdown handlers
        // to avoid confusion.
        self.is_shutting_down.store(true, SeqCst);

        loop {
            let f_opt = self.on_shutdown.lock().await.pop();

            if let Some(f) = f_opt {
                f().await;
            } else {
                break;
            }
        }
    }
}
/// A group of task working together
///
/// Using this struct it is possible to spawn one or more
/// main thread collabarating, which can cooperatively gracefully
/// shut down, either due to external request, or failure of
/// one of them.
///
/// Each thread should periodically check [`TaskHandle`] or rely
/// on condition like channel disconnection to detect when it is time
/// to finish.
#[derive(Clone, Default, Debug)]
pub struct TaskGroup {
    inner: Arc<TaskGroupInner>,
}

impl TaskGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn make_handle(&self) -> TaskHandle {
        TaskHandle {
            inner: self.inner.clone(),
        }
    }

    /// Create a sub-group
    ///
    /// Task subgroup works like an independent [`TaskGroup`], but the parent
    /// `TaskGroup` will propagate the shut down signal to a sub-group.
    ///
    /// In contrast to using the parent group directly, a subgroup allows
    /// calling [`Self::join_all`] and detecting any panics on just a
    /// subset of tasks.
    ///
    /// The code create a subgroup is responsible for calling
    /// [`Self::join_all`]. If it won't, the parent subgroup **will not**
    /// detect any panics in the tasks spawned by the subgroup.
    pub async fn make_subgroup(&self) -> TaskGroup {
        let new_tg = Self::new();
        self.make_handle()
            .on_shutdown({
                let new_tg = self.clone();
                Box::new(move || {
                    Box::pin(async move {
                        new_tg.shutdown().await;
                    })
                })
            })
            .await;

        new_tg
    }

    pub async fn shutdown(&self) {
        self.inner.shutdown().await
    }

    pub async fn shutdown_join_all(
        self,
        join_timeout: Option<Duration>,
    ) -> Result<(), anyhow::Error> {
        self.shutdown().await;
        self.join_all(join_timeout).await
    }

    #[cfg(not(target_family = "wasm"))]
    pub fn install_kill_handler(&self) {
        use tokio::signal;

        async fn wait_for_shutdown_signal() {
            let ctrl_c = async {
                signal::ctrl_c()
                    .await
                    .expect("failed to install Ctrl+C handler");
            };

            #[cfg(unix)]
            let terminate = async {
                signal::unix::signal(signal::unix::SignalKind::terminate())
                    .expect("failed to install signal handler")
                    .recv()
                    .await;
            };

            #[cfg(not(unix))]
            let terminate = std::future::pending::<()>();

            tokio::select! {
                _ = ctrl_c => {},
                _ = terminate => {},
            }
        }
        tokio::spawn({
            let task_group = self.clone();
            async move {
                wait_for_shutdown_signal().await;
                info!(
                    target: LOG_TASK,
                    "signal received, starting graceful shutdown"
                );
                task_group.shutdown().await;
            }
        });
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn spawn<Fut, R>(
        &mut self,
        name: impl Into<String>,
        f: impl FnOnce(TaskHandle) -> Fut + Send + 'static,
    ) -> oneshot::Receiver<R>
    where
        Fut: Future<Output = R> + Send + 'static,
        R: Send + 'static,
    {
        let name = name.into();
        let mut guard = TaskPanicGuard {
            name: name.clone(),
            inner: self.inner.clone(),
            completed: false,
        };
        let handle = self.make_handle();

        let (tx, rx) = oneshot::channel();
        if let Some(handle) = self::imp::spawn(async move {
            // if receiver is not interested, just drop the message
            let _ = tx.send(f(handle).await);
        }) {
            self.inner.join.lock().await.push_back((name, handle));
        }
        guard.completed = true;

        rx
    }

    #[cfg(not(target_family = "wasm"))]
    pub async fn spawn_local<Fut>(
        &mut self,
        name: impl Into<String>,
        f: impl FnOnce(TaskHandle) -> Fut + 'static,
    ) where
        Fut: Future<Output = ()> + 'static,
    {
        let name = name.into();
        let mut guard = TaskPanicGuard {
            name: name.clone(),
            inner: self.inner.clone(),
            completed: false,
        };
        let handle = self.make_handle();

        if let Some(handle) = self::imp::spawn_local(async move {
            f(handle).await;
        }) {
            self.inner.join.lock().await.push_back((name, handle));
        }
        guard.completed = true;
    }
    // TODO: Send vs lack of Send bound; do something about it
    #[cfg(target_family = "wasm")]
    pub async fn spawn<Fut, R>(
        &mut self,
        name: impl Into<String>,
        f: impl FnOnce(TaskHandle) -> Fut + 'static,
    ) -> oneshot::Receiver<R>
    where
        Fut: Future<Output = R> + 'static,
        R: 'static,
    {
        let name = name.into();
        let mut guard = TaskPanicGuard {
            name: name.clone(),
            inner: self.inner.clone(),
            completed: false,
        };
        let handle = self.make_handle();

        let (tx, rx) = oneshot::channel();
        if let Some(handle) = self::imp::spawn(async move {
            let _ = tx.send(f(handle).await);
        }) {
            self.inner.join.lock().await.push_back((name, handle));
        }
        guard.completed = true;

        rx
    }

    pub async fn join_all(self, timeout: Option<Duration>) -> Result<(), anyhow::Error> {
        let mut errors = vec![];

        let deadline = timeout.map(|timeout| now() + timeout);

        while let Some((name, join)) = self.inner.join.lock().await.pop_front() {
            info!(target: LOG_TASK, task=%name, "Waiting for task to finish");

            let timeout = deadline.map(|deadline| {
                deadline
                    .duration_since(now())
                    .unwrap_or(Duration::from_millis(10))
            });

            #[cfg(not(target_family = "wasm"))]
            let join_future: Pin<Box<dyn Future<Output = _> + Send>> =
                if let Some(timeout) = timeout {
                    Box::pin(self::timeout(timeout, join))
                } else {
                    Box::pin(async move { Ok(join.await) })
                };

            #[cfg(target_family = "wasm")]
            let join_future: Pin<Box<dyn Future<Output = _>>> = if let Some(timeout) = timeout {
                Box::pin(self::timeout(timeout, join))
            } else {
                Box::pin(async move { Ok(join.await) })
            };

            match join_future.await {
                Ok(Ok(())) => {
                    info!(target: LOG_TASK, task=%name, "Task finished");
                }
                Ok(Err(e)) => {
                    error!(target: LOG_TASK, task=%name, error=%e, "Task panicked");
                    errors.push(e);
                }
                Err(Elapsed) => {
                    warn!(
                        target: LOG_TASK, task=%name,
                        "Timeout waiting for task to shut down"
                    )
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            let num_errors = errors.len();
            Err(anyhow::Error::msg(format!(
                "{num_errors} tasks did not finish cleanly: {errors:?}"
            )))
        }
    }
}

pub struct TaskPanicGuard {
    name: String,
    inner: Arc<TaskGroupInner>,
    /// Did the future completed successfully (no panic)
    completed: bool,
}

impl TaskPanicGuard {
    pub fn is_shutting_down(&self) -> bool {
        self.inner.is_shutting_down.load(SeqCst)
    }
}

impl Drop for TaskPanicGuard {
    fn drop(&mut self) {
        if !self.completed {
            info!(
                target: LOG_TASK,
                "Task {} shut down uncleanly. Shutting down task group.", self.name
            );
            self.inner.is_shutting_down.store(true, SeqCst);
        }
    }
}

#[derive(Clone, Debug)]
pub struct TaskHandle {
    inner: Arc<TaskGroupInner>,
}

impl TaskHandle {
    /// Is task group shutting down?
    ///
    /// Every task in a task group should detect and stop if `true`.
    pub fn is_shutting_down(&self) -> bool {
        self.inner.is_shutting_down.load(SeqCst)
    }

    /// Run `f` on shutdown.
    ///
    /// If TaskGroup is already shuting down, run the function immediately.
    pub async fn on_shutdown(
        &self,
        // f: FnOnce() -> BoxFuture<'static, ()> + Send + 'static
        f: Box<dyn FnOnce() -> BoxFuture<'static, ()> + Send + 'static>,
    ) {
        // NOTE: hold the lock to avoid race if shutdown happens immediately after
        // checking here.
        let mut on_shutdown = self.inner.on_shutdown.lock().await;
        if self.is_shutting_down() {
            // release lock before calling f.
            drop(on_shutdown);
            f().await;
        } else {
            on_shutdown.push(f);
        }
    }

    /// Make a [`oneshot::Receiver`] that will fire on shutdown
    ///
    /// Tasks can use `select` on the return value to handle shutdown
    /// signal during otherwise blocking operation.
    pub async fn make_shutdown_rx(&self) -> oneshot::Receiver<()> {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        self.on_shutdown(Box::new(|| {
            Box::pin(async {
                let _ = shutdown_tx.send(());
            })
        }))
        .await;

        shutdown_rx
    }
}

#[cfg(not(target_family = "wasm"))]
mod imp {
    pub use tokio::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

    use super::*;

    pub(crate) fn spawn<F>(future: F) -> Option<JoinHandle<()>>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        Some(tokio::spawn(future))
    }

    pub(crate) fn spawn_local<F>(future: F) -> Option<JoinHandle<()>>
    where
        F: Future<Output = ()> + 'static,
    {
        Some(tokio::task::spawn_local(future))
    }

    pub fn block_in_place<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        tokio::task::block_in_place(f)
    }

    pub async fn sleep(duration: Duration) {
        tokio::time::sleep(duration).await
    }

    pub async fn sleep_until(deadline: Instant) {
        tokio::time::sleep_until(deadline.into()).await
    }

    pub async fn timeout<T>(duration: Duration, future: T) -> Result<T::Output, Elapsed>
    where
        T: Future,
    {
        tokio::time::timeout(duration, future)
            .await
            .map_err(|_| Elapsed)
    }
}

#[cfg(target_family = "wasm")]
mod imp {
    pub use async_lock::{RwLock, RwLockReadGuard, RwLockWriteGuard};
    use futures::FutureExt;

    use super::*;

    pub(crate) fn spawn<F>(future: F) -> Option<JoinHandle<()>>
    where
        // No Send needed on wasm
        F: Future<Output = ()> + 'static,
    {
        wasm_bindgen_futures::spawn_local(future);
        None
    }

    pub(crate) fn spawn_local<F>(future: F) -> Option<JoinHandle<()>>
    where
        // No Send needed on wasm
        F: Future<Output = ()> + 'static,
    {
        self::spawn(future)
    }

    pub fn block_in_place<F, R>(f: F) -> R
    where
        F: FnOnce() -> R,
    {
        // no such hint on wasm
        f()
    }

    pub async fn sleep(duration: Duration) {
        gloo_timers::future::sleep(duration.min(Duration::from_millis(i32::MAX as _))).await
    }

    pub async fn sleep_until(deadline: Instant) {
        sleep(deadline.saturating_duration_since(Instant::now())).await
    }

    pub async fn timeout<T>(duration: Duration, future: T) -> Result<T::Output, Elapsed>
    where
        T: Future,
    {
        futures::pin_mut!(future);
        futures::select_biased! {
            value = future.fuse() => Ok(value),
            _ = sleep(duration).fuse() => Err(Elapsed),
        }
    }
}

/// async trait that use MaybeSend
///
/// # Example
///
/// ```rust
/// use fedimint_core::{apply, async_trait_maybe_send};
/// #[apply(async_trait_maybe_send!)]
/// trait Foo {
///     // methods
/// }
///
/// #[apply(async_trait_maybe_send!)]
/// impl Foo for () {
///     // methods
/// }
/// ```
#[macro_export]
macro_rules! async_trait_maybe_send {
    ($($tt:tt)*) => {
        #[cfg_attr(not(target_family = "wasm"), ::async_trait::async_trait)]
        #[cfg_attr(target_family = "wasm", ::async_trait::async_trait(?Send))]
        $($tt)*
    };
}

/// MaybeSync can not be used in `dyn $Trait + MaybeSend`
///
/// # Example
///
/// ```rust
/// use std::any::Any;
///
/// use fedimint_core::{apply, maybe_add_send};
/// type Foo = maybe_add_send!(dyn Any);
/// ```
#[cfg(not(target_family = "wasm"))]
#[macro_export]
macro_rules! maybe_add_send {
    ($($tt:tt)*) => {
        $($tt)* + Send
    };
}

/// MaybeSync can not be used in `dyn $Trait + MaybeSend`
///
/// # Example
///
/// ```rust
/// type Foo = maybe_add_send!(dyn Any);
/// ```
#[cfg(target_family = "wasm")]
#[macro_export]
macro_rules! maybe_add_send {
    ($($tt:tt)*) => {
        $($tt)*
    };
}

/// See `maybe_add_send`
#[cfg(not(target_family = "wasm"))]
#[macro_export]
macro_rules! maybe_add_send_sync {
    ($($tt:tt)*) => {
        $($tt)* + Send + Sync
    };
}

/// See `maybe_add_send`
#[cfg(target_family = "wasm")]
#[macro_export]
macro_rules! maybe_add_send_sync {
    ($($tt:tt)*) => {
        $($tt)*
    };
}

/// `MaybeSend` is no-op on wasm and `Send` on non wasm.
///
/// On wasm, most types don't implement `Send` because JS types can not sent
/// between workers directly.
#[cfg(target_family = "wasm")]
pub trait MaybeSend {}

/// `MaybeSend` is no-op on wasm and `Send` on non wasm.
///
/// On wasm, most types don't implement `Send` because JS types can not sent
/// between workers directly.
#[cfg(not(target_family = "wasm"))]
pub trait MaybeSend: Send {}

#[cfg(not(target_family = "wasm"))]
impl<T: Send> MaybeSend for T {}

#[cfg(target_family = "wasm")]
impl<T> MaybeSend for T {}

/// `MaybeSync` is no-op on wasm and `Sync` on non wasm.
#[cfg(target_family = "wasm")]
pub trait MaybeSync {}

/// `MaybeSync` is no-op on wasm and `Sync` on non wasm.
#[cfg(not(target_family = "wasm"))]
pub trait MaybeSync: Sync {}

#[cfg(not(target_family = "wasm"))]
impl<T: Sync> MaybeSync for T {}

#[cfg(target_family = "wasm")]
impl<T> MaybeSync for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_log::test(tokio::test)]
    async fn test_task_group() -> anyhow::Result<()> {
        let mut tg = TaskGroup::new();
        tg.spawn("shutdown waiter", |handle| async move {
            handle.make_shutdown_rx().await.await
        })
        .await;
        tg.shutdown_join_all(None).await?;
        Ok(())
    }
}
