#![forbid(unsafe_code)]

use kernel_objects::SchedulerRegistry;
use ostd::arch::syscall::{
    read_user_bytes, setup_sysv_abi_user_stack_args, write_user_bytes, SyscallAction, TrapFrame,
};
use ostd::mm::{active_cr3, reset_user_brk, switch_cr3};

use crate::elf::Elf64Loader;
use crate::vfs::clone_process_fds;
use super::{resolve_path, set_active_program_name, GLOBAL_VFS};

/// Linux `sys_fork` (syscall 57): duplicates the calling process by creating a new PML4 with
/// complete Lower-Half memory isolation, registering the child in the scheduler, and returning
/// `child_pid` to the parent and `0` to the child.
pub fn sys_fork(_frame: &TrapFrame) -> SyscallAction {
    if let Err(e) = super::sandbox::check_spawn_sandbox() {
        return SyscallAction::Continue(e as u64);
    }
    super::fs::reclaim_dead_process_fds();
    let parent_cr3 = active_cr3();
    let parent_pid = ostd::scheduler::active_pid();

    let child_cr3 = match ostd::mm::clone_user_vm_space(parent_cr3) {
        Ok(cr3) => cr3,
        Err(e) => {
            ostd::early_println!("[linux_abi::fork] Error al clonar espacio VM: {}", e);
            return SyscallAction::Continue((-12i64) as u64); // -ENOMEM
        }
    };

    let (child_slot, child_pid) = match ostd::scheduler::allocate_child_slot(parent_pid, child_cr3) {
        Some(res) => res,
        None => {
            let _ = ostd::mm::destroy_process_vm_space(child_cr3);
            ostd::early_println!("[linux_abi::fork] Tabla de procesos llena (32 procesos)");
            return SyscallAction::Continue((-11i64) as u64); // -EAGAIN
        }
    };

    ostd::early_println!(
        "[linux_abi::fork] sys_fork() -> Nuevo proceso hijo PID={} (Slot={}) | CR3: {:#x} -> {:#x} | PML4 duplicado con aislamiento W^X",
        child_pid,
        child_slot,
        parent_cr3,
        child_cr3
    );

    let parent_slot = ostd::scheduler::active_slot();
    clone_process_fds(parent_slot, child_slot);
    crate::container::on_process_fork(crate::vfs::get_process_container_id(child_slot));

    SyscallAction::ForkChild {
        child_pid,
        child_cr3,
        parent_cr3,
        child_slot,
        child_rsp: 0,
    }
}


/// Linux `sys_clone` (syscall 56): clone process or thread. When creating a process (without CLONE_VM),
/// performs full PML4 duplication with Lower-Half isolation.
pub fn sys_clone(flags: u64, newsp: u64, _frame: &TrapFrame) -> SyscallAction {
    if let Err(e) = super::sandbox::check_spawn_sandbox() {
        return SyscallAction::Continue(e as u64);
    }
    super::fs::reclaim_dead_process_fds();
    let parent_cr3 = active_cr3();
    let parent_pid = ostd::scheduler::active_pid();

    let child_cr3 = match ostd::mm::clone_user_vm_space(parent_cr3) {
        Ok(cr3) => cr3,
        Err(e) => {
            ostd::early_println!("[linux_abi::clone] Error al clonar espacio VM: {}", e);
            return SyscallAction::Continue((-12i64) as u64); // -ENOMEM
        }
    };

    let (child_slot, child_pid) = match ostd::scheduler::allocate_child_slot(parent_pid, child_cr3) {
        Some(res) => res,
        None => {
            let _ = ostd::mm::destroy_process_vm_space(child_cr3);
            return SyscallAction::Continue((-11i64) as u64); // -EAGAIN
        }
    };

    ostd::early_println!(
        "[linux_abi::clone] sys_clone(flags={:#x}, newsp={:#x}) -> PID={} | CR3: {:#x} -> {:#x}",
        flags,
        newsp,
        child_pid,
        parent_cr3,
        child_cr3
    );

    let parent_slot = ostd::scheduler::active_slot();
    clone_process_fds(parent_slot, child_slot);
    crate::container::on_process_fork(crate::vfs::get_process_container_id(child_slot));

    SyscallAction::ForkChild {
        child_pid,
        child_cr3,
        parent_cr3,
        child_slot,
        child_rsp: newsp,
    }
}


