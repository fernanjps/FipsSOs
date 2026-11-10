use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, Ordering};

use crate::arch::fpu::FpuState;
use crate::mm::{
    vm_space::{
        map_user_segment, read_user_bytes as vm_read_user_bytes,
        write_user_bytes as vm_write_user_bytes, USER_SPACE_LOW_BASE, USER_STACK_DEFAULT_BASE,
    },
    PAGE_SIZE,
};
use crate::sync::SpinLock;

pub use crate::mm::vm_space::{map_vmo_to_user, protect_user_segment};

/// x86_64 Architectural MSRs for `syscall` / `sysretq`, `swapgs`, and Thread-Local Storage (TLS).
pub const MSR_IA32_EFER: u32 = 0xC000_0080;
pub const MSR_IA32_STAR: u32 = 0xC000_0081;
pub const MSR_IA32_LSTAR: u32 = 0xC000_0082;
pub const MSR_IA32_FMASK: u32 = 0xC000_0084;
pub const MSR_IA32_FS_BASE: u32 = 0xC000_0100;
pub const MSR_IA32_GS_BASE: u32 = 0xC000_0101;
pub const MSR_IA32_KERNEL_GS_BASE: u32 = 0xC000_0102;

const EFER_SCE: u64 = 1 << 0; // System Call Extensions enable bit
const EFER_NXE: u64 = 1 << 11; // No-Execute Enable bit (Hardware W^X enforcement)
const SYSCALL_RFLAGS_MASK: u64 = 0x0004_0300; // Clear IF (bit 9), TF (bit 8), AC (bit 18)

/// Canonical Lower-Half User Space virtual addresses used for Ring 3 execution.
pub const USER_CODE_BASE_VADDR: u64 = USER_SPACE_LOW_BASE;
pub const USER_STACK_BASE_VADDR: u64 = USER_STACK_DEFAULT_BASE;
pub const USER_STACK_PAGES: usize = 4; // 16 KiB Ring 3 stack (0x0080_0000..0x0080_4000)

/// Unified Lower-Half User Stack base for isolated child ELF64 `CR3` processes (`0x0080_0000`).
pub const USER_CHILD_STACK_BASE_VADDR: u64 = USER_STACK_DEFAULT_BASE;
pub const USER_CHILD_STACK_PAGES: usize = 4; // 16 KiB Ring 3 stack (Child Process)

const SYSCALL_KERNEL_STACK_SIZE: usize = 64 * 1024;

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
        Self { kernel_syscall_rsp: 0, user_rsp_scratch: 0, saved_kernel_rsp: 0, user_exit_code: 0, cpu_id: 0 }
    }
}

static mut BSP_CPU_LOCAL: CpuLocal = CpuLocal::new();

core::arch::global_asm!(include_str!("trap.s"));

extern "C" {
    fn __fipsos_syscall_entry();
    fn __fipsos_enter_ring3(user_rip: u64, user_rsp: u64) -> i32;
    fn __fipsos_resume_from_fault(tf: *const TrapFrame) -> !;
    fn __fipsos_usermode_exit() -> !;
}

static USER_FAULTS_RECOVERED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Returns the total number of Ring 3 user faults (`#PF` / `#GP`) safely recovered by the kernel.
pub fn user_faults_recovered() -> u64 {
    USER_FAULTS_RECOVERED.load(core::sync::atomic::Ordering::Relaxed)
}

/// Triggers a test Ring 3 user fault recovery verification log message.
pub fn trigger_ring3_fault_test() {
    crate::early_println!("[ostd::syscall] Disparando test de recuperacion de fallo en Anillo 3...");
}

