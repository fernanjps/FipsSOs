use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::mm::{
    vm_space::{
        map_user_segment, read_user_bytes as vm_read_user_bytes,
        write_user_bytes as vm_write_user_bytes, USER_SPACE_LOW_BASE, USER_STACK_DEFAULT_BASE,
    },
    PAGE_SIZE,
};
use crate::sync::SpinLock;

pub use crate::mm::vm_space::{map_vmo_to_user, protect_user_segment};

/// x86_64 Architectural MSRs for `syscall` / `sysretq` and `swapgs`.
pub const MSR_IA32_EFER: u32 = 0xC000_0080;
pub const MSR_IA32_STAR: u32 = 0xC000_0081;
pub const MSR_IA32_LSTAR: u32 = 0xC000_0082;
pub const MSR_IA32_FMASK: u32 = 0xC000_0084;
pub const MSR_IA32_GS_BASE: u32 = 0xC000_0101;
pub const MSR_IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

const EFER_SCE: u64 = 1 << 0; // System Call Extensions enable bit
const EFER_NXE: u64 = 1 << 11; // No-Execute Enable bit (Hardware W^X enforcement)
const SYSCALL_RFLAGS_MASK: u64 = 0x0004_0300; // Clear IF (bit 9), TF (bit 8), AC (bit 18)

/// Canonical Lower-Half User Space virtual addresses used for Ring 3 execution.
pub const USER_CODE_BASE_VADDR: u64 = USER_SPACE_LOW_BASE;
pub const USER_STACK_BASE_VADDR: u64 = USER_STACK_DEFAULT_BASE;
pub const USER_STACK_PAGES: usize = 4; // 16 KiB Ring 3 stack (Parent Shell)

/// Dedicated Lower-Half User Stack base for child ELF64 processes launched via `sys_execve` (`0x0090_0000`).
pub const USER_CHILD_STACK_BASE_VADDR: u64 = 0x0000_0000_0090_0000;
pub const USER_CHILD_STACK_PAGES: usize = 4; // 16 KiB Ring 3 stack (Child Process)

const SYSCALL_KERNEL_STACK_SIZE: usize = 16 * 1024;

#[repr(C, align(16))]
struct SyscallStack([u8; SYSCALL_KERNEL_STACK_SIZE]);

static mut SYSCALL_STACK: SyscallStack = SyscallStack([0; SYSCALL_KERNEL_STACK_SIZE]);

/// Per-CPU state structure accessed via `GS` in Ring 0 after `swapgs`.
///
/// Field offsets are ABI-fixed and matched by `ostd/src/arch/x86_64/trap.s`:
/// - `0x00`: `kernel_syscall_rsp`
/// - `0x08`: `user_rsp_scratch`
/// - `0x10`: `saved_kernel_rsp`
/// - `0x18`: `user_exit_code`
/// - `0x20`: `cpu_id`
#[repr(C, align(64))]
pub struct CpuLocal {
    pub kernel_syscall_rsp: u64,
    pub user_rsp_scratch: u64,
    pub saved_kernel_rsp: u64,
    pub user_exit_code: i64,
    pub cpu_id: u64,
}

impl CpuLocal {
    pub const fn new() -> Self {
        Self {
            kernel_syscall_rsp: 0,
            user_rsp_scratch: 0,
            saved_kernel_rsp: 0,
            user_exit_code: 0,
            cpu_id: 0,
        }
    }
}

static mut BSP_CPU_LOCAL: CpuLocal = CpuLocal::new();

core::arch::global_asm!(include_str!("trap.s"));

extern "C" {
    fn __fipsos_syscall_entry();
    fn __fipsos_enter_ring3(user_rip: u64, user_rsp: u64) -> i32;
}

/// CPU register context captured by `trap.s` upon a Ring 3 `syscall` instruction.
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
    pub rbx: u64,
    pub rbp: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub user_rip: u64,
    pub user_rflags: u64,
    pub user_rsp: u64,
}

/// Control flow action returned by the high-level syscall dispatcher (`linux_abi`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallAction {
    /// Return to Ring 3 via `sysretq` with `frame.rax` set to the given return value.
    Continue(u64),
    /// Save the parent process context (e.g. Shell) and enter a child Ring 3 ELF64 binary.
    SpawnChild { entry_rip: u64, user_rsp: u64 },
    /// Terminate the current Ring 3 execution context with the specified exit status.
    ExitUser(i32),
}

/// Saved Ring 3 `TrapFrame` and kernel `saved_kernel_rsp` of a parent process waiting on `sys_execve`.
#[derive(Debug, Clone, Copy)]
struct SavedParentContext {
    trap_frame: TrapFrame,
    saved_kernel_rsp: u64,
}