/// Linux `sys_wait4` (syscall 61): waits for child process termination, collects its exit status,
/// and recycles its process slot in the scheduler table.
pub fn sys_wait4(target_pid: i64, status_addr: u64, options: u64, _rusage: u64) -> i64 {
    let parent_pid = ostd::scheduler::active_pid();

    let mut attempts = 0;
    loop {
        super::fs::reclaim_dead_process_fds();
        match ostd::scheduler::collect_zombie_child(parent_pid, target_pid) {
            Ok((collected_pid, exit_status, collected_slot, _residual_cr3)) => {
                if collected_slot >= 2 {
                    super::fs::finish_orphan_nodes(crate::vfs::cleanup_process_fds(collected_slot));
                }
                super::fs::reclaim_dead_process_fds();
                if status_addr != 0 {
                    let _ = write_user_bytes(status_addr, &exit_status.to_le_bytes());
                }
                ostd::early_println!(
                    "[linux_abi::wait4] sys_wait4(pid={}) -> Recolectado hijo PID={} con status={:#x} (exit={})",
                    target_pid,
                    collected_pid,
                    exit_status,
                    (exit_status >> 8) & 0xff
                );
                return collected_pid as i64;
            }
            Err(0) => {
                // Child is still running
                if (options & 1) != 0 {
                    // WNOHANG
                    return 0;
                }
                attempts += 1;
                if attempts > 1000 {
                    return -10; // -ECHILD timeout fallback
                }
                let _ = ostd::scheduler::on_sys_sched_yield(0, 0);
            }
            Err(errno) => {
                return errno as i64;
            }
        }
    }
}


/// Linux `sys_sched_yield` (syscall 24): voluntarily yields the active thread's CPU quantum
/// to the preemptive Round-Robin scheduler and `simd-worker`.
pub fn sys_sched_yield(user_rip: u64, user_rsp: u64) -> i64 {
    let (tid, switches) = ostd::scheduler::on_sys_sched_yield(user_rip, user_rsp);
    if switches <= 4 {
        ostd::early_println!(
            "[Scheduler::yield] sys_sched_yield() en TID=koid:{} (RIP={:#x}) -> Quantum cedido a simd-worker (Switches={})",
            tid,
            user_rip,
            switches
        );
    }
    0
}


/// Linux `sys_getpid` (syscall 39): returns the `Koid` / `PID` of the calling Ring 3 `Process`.
pub fn sys_getpid(user_rip: u64) -> i64 {
    let (pid, tid) = ostd::scheduler::active_pid_and_tid();
    ostd::early_println!(
        "[linux_abi::syscall] sys_getpid() -> PID=koid:{} (TID=koid:{}, RIP={:#x})",
        pid,
        tid,
        user_rip
    );
    pid as i64
}


/// Linux `sys_sched_getaffinity` (syscall 204): writes a CPU affinity bitmask for the process.
pub fn sys_sched_getaffinity(_pid: i64, cpusetsize: usize, mask_ptr: u64) -> isize {
    if cpusetsize < 8 {
        return -22; // -EINVAL
    }
    let num_cpus = ostd::scheduler::online_cpu_count().min(64);
    let mask: u64 = if num_cpus >= 64 {
        !0u64
    } else {
        (1u64 << num_cpus) - 1
    };
    if write_user_bytes(mask_ptr, &mask.to_le_bytes()).is_err() {
        return -14; // -EFAULT
    }
    8
}


