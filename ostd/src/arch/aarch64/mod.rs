pub mod serial {
    use core::fmt;

    pub fn init() {}

    #[doc(hidden)]
    pub fn _print(_args: fmt::Arguments<'_>) {}
}

pub mod syscall {
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct TrapFrame {
        pub rax: u64,
        pub rdi: u64,
        pub rsi: u64,
        pub rdx: u64,
        pub r10: u64,
        pub r8: u64,
        pub r9: u64,
        pub user_rip: u64,
        pub user_rsp: u64,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum SyscallAction {
        Continue(u64),
        ExitUser(i32),
    }

    pub type SyscallDispatcherFn = fn(&mut TrapFrame) -> SyscallAction;

    pub fn register_dispatcher(_dispatcher: SyscallDispatcherFn) {}
    pub fn read_user_bytes(_user_vaddr: u64, _dst: &mut [u8]) -> Result<(), &'static str> {
        Err("Not implemented on aarch64 stub")
    }
    pub fn write_user_bytes(_user_vaddr: u64, _src: &[u8]) -> Result<(), &'static str> {
        Err("Not implemented on aarch64 stub")
    }
    pub fn enter_ring3_at(_entry_rip: u64) -> Result<i32, &'static str> {
        Ok(0)
    }
}

pub mod topology {
    #[derive(Debug, Clone, Copy)]
    pub struct CpuTopology {
        pub logical_cores: u32,
        pub sse4_2: bool,
        pub avx: bool,
        pub avx2: bool,
        pub avx512f: bool,
    }

    impl CpuTopology {
        pub fn vendor(&self) -> &str {
            "ARM64"
        }

        pub fn brand(&self) -> &str {
            "Generic AArch64 Processor"
        }
    }

    pub fn detect() -> CpuTopology {
        CpuTopology {
            logical_cores: 1,
            sse4_2: false,
            avx: false,
            avx2: false,
            avx512f: false,
        }
    }
}

pub fn init() {}

/// Disables interrupts and halts the aarch64 processor in an infinite loop.
pub fn halt_loop() -> ! {
    loop {
        // SAFETY: `wfe` puts the ARM64 core into low-power wait state.
        unsafe {
            core::arch::asm!("wfe", options(nomem, nostack));
        }
    }
}