static PARENT_CONTEXT_STACK: SpinLock<Vec<SavedParentContext>> = SpinLock::new(Vec::new());

/// Returns `true` if a parent Ring 3 process (e.g. Shell) is stacked waiting for a child process.
pub fn has_parent_context() -> bool {
    !PARENT_CONTEXT_STACK.lock().is_empty()
}

/// Allocates and maps the isolated Ring 3 stack for a child ELF64 process at `0x0090_0000` (`W^X` `R+W`)
/// and returns its initial 16-byte-ABI-aligned `RSP`.
pub fn prepare_child_ring3_stack() -> Result<u64, &'static str> {
    let stack_size = USER_CHILD_STACK_PAGES * PAGE_SIZE;
    map_user_segment(USER_CHILD_STACK_BASE_VADDR, stack_size, true, false)?;
    // System V AMD64 ABI expects (RSP + 8) % 16 == 0 upon entering `extern "C" fn _user_start()`.
    Ok(USER_CHILD_STACK_BASE_VADDR + (stack_size as u64) - 8)
}

/// Type signature for the safe high-level syscall handler registered by `linux_abi`.
pub type SyscallDispatcherFn = fn(&mut TrapFrame) -> SyscallAction;

static SYSCALL_DISPATCHER: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());

/// Registers the safe syscall dispatcher callback from `linux_abi`.
pub fn register_dispatcher(dispatcher: SyscallDispatcherFn) {
    SYSCALL_DISPATCHER.store(dispatcher as *mut (), Ordering::Release);
}

#[no_mangle]
extern "C" fn __ostd_syscall_dispatch(frame: &mut TrapFrame) -> u64 {
    let ptr = SYSCALL_DISPATCHER.load(Ordering::Acquire);
    if ptr.is_null() {
        crate::early_println!(
            "[ostd::syscall] Sin despachador registrado para syscall {}",
            frame.rax
        );
        frame.rax = (-38i64) as u64; // -ENOSYS
        return 0;
    }

    // SAFETY: `ptr` was stored from a valid `SyscallDispatcherFn` function pointer.
    let dispatcher: SyscallDispatcherFn = unsafe { core::mem::transmute(ptr) };
    match dispatcher(frame) {
        SyscallAction::Continue(ret_val) => {
            frame.rax = ret_val;
            0
        }
        SyscallAction::SpawnChild {
            entry_rip,
            user_rsp,
        } => {
            // SAFETY: Reading `BSP_CPU_LOCAL.saved_kernel_rsp` (`gs:[0x10]`) in Ring 0.
            let parent_kernel_rsp =
                unsafe { core::ptr::read_volatile(&raw const BSP_CPU_LOCAL.saved_kernel_rsp) };
            PARENT_CONTEXT_STACK.lock().push(SavedParentContext {
                trap_frame: *frame,
                saved_kernel_rsp: parent_kernel_rsp,
            });

            *frame = TrapFrame {
                rax: 0,
                rdi: 0,
                rsi: 0,
                rdx: 0,
                r10: 0,
                r8: 0,
                r9: 0,
                rbx: 0,
                rbp: 0,
                r12: 0,
                r13: 0,
                r14: 0,
                r15: 0,
                user_rip: entry_rip,
                user_rflags: 0x202, // IF=1 (bit 9) + reserved bit 1
                user_rsp,
            };
            0
        }
        SyscallAction::ExitUser(exit_code) => {
            if let Some(parent_ctx) = PARENT_CONTEXT_STACK.lock().pop() {
                *frame = parent_ctx.trap_frame;
                frame.rax = (exit_code as i64) as u64;
                // SAFETY: Restoring the parent process's `saved_kernel_rsp` into `BSP_CPU_LOCAL`.
                unsafe {
                    core::ptr::write_volatile(
                        &raw mut BSP_CPU_LOCAL.saved_kernel_rsp,
                        parent_ctx.saved_kernel_rsp,
                    );
                }
                0
            } else {
                // SAFETY: Updating current CPU's `CpuLocal.user_exit_code` via `gs:[0x18]`.
                unsafe {
                    core::arch::asm!(
                        "mov gs:[0x18], {}",
                        in(reg) (exit_code as i64),
                        options(nostack, preserves_flags)
                    );
                }
                1
            }
        }
    }
}