/// Linux `sys_execve` (syscall 59): loads an ELF64 executable from the `TarFs` `Vmo` in `CapabilityVfs`,
/// enforces strict `W^X` on its `PT_LOAD` segments, transitions the child `Process` & `Thread` in
/// `SchedulerRegistry` to `Running`, and enters Ring 3 while stacking the parent process's `TrapFrame`.
pub fn sys_execve(user_path_vaddr: u64, argv_vaddr: u64, _envp_vaddr: u64) -> SyscallAction {
    if let Err(e) = super::sandbox::check_spawn_sandbox() {
        return SyscallAction::Continue(e as u64);
    }
    let path_owned = match super::fs_dir::read_user_cstr_path(user_path_vaddr) {
        Ok(p) => p,
        Err(e) => return SyscallAction::Continue(e as u64),
    };
    let path_str = path_owned.as_str();
    if path_str.is_empty() {
        return SyscallAction::Continue((-2i64) as u64); // -ENOENT
    }

    let resolved = resolve_path(path_str);
    let mut elf_bytes = GLOBAL_VFS.lock().read_all_by_path(&resolved);

    // Fallback for extensionless absolute path (e.g. /bin/sample_tool -> /bin/sample_tool.elf)
    if elf_bytes.is_err() && path_str.starts_with('/') && !resolved.ends_with(".elf") {
        let with_elf = alloc::format!("{}.elf", resolved);
        if let Ok(bytes) = GLOBAL_VFS.lock().read_all_by_path(&with_elf) {
            elf_bytes = Ok(bytes);
        }
    }

    // Sequential executable resolution in /bin and /disk/bin when relative
    if elf_bytes.is_err() && !path_str.starts_with('/') {
        let candidates = [
            resolve_path(&alloc::format!("/bin/{}", path_str)),
            resolve_path(&alloc::format!("/disk/bin/{}", path_str)),
            resolve_path(&alloc::format!("/bin/{}.elf", path_str)),
            resolve_path(&alloc::format!("/disk/bin/{}.elf", path_str)),
        ];
        let vfs = GLOBAL_VFS.lock();
        for cand in &candidates {
            if let Ok(bytes) = vfs.read_all_by_path(cand) {
                elf_bytes = Ok(bytes);
                break;
            }
        }
    }

    let elf_bytes = match elf_bytes {
        Ok(bytes) => bytes,
        Err(errno) => return SyscallAction::Continue(errno as u64),
    };

    // Extract command-line argument strings from parent user space
    let mut args: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    if argv_vaddr != 0 {
        for i in 0..16 {
            let ptr_addr = argv_vaddr + (i * 8);
            let mut ptr_bytes = [0u8; 8];
            if read_user_bytes(ptr_addr, &mut ptr_bytes).is_err() {
                break;
            }
            let str_vaddr = u64::from_le_bytes(ptr_bytes);
            if str_vaddr == 0 {
                break;
            }
            let mut str_buf = [0u8; 256];
            let mut str_len = 0usize;
            while str_len < str_buf.len() {
                let mut b = [0u8; 1];
                if read_user_bytes(str_vaddr + (str_len as u64), &mut b).is_err() || b[0] == 0 {
                    break;
                }
                str_buf[str_len] = b[0];
                str_len += 1;
            }
            if let Ok(s) = core::str::from_utf8(&str_buf[..str_len]) {
                args.push(alloc::string::String::from(s));
            }
        }
    }
    if args.is_empty() {
        args.push(resolved.clone());
    }
    let arg_refs: alloc::vec::Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    // 1. Validate ELF header BEFORE touching CR3, allocating memory, or altering active program name
    if let Err(err) = Elf64Loader::validate_header(&elf_bytes) {
        ostd::early_println!(
            "[linux_abi::execve] Cabecera ELF64 invalida para \"{}\": {:?}",
            resolved,
            err
        );
        return SyscallAction::Continue((-8i64) as u64); // -ENOEXEC
    }

    // 2. Reclaim dead children and save the caller's active PML4 for atomic rollback on failure
    super::fs::reclaim_dead_process_fds();
    let parent_cr3 = active_cr3();

    let child_cr3 = match SchedulerRegistry::prepare_child_cr3(&resolved) {
        Ok(cr3) => cr3,
        Err(_) => return SyscallAction::Continue((-12i64) as u64), // -ENOMEM
    };
    switch_cr3(child_cr3);
    reset_user_brk();

    let loaded_elf = match Elf64Loader::load(&elf_bytes, 0) {
        Ok(loaded) => loaded,
        Err(err) => {
            switch_cr3(parent_cr3);
            let _ = ostd::mm::destroy_process_vm_space(child_cr3);
            SchedulerRegistry::abort_child_cr3(&resolved, child_cr3);
            ostd::early_println!(
                "[linux_abi::execve] Error al cargar ELF64 \"{}\": {:?}",
                resolved,
                err
            );
            return SyscallAction::Continue((-8i64) as u64); // -ENOEXEC
        }
    };

    let child_rsp = match setup_sysv_abi_user_stack_args(
        &arg_refs,
        loaded_elf.entry_point,
        loaded_elf.phdr_vaddr,
        loaded_elf.phentsize,
        loaded_elf.phnum,
    ) {
        Ok(rsp) => rsp,
        Err(_) => {
            switch_cr3(parent_cr3);
            let _ = ostd::mm::destroy_process_vm_space(child_cr3);
            SchedulerRegistry::abort_child_cr3(&resolved, child_cr3);
            return SyscallAction::Continue((-12i64) as u64); // -ENOMEM
        }
    };

    // Inherit the caller's FD table, CWD, and capabilities into the exec'd slot.
    let pre_exec_slot = ostd::scheduler::active_slot();
    let (child_pid, child_tid) =
        SchedulerRegistry::begin_child_exec(&resolved, loaded_elf.entry_point, child_rsp);
    let exec_slot = ostd::scheduler::active_slot();
    if exec_slot != pre_exec_slot {
        clone_process_fds(pre_exec_slot, exec_slot);
        crate::vfs::set_process_container_id(pre_exec_slot, 0);
        ostd::scheduler::inherit_slot_capabilities(pre_exec_slot, exec_slot);
    }
    set_active_program_name(resolved.clone());
    crate::container::refresh_current_container_pages();

    ostd::early_println!(
        "[linux_abi::execve] sys_execve(\"{}\", {} B) -> Proceso {} / Thread {} | CR3: {:#x} -> {:#x} | Entry={:#x} RSP={:#x} (Seg={})",
        resolved,
        elf_bytes.len(),
        child_pid,
        child_tid,
        parent_cr3,
        child_cr3,
        loaded_elf.entry_point,
        child_rsp,
        loaded_elf.segments_loaded
    );

    SyscallAction::SpawnChild {
        entry_rip: loaded_elf.entry_point,
        user_rsp: child_rsp,
        parent_cr3,
    }
}