/// Safely recovers from a Ring 3 user-space fault (`#PF` or `#GP`) without halting the kernel.
///
/// If a parent process (e.g. Shell or Desktop UI) is stacked waiting on `sys_execve`, restores the parent's
/// `PML4` in hardware `CR3`, restores its `FpuState` (`FXRSTOR64`), sets the child return code to
/// `signal_exit_code` (e.g. `-11` for `SIGSEGV` or `-13` for `#GP`), and returns to the parent in Ring 3.
/// If no parent context exists, returns control to the master kernel loop via `__fipsos_usermode_exit`.
pub fn recover_from_user_fault(fault_rip: u64, fault_addr: u64, signal_exit_code: i32) -> ! {
    let count = USER_FAULTS_RECOVERED.fetch_add(1, core::sync::atomic::Ordering::Relaxed) + 1;

    crate::early_println!(
        "[USER FAULT RECOVERY] Proceso Anillo 3 abortado con señal {} en RIP={:#x}, CR2={:#x} -> Kernel intacto (Total recuperados={})",
        signal_exit_code,
        fault_rip,
        fault_addr,
        count
    );
    crate::fb_println!(
        "[USER FAULT RECOVERY] Anillo 3 abortado (Sig={}, RIP={:#x}, CR2={:#x}) -> Kernel intacto",
        signal_exit_code,
        fault_rip,
        fault_addr
    );

    let faulted_cr3 = crate::mm::active_cr3();
    let parent_opt = PARENT_CONTEXT_STACK.lock().pop();
    if let Some(parent_ctx) = parent_opt {
        // 1. Switch back to parent's isolated PML4 root
        crate::mm::switch_cr3(parent_ctx.parent_cr3);

        // 2. Reclaim memory of the faulted child process
        if faulted_cr3 != parent_ctx.parent_cr3 && faulted_cr3 != crate::mm::master_kernel_cr3() {
            let _ = crate::mm::destroy_process_vm_space(faulted_cr3);
        }

        crate::scheduler::set_active_slot(parent_ctx.parent_slot);

        // 3. Restore parent's FPU/SSE2 context (FXRSTOR64)
        parent_ctx.fpu_state.restore();

        // 4. Restore parent's MSR_IA32_FS_BASE (Thread-Local Storage)
        set_user_fs_base(parent_ctx.user_fs_base);

        // 5. Restore parent's saved_kernel_rsp in CpuLocal
        unsafe {
            core::ptr::write_volatile(
                &raw mut BSP_CPU_LOCAL.saved_kernel_rsp,
                parent_ctx.saved_kernel_rsp,
            );
        }

        // 6. Return to parent process in Ring 3 via iretq with RAX = signal_exit_code
        let mut tf = parent_ctx.trap_frame;
        tf.rax = (signal_exit_code as i64) as u64;

        unsafe {
            __fipsos_resume_from_fault(&tf);
        }
    } else {
        // Top-level Ring 3 process: restore clean Master Kernel PML4
        crate::mm::switch_cr3(crate::mm::master_kernel_cr3());

        if faulted_cr3 != crate::mm::master_kernel_cr3() {
            let _ = crate::mm::destroy_process_vm_space(faulted_cr3);
        }

        unsafe {
            core::arch::asm!(
                "mov gs:[0x18], {}",
                in(reg) (signal_exit_code as i64),
                options(nostack, preserves_flags)
            );
            __fipsos_usermode_exit();
        }
    }
}

/// CPU register context captured by `trap.s` upon a Ring 3 `syscall` instruction.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct TrapFrame {
    pub rax: u64, pub rdi: u64, pub rsi: u64, pub rdx: u64,
    pub r10: u64, pub r8: u64, pub r9: u64, pub rbx: u64,
    pub rbp: u64, pub r12: u64, pub r13: u64, pub r14: u64,
    pub r15: u64, pub user_rip: u64, pub user_rflags: u64, pub user_rsp: u64,
}

impl TrapFrame {
    pub const fn empty(user_rip: u64, user_rsp: u64) -> Self {
        Self {
            rax: 0, rdi: 0, rsi: 0, rdx: 0, r10: 0, r8: 0, r9: 0,
            rbx: 0, rbp: 0, r12: 0, r13: 0, r14: 0, r15: 0,
            user_rip, user_rflags: 0x202, user_rsp,
        }
    }
}

/// Control flow action returned by the high-level syscall dispatcher (`linux_abi`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallAction {
    /// Return to Ring 3 via `sysretq` with `frame.rax` set to the given return value.
    Continue(u64),
    /// Save parent process context and enter a child Ring 3 ELF64 binary.
    SpawnChild { entry_rip: u64, user_rsp: u64, parent_cr3: u64 },
    /// Save parent process context, switch CR3 to child_cr3, set frame.rax = 0, and enter child.
    ForkChild { child_pid: u64, child_cr3: u64, parent_cr3: u64, child_slot: u64, child_rsp: u64 },
    /// Terminate current Ring 3 execution context with the specified exit status.
    ExitUser(i32),
}