/// Configures `IA32_EFER`, `IA32_STAR`, `IA32_LSTAR`, `IA32_FMASK`, and `IA32_KERNEL_GS_BASE`
/// (`CpuLocal` + `swapgs`) for SMP-safe Ring 3 `syscall`/`sysretq`.
pub fn init() {
    // SAFETY: Writing architectural x86_64 syscall and GS base MSRs in Ring 0.
    unsafe {
        let stack_top = (&raw const SYSCALL_STACK)
            .cast::<u8>()
            .add(SYSCALL_KERNEL_STACK_SIZE) as u64;

        BSP_CPU_LOCAL.kernel_syscall_rsp = stack_top;
        BSP_CPU_LOCAL.user_rsp_scratch = 0;
        BSP_CPU_LOCAL.saved_kernel_rsp = 0;
        BSP_CPU_LOCAL.user_exit_code = 0;
        BSP_CPU_LOCAL.cpu_id = 0;

        let cpu_local_addr = (&raw const BSP_CPU_LOCAL) as u64;

        // Initialize IA32_KERNEL_GS_BASE with CpuLocal address and IA32_GS_BASE with 0,
        // then execute `swapgs` so Ring 0 runs with GS.base = &BSP_CPU_LOCAL and
        // IA32_KERNEL_GS_BASE = 0 (ready to be swapped again upon entering Ring 3).
        wrmsr(MSR_IA32_GS_BASE, 0);
        wrmsr(MSR_IA32_KERNEL_GS_BASE, cpu_local_addr);
        core::arch::asm!("swapgs", options(nomem, nostack, preserves_flags));

        // 1. Enable SCE (System Call Extensions) and NXE (No-Execute Enable) in IA32_EFER
        let efer = rdmsr(MSR_IA32_EFER);
        wrmsr(MSR_IA32_EFER, efer | EFER_SCE | EFER_NXE);

        // 2. Configure IA32_STAR:
        //    Bits 47:32 = 0x0008 (Kernel CS=0x08, SS=0x10)
        //    Bits 63:48 = 0x0010 (User SS=(0x10+8)|3=0x1B, User CS=(0x10+16)|3=0x23)
        let star: u64 = (0x0010u64 << 48) | (0x0008u64 << 32);
        wrmsr(MSR_IA32_STAR, star);

        // 3. Configure IA32_LSTAR with the address of `__fipsos_syscall_entry` in `trap.s`
        let lstar = __fipsos_syscall_entry as *const () as u64;
        wrmsr(MSR_IA32_LSTAR, lstar);

        // 4. Configure IA32_FMASK to clear IF, TF, and AC on syscall entry
        wrmsr(MSR_IA32_FMASK, SYSCALL_RFLAGS_MASK);

        crate::early_println!(
            "[ostd::arch::syscall] MSRs + swapgs CpuLocal@{:#x} + EFER.NXE (W^X) activos (STAR={:#x}, LSTAR={:#x})",
            cpu_local_addr,
            star,
            lstar
        );
    }
}

/// Safely reads bytes from any mapped Lower-Half User Space buffer (ELF, Stack, or `mmap` VMO).
pub fn read_user_bytes(user_vaddr: u64, dst: &mut [u8]) -> Result<(), &'static str> {
    vm_read_user_bytes(user_vaddr, dst)
}

/// Safely writes bytes into any writable Lower-Half User Space buffer (ELF, Stack, or `mmap` VMO).
pub fn write_user_bytes(user_vaddr: u64, src: &[u8]) -> Result<(), &'static str> {
    vm_write_user_bytes(user_vaddr, src)
}

/// Maps a Ring 3 User Stack at `USER_STACK_BASE_VADDR`, transitions the CPU to **Ring 3 (`CPL=3`)**
/// starting at `entry_rip`, and returns the exit status when the user program invokes `sys_exit`.
pub fn enter_ring3_at(entry_rip: u64) -> Result<i32, &'static str> {
    let stack_size = USER_STACK_PAGES * PAGE_SIZE;
    map_user_segment(USER_STACK_BASE_VADDR, stack_size, true, false)?;

    let user_rsp = USER_STACK_BASE_VADDR + (stack_size as u64) - 16;
    // SAFETY: `entry_rip` and `user_rsp` are mapped in Lower-Half User Space with PTE_USER.
    let exit_code = unsafe { __fipsos_enter_ring3(entry_rip, user_rsp) };
    Ok(exit_code)
}

#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    // SAFETY: Caller guarantees valid architectural MSR.
    unsafe {
        core::arch::asm!(
            "rdmsr",
            in("ecx") msr,
            out("eax") low,
            out("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }
    ((high as u64) << 32) | (low as u64)
}

#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    // SAFETY: Caller guarantees valid architectural MSR and value.
    unsafe {
        core::arch::asm!(
            "wrmsr",
            in("ecx") msr,
            in("eax") low,
            in("edx") high,
            options(nomem, nostack, preserves_flags)
        );
    }
}
