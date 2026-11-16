use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Safe mutual exclusion spinlock primitive provided by `ostd` for `#![forbid(unsafe_code)]` crates.
pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

// SAFETY: `SpinLock<T>` serializes all mutable access to `T` across threads/cores.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

#[inline]
fn save_flags_and_disable_irq() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        let rflags: u64;
        // SAFETY: Reading RFLAGS and disabling interrupts via `cli` in Ring 0.
        unsafe {
            core::arch::asm!(
                "pushfq",
                "pop {}",
                "cli",
                out(reg) rflags,
                options(nomem, nostack, preserves_flags)
            );
        }
        (rflags & (1 << 9)) != 0
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

#[inline]
fn restore_irq(was_enabled: bool) {
    #[cfg(target_arch = "x86_64")]
    if was_enabled {
        // SAFETY: Restoring IF=1 via `sti` only if interrupts were enabled prior to lock acquisition.
        unsafe {
            core::arch::asm!("sti", options(nomem, nostack, preserves_flags));
        }
    }
}

impl<T> SpinLock<T> {
    /// Creates a new unlocked `SpinLock<T>`.
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(value),
        }
    }

    /// Acquires the lock with IRQ-save semantics (`cli` while held), returning an RAII `SpinLockGuard`.
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        let irq_was_enabled = save_flags_and_disable_irq();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        SpinLockGuard {
            lock: self,
            irq_was_enabled,
        }
    }

    /// Attempts to acquire the lock without spinning (IRQ-safe).
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        let irq_was_enabled = save_flags_and_disable_irq();
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinLockGuard {
                lock: self,
                irq_was_enabled,
            })
        } else {
            restore_irq(irq_was_enabled);
            None
        }
    }

    /// Forcibly releases the lock.
    ///
    /// # Safety
    /// Must only be used in emergency panics or exception handlers to avoid deadlock.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

/// RAII guard that releases the `SpinLock` and restores previous `IF` state when dropped.
pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
    irq_was_enabled: bool,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Lock is exclusively held by this guard.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Lock is exclusively held by this guard.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        restore_irq(self.irq_was_enabled);
    }
}