/// Saved Ring 3 context for nested execution (`Shell -> Desktop UI -> App`).
struct SavedParentContext {
    fpu_state: FpuState, trap_frame: TrapFrame, saved_kernel_rsp: u64,
    parent_cr3: u64, user_fs_base: u64, is_fork: bool, parent_slot: u64,
}

static PARENT_CONTEXT_STACK: SpinLock<Vec<Box<SavedParentContext>>> = SpinLock::new(Vec::new());

/// Returns `true` if a parent Ring 3 process (e.g. Shell or Desktop UI) is stacked waiting for a child.
pub fn has_parent_context() -> bool {
    !PARENT_CONTEXT_STACK.lock().is_empty()
}

/// Returns `true` if the top stacked parent was created by `sys_fork` / `sys_clone`.
pub fn is_parent_fork() -> bool {
    PARENT_CONTEXT_STACK
        .lock()
        .last()
        .map(|ctx| ctx.is_fork)
        .unwrap_or(false)
}

/// Returns the `parent_cr3` physical `PML4` address at the top of `PARENT_CONTEXT_STACK`, if any.
pub fn peek_parent_cr3() -> Option<u64> {
    PARENT_CONTEXT_STACK.lock().last().map(|ctx| ctx.parent_cr3)
}

/// Allocates and maps the canonical 16 KiB Ring 3 stack (`0x0080_0000..0x0080_4000`, `W^X` `R+W+NX`)
/// inside the child process's isolated `CR3` (`PML4`) page table.
pub fn prepare_child_ring3_stack() -> Result<u64, &'static str> {
    let stack_base = USER_CHILD_STACK_BASE_VADDR;
    let stack_size = USER_CHILD_STACK_PAGES * PAGE_SIZE;
    map_user_segment(stack_base, stack_size, true, false)?;
    // System V AMD64 ABI expects (RSP + 8) % 16 == 0 upon entering `extern "C" fn _user_start()`.
    Ok(stack_base + (stack_size as u64) - 8)
}

/// Sets up the complete System V AMD64 ABI initial stack frame (`argc`, `argv`, `envp`, `auxv`)
/// in Lower-Half User Space with custom command-line arguments for Linux libc / musl / gcc -static binaries.
pub fn setup_sysv_abi_user_stack_args(
    args: &[&str],
    entry_rip: u64,
    phdr_vaddr: u64,
    phent: u16,
    phnum: u16,
) -> Result<u64, &'static str> {
    let stack_base = USER_CHILD_STACK_BASE_VADDR;
    let stack_size = USER_CHILD_STACK_PAGES * PAGE_SIZE;
    map_user_segment(stack_base, stack_size, true, false)?;

    // 1. High stack addresses for string data:
    // Place 16 random bytes at 0x0080_3FE0 for AT_RANDOM
    let random_vaddr = stack_base + (stack_size as u64) - 32; // 0x0080_3FE0
    let random_bytes: [u8; 16] = [
        0x5A, 0x3F, 0x91, 0x2C, 0x88, 0x14, 0x7E, 0xB0, 0x19, 0x5C, 0xA4, 0x63, 0xD2, 0x08, 0xFE, 0x47,
    ];
    write_user_bytes(random_vaddr, &random_bytes)?;

    let mut str_cursor = random_vaddr;
    let mut arg_vaddrs = alloc::vec::Vec::new();
    for arg in args {
        let bytes = arg.as_bytes();
        let copy_len = bytes.len();
        str_cursor = (str_cursor.saturating_sub((copy_len + 1) as u64)) & !7;
        write_user_bytes(str_cursor, &bytes[..copy_len])?;
        write_user_bytes(str_cursor + (copy_len as u64), &[0u8])?;
        arg_vaddrs.push(str_cursor);
    }

    let argc = arg_vaddrs.len();
    let total_words = 1 + argc + 1 + 1 + 32;
    let table_bytes = total_words * 8;
    let rsp_vaddr = (str_cursor.saturating_sub((table_bytes + 16) as u64)) & !0xF;

    let mut table = alloc::vec![0u64; total_words];
    table[0] = argc as u64;
    for (i, &vaddr) in arg_vaddrs.iter().enumerate() {
        table[1 + i] = vaddr;
    }
    table[1 + argc] = 0; // argv[argc] = NULL
    table[2 + argc] = 0; // envp[0] = NULL

    let mut idx = 3 + argc;
    let auxv: [(u64, u64); 14] = [
        (3, phdr_vaddr), (4, phent as u64), (5, phnum as u64), (6, 4096),
        (7, 0), (8, 0), (9, entry_rip), (11, 0), (12, 0), (13, 0), (14, 0),
        (23, 0), (25, random_vaddr), (0, 0),
    ];
    for (t, v) in auxv {
        table[idx] = t; table[idx + 1] = v; idx += 2;
    }

    let mut table_raw = alloc::vec![0u8; total_words * 8];
    for (i, word) in table.iter().enumerate() {
        table_raw[i * 8..(i + 1) * 8].copy_from_slice(&word.to_le_bytes());
    }
    write_user_bytes(rsp_vaddr, &table_raw)?;

    Ok(rsp_vaddr)
}

