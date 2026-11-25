//! Local client work and bounded delivery without exposing transport internals to the desktop.

use std::{future::Future, io, sync::Mutex, time::Duration};

use tokio::{
    runtime,
    sync::{mpsc, watch},
    task::{AbortHandle, JoinHandle},
    time::{Instant, sleep, timeout_at},
};

/// Runtime that owns client I/O until the desktop closes.
pub struct Background {
    runtime: Mutex<Option<runtime::Runtime>>,
    spawner: Spawner,
}

impl Background {
    /// Start a worker thread outside the window's event loop.
    pub fn new() -> io::Result<Self> {
        let runtime = runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("marsa-client")
            .enable_all()
            .build()?;
        Ok(Self {
            spawner: Spawner(runtime.handle().clone()),
            runtime: Mutex::new(Some(runtime)),
        })
    }

    /// Get a non-owning handle for spawning additional work from a worker.
    pub fn spawner(&self) -> Spawner {
        self.spawner.clone()
    }

    /// Run work until it completes or its scoped handle is dropped.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> Task {
        self.spawner().spawn(future)
    }

    /// Cancel all asynchronous work without waiting for the worker on the caller's thread.
    pub fn close(&self) {
        let runtime = self
            .runtime
            .lock()
            .unwrap_or_else(|lock| lock.into_inner())
            .take();
        if let Some(runtime) = runtime {
            runtime.shutdown_background();
        }
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        self.close();
    }
}

/// Handle for creating tasks in an existing background runtime.
#[derive(Clone)]
pub struct Spawner(runtime::Handle);

impl Spawner {
    /// Schedule a scoped worker on the owning runtime.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> Task {
        Task(self.0.spawn(future))
    }
}

/// A background task that cancels its future and releases its streams on drop.
pub struct Task(JoinHandle<()>);

impl Task {
    /// Create a handle that can explicitly cancel this worker from the view.
    pub fn cancellation(&self) -> Cancellation {
        Cancellation(self.0.abort_handle())
    }
}

/// Cloneable cancellation handle for a scoped worker.
#[derive(Clone)]
pub struct Cancellation(AbortHandle);

impl Cancellation {
    /// Stop the worker and release any attachments it owns.
    pub fn cancel(&self) {
        self.0.abort();
    }
}

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Pause a reconnect attempt without blocking its worker.
pub async fn delay(duration: Duration) {
    sleep(duration).await;
}

/// Create an asynchronous bounded queue.
pub fn queue<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    let (sender, receiver) = mpsc::channel(capacity);
    (
        Sender(sender),
        Receiver {
            inner: receiver,
            deferred: None,
        },
    )
}

/// Producer that waits when its bounded queue fills.
pub struct Sender<T>(mpsc::Sender<T>);

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Sender<T> {
    /// Deliver one item, returning false if its consumer is gone.
    pub async fn send(&self, item: T) -> bool {
        self.0.send(item).await.is_ok()
    }

    /// Enqueue from an event handler without blocking. False means closed or full.
    pub fn try_send(&self, item: T) -> bool {
        self.0.try_send(item).is_ok()
    }
}

/// Consumer of bounded background updates.
pub struct Receiver<T> {
    inner: mpsc::Receiver<T>,
    deferred: Option<T>,
}

impl<T> Receiver<T> {
    /// Wait for the next item, returning none after the producer finishes.
    pub async fn next(&mut self) -> Option<T> {
        match self.deferred.take() {
            Some(item) => Some(item),
            None => self.inner.recv().await,
        }
    }

    /// Take an item that is ready now without waiting on the calling thread.
    pub fn try_next(&mut self) -> Option<T> {
        self.deferred.take().or_else(|| self.inner.try_recv().ok())
    }

    /// Collect at most one scheduled batch, keeping delivery below the configured rate.
    ///
    /// The window begins at the first item. Full batches still wait for its deadline.
    /// An item above the weight budget is delivered by itself.
    pub async fn batch(
        &mut self,
        maximum: usize,
        interval: Duration,
        budget: usize,
        weight: impl Fn(&T) -> usize,
    ) -> Option<Vec<T>> {
        let first = self.next().await?;
        let deadline = Instant::now() + interval;
        let mut size = weight(&first);
        let mut batch = vec![first];
        while batch.len() < maximum && size < budget {
            match timeout_at(deadline, self.next()).await {
                Ok(Some(item)) => {
                    size = size.saturating_add(weight(&item));
                    if size > budget {
                        self.deferred = Some(item);
                        break;
                    }
                    batch.push(item);
                }
                Ok(None) => break,
                Err(_) => return Some(batch),
            }
        }
        tokio::time::sleep_until(deadline).await;
        Some(batch)
    }
}

/// Publish the latest value without retaining the entire history for slow observers.
pub struct Latest<T>(watch::Sender<T>);

impl<T> Clone for Latest<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Latest<T> {
    /// Publish the initial value of a snapshot.
    pub fn new(value: T) -> Self {
        Self(watch::Sender::new(value))
    }

    /// Update the value under one writer lock, notifying observers if it changed.
    pub fn update(&self, update: impl FnOnce(&mut T) -> bool) {
        self.0.send_if_modified(update);
    }

    /// Start observing changes with the current value as the initial baseline.
    pub fn observe(&self) -> Observed<T> {
        Observed(self.0.subscribe())
    }
}

/// Observer of a coalesced sequence of state snapshots.
pub struct Observed<T>(watch::Receiver<T>);

impl<T> Observed<T> {
    /// Inspect the current value and mark its revision as consumed.
    pub fn read<R>(&mut self, read: impl FnOnce(&T) -> R) -> R {
        let value = self.0.borrow_and_update();
        read(&value)
    }

    /// Wait until a newer value is published or the last publisher drops.
    pub async fn changed(&mut self) -> bool {
        self.0.changed().await.is_ok()
    }
}