/// Linux `sys_reboot` (169) implementation supporting ACPI S5 shutdown and hardware reset.
pub fn sys_reboot(magic1: u64, magic2: u64, cmd: u64, _arg: u64) -> i64 {
    ostd::early_println!(
        "[linux_abi::syscall] sys_reboot(magic1={:#x}, magic2={:#x}, cmd={:#x})",
        magic1,
        magic2,
        cmd
    );
    ostd::fb_println!(
        "[linux_abi::syscall] sys_reboot(cmd={:#x})",
        cmd
    );

    if cmd == 1 || cmd == 0x0123_4567 || cmd == 0xCDEF_0123 {
        ostd::arch::x86_64::rtc_power::system_reboot();
    } else {
        ostd::arch::x86_64::rtc_power::system_shutdown();
    }
}


/// Linux `sys_rt_sigaction` (syscall 13): sets or inspects signal disposition.
pub fn sys_rt_sigaction(signum: u64, act_ptr: u64, oldact_ptr: u64, _sigsetsize: u64) -> i64 {
    if signum == 0 || signum >= 32 || signum == 9 || signum == 19 {
        return -22; // -EINVAL
    }
    let (active_pid, _) = ostd::scheduler::active_pid_and_tid();
    if oldact_ptr != 0 {
        let old_handler = ostd::scheduler::get_sigaction(active_pid, signum as usize);
        let mut oldact = [0u8; 32];
        oldact[0..8].copy_from_slice(&old_handler.to_le_bytes());
        if write_user_bytes(oldact_ptr, &oldact).is_err() {
            return -14; // -EFAULT
        }
    }
    if act_ptr != 0 {
        let mut act = [0u8; 8];
        if read_user_bytes(act_ptr, &mut act).is_err() {
            return -14; // -EFAULT
        }
        let new_handler = u64::from_le_bytes(act);
        let _ = ostd::scheduler::set_sigaction(active_pid, signum as usize, new_handler);
    }
    0
}


/// Linux `sys_rt_sigprocmask` (syscall 14): sets or inspects the signal mask.
pub fn sys_rt_sigprocmask(how: i32, set_ptr: u64, oldset_ptr: u64, _sigsetsize: u64) -> i64 {
    let (active_pid, _) = ostd::scheduler::active_pid_and_tid();
    if oldset_ptr != 0 {
        let old_mask = ostd::scheduler::get_sigprocmask(active_pid);
        if write_user_bytes(oldset_ptr, &old_mask.to_le_bytes()).is_err() {
            return -14;
        }
    }
    if set_ptr != 0 {
        let mut mask_bytes = [0u8; 8];
        if read_user_bytes(set_ptr, &mut mask_bytes).is_err() {
            return -14;
        }
        let set = u64::from_le_bytes(mask_bytes);
        let _ = ostd::scheduler::set_sigprocmask(active_pid, how, set);
    }
    0
}


/// Linux `sys_kill` (62) implementation for signaling or terminating a process/daemon.
pub fn sys_kill(pid: u64, sig: u64) -> i64 {
    ostd::early_println!("[linux_abi::syscall] sys_kill(pid={}, sig={})", pid, sig);

    if sig >= 32 {
        return -22; // -EINVAL
    }
    let res = match ostd::scheduler::kill_task(pid, sig) {
        Ok(_) => 0,
        Err(err) => err,
    };
    super::fs::reclaim_dead_process_fds();
    res
}