/// Sets up the complete System V AMD64 ABI initial stack frame (`argc`, `argv`, `envp`, `auxv`)
/// in Lower-Half User Space for single-argument Linux libc / musl / gcc -static binaries.
pub fn setup_sysv_abi_user_stack(
    program_path: &str,
    entry_rip: u64,
    phdr_vaddr: u64,
    phent: u16,
    phnum: u16,
) -> Result<u64, &'static str> {
    setup_sysv_abi_user_stack_args(&[program_path], entry_rip, phdr_vaddr, phent, phnum)
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

            // Deliver pending signals to userland handler if registered
            let (active_pid, _) = crate::scheduler::active_pid_and_tid();
            if let Some((sig, handler)) = crate::scheduler::pop_pending_signal(active_pid) {
                if handler > 1 {
                    let new_rsp = (frame.user_rsp.saturating_sub(8)) & !0x7;
                    if write_user_bytes(new_rsp, &frame.user_rip.to_le_bytes()).is_ok() {
                        frame.user_rsp = new_rsp;
                        frame.user_rip = handler;
                        frame.rdi = sig as u64;
                    }
                }
            }
            0
        }
        SyscallAction::SpawnChild {
            entry_rip,
            user_rsp,
            parent_cr3,
        } => {
            // SAFETY: Reading `BSP_CPU_LOCAL.saved_kernel_rsp` (`gs:[0x10]`) in Ring 0.
            let parent_kernel_rsp =
                unsafe { core::ptr::read_volatile(&raw const BSP_CPU_LOCAL.saved_kernel_rsp) };
            let parent_fs_base = get_user_fs_base();
            let parent_slot = crate::scheduler::active_slot();
            let mut parent_ctx = Box::new(SavedParentContext {
                fpu_state: FpuState::new(), trap_frame: *frame, saved_kernel_rsp: parent_kernel_rsp,
                parent_cr3, user_fs_base: parent_fs_base, is_fork: false, parent_slot,
            });
            parent_ctx.fpu_state.save();
            PARENT_CONTEXT_STACK.lock().push(parent_ctx);

            // Clean child MSR_IA32_FS_BASE for fresh TLS initialization
            set_user_fs_base(0);

            *frame = TrapFrame::empty(entry_rip, user_rsp);
            0
        }
        SyscallAction::ForkChild {
            child_pid, child_cr3, parent_cr3, child_slot, child_rsp,
        } => {
            // SAFETY: Reading `BSP_CPU_LOCAL.saved_kernel_rsp` (`gs:[0x10]`) in Ring 0.
            let parent_kernel_rsp =
                unsafe { core::ptr::read_volatile(&raw const BSP_CPU_LOCAL.saved_kernel_rsp) };
            let parent_fs_base = get_user_fs_base();
            let parent_slot = crate::scheduler::active_slot();

            // Set parent's return RAX to child_pid!
            let mut parent_frame = *frame;
            parent_frame.rax = child_pid;

            let mut parent_ctx = Box::new(SavedParentContext {
                fpu_state: FpuState::new(), trap_frame: parent_frame, saved_kernel_rsp: parent_kernel_rsp,
                parent_cr3, user_fs_base: parent_fs_base, is_fork: true, parent_slot,
            });
            parent_ctx.fpu_state.save();
            PARENT_CONTEXT_STACK.lock().push(parent_ctx);

            // Record child slot switch in scheduler
            crate::scheduler::set_active_slot(child_slot);

            // Switch hardware CR3 to child's isolated PML4 root
            crate::mm::switch_cr3(child_cr3);

            // In child, RAX is 0 (child process)
            frame.rax = 0;
            if child_rsp != 0 {
                frame.user_rsp = child_rsp;
            }
            0
        }
        SyscallAction::ExitUser(exit_code) => {
            let exiting_cr3 = crate::mm::active_cr3();
            let (exited_pid, _) = crate::scheduler::active_pid_and_tid();
            crate::scheduler::mark_process_exit(exited_pid, exit_code);

            if let Some(parent_ctx) = PARENT_CONTEXT_STACK.lock().pop() {
                // Restore the parent's isolated `PML4` root in hardware `CR3` (flushes Lower-Half TLB)
                crate::mm::switch_cr3(parent_ctx.parent_cr3);

                // Reclaim the exiting child's memory if it had a dedicated CR3
                if exiting_cr3 != parent_ctx.parent_cr3 && exiting_cr3 != crate::mm::master_kernel_cr3() {
                    let _ = crate::mm::destroy_process_vm_space(exiting_cr3);
                }

                crate::scheduler::set_active_slot(parent_ctx.parent_slot);

                *frame = parent_ctx.trap_frame;
                if !parent_ctx.is_fork {
                    frame.rax = (exit_code as i64) as u64;
                }
                // If is_fork is true, frame.rax was already set to child_pid!

                parent_ctx.fpu_state.restore();
                set_user_fs_base(parent_ctx.user_fs_base);
                // SAFETY: Restoring the parent process's `saved_kernel_rsp` into `BSP_CPU_LOCAL`.
                unsafe {
                    core::ptr::write_volatile(
                        &raw mut BSP_CPU_LOCAL.saved_kernel_rsp,
                        parent_ctx.saved_kernel_rsp,
                    );
                }
                0
            } else {
                // Top-level user process exited: restore the clean Master Kernel `PML4`
                crate::mm::switch_cr3(crate::mm::master_kernel_cr3());

                if exiting_cr3 != crate::mm::master_kernel_cr3() {
                    let _ = crate::mm::destroy_process_vm_space(exiting_cr3);
                }

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
        //    Bits 63:48 = 0x0010 | 3 = 0x0013 (User SS=(0x10+8)|3=0x1B, User CS=(0x10+16)|3=0x23)
        //    Crucial: setting bits 49:48 to RPL=3 ensures AMD CPUs calculate SS with RPL=3 on sysretq!
        let star: u64 = ((0x0010u64 | 3) << 48) | (0x0008u64 << 32);
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

/// Transitions the CPU to **Ring 3 (`CPL=3`)** with an already configured System V ABI stack (`user_rsp`)
/// starting at `entry_rip`, and returns the exit status when the user program invokes `sys_exit`.
pub fn enter_ring3_sysv(entry_rip: u64, user_rsp: u64) -> Result<i32, &'static str> {
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

/// Reads the current Ring 3 `MSR_IA32_FS_BASE` (Thread-Local Storage base).
pub fn get_user_fs_base() -> u64 {
    // SAFETY: Reading architectural MSR_IA32_FS_BASE.
    unsafe { rdmsr(MSR_IA32_FS_BASE) }
}

/// Sets the Ring 3 `MSR_IA32_FS_BASE` (Thread-Local Storage base).
pub fn set_user_fs_base(addr: u64) {
    // SAFETY: Writing architectural MSR_IA32_FS_BASE.
    unsafe { wrmsr(MSR_IA32_FS_BASE, addr) };
}
