#![forbid(unsafe_code)]

//! Ring-3 Per-Process Capability Sandbox (`SYS_FIPS_SANDBOX = 450`).

use kernel_objects::{
    CAP_ALL, CAP_FS_READ, CAP_FS_WRITE, CAP_NET, CAP_RAW_FB, CAP_SPAWN, SANDBOX_AUTO_REAP,
    SANDBOX_RESTRICT_FAT0,
};

/// Dedicated FipSOs syscall number for irreversible capability narrowing.
pub const SYS_FIPS_SANDBOX: u64 = 450;

/// Linux `sys_fips_sandbox(mask, flags)` (syscall 450):
/// Irreversibly narrows the active process slot's capability mask and optional
/// write-path restriction (`SANDBOX_RESTRICT_FAT0`) or background `SANDBOX_AUTO_REAP`.
pub fn sys_fips_sandbox(mask: u64, flags: u64) -> i64 {
    let slot = ostd::scheduler::active_slot();
    let pid = ostd::scheduler::active_pid();
    if (flags & SANDBOX_AUTO_REAP) != 0 {
        ostd::scheduler::set_slot_auto_reap(slot, true);
    }
    let (new_mask, new_flags) =
        ostd::scheduler::narrow_slot_capabilities(slot, mask, flags & SANDBOX_RESTRICT_FAT0);
    if new_mask != CAP_ALL || (new_flags & SANDBOX_RESTRICT_FAT0) != 0 {
        ostd::early_println!(
            "[sandbox] PID={} (slot={}) caps={:#x} flags={:#x}",
            pid,
            slot,
            new_mask,
            new_flags
        );
    }
    new_mask as i64
}

/// Verifies that the active slot holds all capability bits in `required`, else `-EPERM (-1)`.
#[inline]
pub fn check_cap(required: u64) -> Result<(), i64> {
    let (cap_mask, _) = ostd::scheduler::slot_capabilities(ostd::scheduler::active_slot());
    if (cap_mask & required) == required {
        Ok(())
    } else {
        Err(-1) // -EPERM
    }
}

/// Verifies network capability (`CAP_NET`) before socket/connect/bind operations.
#[inline]
pub fn check_net_sandbox() -> Result<(), i64> {
    check_cap(CAP_NET)
}

/// Verifies process creation/execution capability (`CAP_SPAWN`).
#[inline]
pub fn check_spawn_sandbox() -> Result<(), i64> {
    check_cap(CAP_SPAWN)
}

/// Verifies raw framebuffer access capability (`CAP_RAW_FB`).
#[inline]
pub fn check_raw_fb_sandbox() -> Result<(), i64> {
    check_cap(CAP_RAW_FB)
}

/// Enforces `CAP_FS_READ`, `CAP_FS_WRITE`, `CAP_RAW_FB`, and `SANDBOX_RESTRICT_FAT0` on `open`/`openat`.
pub fn check_open_sandbox(resolved: &str, flags: u64) -> Result<(), i64> {
    let (cap_mask, sb_flags) = ostd::scheduler::slot_capabilities(ostd::scheduler::active_slot());
    if resolved == "/dev/fb0" && (cap_mask & CAP_RAW_FB) == 0 {
        return Err(-1); // -EPERM
    }

    let acc = flags & 3;
    let is_write = acc == 1 || acc == 2 || (flags & 0x40) != 0 || (flags & 0x200) != 0;
    if is_write {
        if matches!(resolved, "/dev/null" | "/dev/tty" | "/dev/console") {
            return Ok(());
        }
        if (sb_flags & SANDBOX_RESTRICT_FAT0) != 0 {
            if (cap_mask & CAP_FS_WRITE) != 0
                && (resolved == "/mnt/fat0/sandbox" || resolved.starts_with("/mnt/fat0/sandbox/"))
            {
                return Ok(());
            }
            return Err(-1); // -EPERM
        }
        if (cap_mask & CAP_FS_WRITE) == 0 {
            return Err(-1); // -EPERM
        }
        return Ok(());
    }

    if (cap_mask & CAP_FS_READ) == 0
        && !resolved.starts_with("/dev/")
        && !resolved.starts_with("/lib/")
    {
        return Err(-1); // -EPERM
    }
    Ok(())
}

/// Enforces write capability on an already-open file descriptor in `sys_write`.
pub fn check_write_fd_sandbox(path: &str, is_device: bool, is_pipe: bool) -> Result<(), i64> {
    if is_device || is_pipe {
        return Ok(());
    }
    let (cap_mask, sb_flags) = ostd::scheduler::slot_capabilities(ostd::scheduler::active_slot());
    if (sb_flags & SANDBOX_RESTRICT_FAT0) != 0 {
        if (cap_mask & CAP_FS_WRITE) != 0
            && (path == "/mnt/fat0/sandbox" || path.starts_with("/mnt/fat0/sandbox/"))
        {
            return Ok(());
        }
        return Err(-1); // -EPERM
    }
    if (cap_mask & CAP_FS_WRITE) == 0 {
        return Err(-1); // -EPERM
    }
    Ok(())
}

/// Enforces write capability on a resolved path for mutating syscalls (`mkdir`, `unlink`, `rmdir`, `truncate`).
#[inline]
pub fn check_write_path_sandbox(resolved: &str) -> Result<(), i64> {
    check_write_fd_sandbox(resolved, false, false)
}
