#![forbid(unsafe_code)]

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use alloc::task::Wake;
use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::task::{Context, Poll, Waker};
use ostd::sync::SpinLock;

/// Unique identifier for a kernel asynchronous task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u64);

static NEXT_TASK_ID: AtomicU64 = AtomicU64::new(1);

impl TaskId {
    /// Allocates a new globally unique `TaskId`.
    pub fn allocate() -> Self {
        Self(NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed))
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "task:#{}", self.0)
    }
}

/// Stackless asynchronous kernel task wrapping a pinned `Future`.
pub struct Task {
    id: TaskId,
    future: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
}

impl Task {
    /// Creates a new asynchronous `Task` from any `Future<Output = ()> + Send + 'static`.
    pub fn new(future: impl Future<Output = ()> + Send + 'static) -> Self {
        Self {
            id: TaskId::allocate(),
            future: Box::pin(future),
        }
    }

    #[inline]
    pub fn id(&self) -> TaskId {
        self.id
    }

    fn poll(&mut self, context: &mut Context<'_>) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}

struct TaskWaker {
    task_id: TaskId,
    ready_queue: Arc<SpinLock<VecDeque<TaskId>>>,
}

impl TaskWaker {
    fn new_waker(task_id: TaskId, ready_queue: Arc<SpinLock<VecDeque<TaskId>>>) -> Waker {
        Waker::from(Arc::new(Self {
            task_id,
            ready_queue,
        }))
    }

    fn wake_task(&self) {
        self.ready_queue.lock().push_back(self.task_id);
    }
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.wake_task();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.wake_task();
    }
}

/// Cooperative `async/await` kernel executor managing a task table, `Waker` cache, and ready queue.
pub struct Executor {
    tasks: BTreeMap<TaskId, Task>,
    ready_queue: Arc<SpinLock<VecDeque<TaskId>>>,
    waker_cache: BTreeMap<TaskId, Waker>,
}

impl Executor {
    /// Creates a new empty kernel `Executor`.
    pub fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            ready_queue: Arc::new(SpinLock::new(VecDeque::new())),
            waker_cache: BTreeMap::new(),
        }
    }

    /// Spawns a new `Future` onto the executor's ready queue without blocking the caller.
    pub fn spawn(&mut self, future: impl Future<Output = ()> + Send + 'static) -> TaskId {
        let task = Task::new(future);
        let id = task.id();
        self.tasks.insert(id, task);
        self.ready_queue.lock().push_back(id);
        id
    }

    /// Polls all currently ready tasks in the queue once without blocking.
    /// Returns the number of `poll` invocations performed.
    pub fn poll_ready(&mut self) -> usize {
        let mut polled_count = 0;
        let snapshot_len = self.ready_queue.lock().len();

        for _ in 0..snapshot_len {
            let Some(task_id) = self.ready_queue.lock().pop_front() else {
                break;
            };

            let Some(task) = self.tasks.get_mut(&task_id) else {
                continue;
            };

            let waker = self
                .waker_cache
                .entry(task_id)
                .or_insert_with(|| TaskWaker::new_waker(task_id, Arc::clone(&self.ready_queue)));

            let mut cx = Context::from_waker(waker);
            polled_count += 1;

            match task.poll(&mut cx) {
                Poll::Ready(()) => {
                    self.tasks.remove(&task_id);
                    self.waker_cache.remove(&task_id);
                }
                Poll::Pending => {}
            }
        }

        polled_count
    }

    /// Cooperatively drives all ready tasks until the ready queue is drained or all tasks finish.
    /// Returns the total number of `poll` steps executed.
    pub fn run_until_idle(&mut self) -> usize {
        let mut total_polls = 0;
        while !self.ready_queue.lock().is_empty() {
            total_polls += self.poll_ready();
        }
        total_polls
    }

    /// Returns the number of active (unfinished) tasks in the executor.
    #[inline]
    pub fn active_task_count(&self) -> usize {
        self.tasks.len()
    }
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

/// Future that yields execution once back to the kernel `Executor`, allowing other
/// queued tasks to make progress cooperatively.
pub struct YieldNow {
    yielded: bool,
}

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.yielded {
            Poll::Ready(())
        } else {
            self.yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

/// Cooperatively yields the current asynchronous task once, re-scheduling it via its `Waker`.
pub const fn yield_now() -> YieldNow {
    YieldNow { yielded: false }
}
