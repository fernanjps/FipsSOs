//! Zero-Syscall Lock-Free Shared Memory Ring Buffers (`ostd::ipc`).
//!
//! Provides high-throughput, zero-copy inter-process and kernel communication
//! via Submission Queue (SQ) and Completion Queue (CQ) structures mapped in
//! shared Virtual Memory Objects (VMOs).

use core::sync::atomic::{AtomicU32, Ordering};
use crate::mm::{PhysAddr, VirtAddr};
use crate::sync::SpinLock;

/// Magic identifier for Zero-Syscall VMO I/O Rings (`'VMOR'`).
pub const VMO_RING_MAGIC: u32 = 0x564D_4F52;
/// Protocol version for Zero-Syscall VMO I/O Rings.
pub const VMO_RING_VERSION: u32 = 1;
/// Queue capacity (depth) of the ring buffers (32 entries).
pub const VMO_RING_QUEUE_DEPTH: usize = 32;

/// OpCodes for Submission Queue Entries (SQEs).
pub const VMO_RING_OP_NOP: u32 = 0;
pub const VMO_RING_OP_ECHO: u32 = 1;
pub const VMO_RING_OP_3D_TRANSFORM: u32 = 2;
pub const VMO_RING_OP_NET_POLL: u32 = 3;
pub const VMO_RING_OP_MEM_CRC: u32 = 4;

/// Submission Queue Entry (SQE) produced by Ring 3 userland, consumed by Ring 0 kernel.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmoSqEntry {
    pub opcode: u32,
    pub flags: u32,
    pub user_data: u64,
    pub buffer_offset: u64,
    pub len: u32,
    pub reserved: u32,
}

/// Completion Queue Entry (CQE) produced by Ring 0 kernel, consumed by Ring 3 userland.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VmoCqEntry {
    pub user_data: u64,
    pub result: i32,
    pub flags: u32,
}

/// Fixed header layout residing at byte 0 of the Shared Ring VMO.
#[repr(C)]
pub struct VmoRingHeaderRaw {
    pub magic: u32,
    pub version: u32,
    pub queue_depth: u32,
    pub flags: u32,
    pub sq_head: AtomicU32,
    pub sq_tail: AtomicU32,
    pub cq_head: AtomicU32,
    pub cq_tail: AtomicU32,
    pub sq_dropped: AtomicU32,
    pub cq_overflow: AtomicU32,
    pub reserved: [u32; 6],
}

struct RegisteredRing {
    phys: PhysAddr,
    virt: VirtAddr,
    active: bool,
}

static REGISTERED_RINGS: SpinLock<[RegisteredRing; 4]> = SpinLock::new([
    RegisteredRing { phys: 0, virt: 0, active: false },
    RegisteredRing { phys: 0, virt: 0, active: false },
    RegisteredRing { phys: 0, virt: 0, active: false },
    RegisteredRing { phys: 0, virt: 0, active: false },
]);

/// Registers an active shared VMO ring buffer located at `phys` (and HHDM `virt`).
pub fn register_shared_ring(phys: PhysAddr, virt: VirtAddr) {
    let mut rings = REGISTERED_RINGS.lock();
    for ring in rings.iter_mut() {
        if !ring.active || ring.phys == phys {
            ring.phys = phys;
            ring.virt = virt;
            ring.active = true;
            return;
        }
    }
}

/// Polls and processes all pending Submission Queue entries across registered rings.
///
/// Returns the number of requests processed.
pub fn poll_active_rings() -> usize {
    let rings = REGISTERED_RINGS.lock();
    let mut total_processed = 0usize;

    for ring in rings.iter() {
        if !ring.active || ring.virt == 0 {
            continue;
        }

        let base_ptr = ring.virt as *mut u8;
        // SAFETY: `ring.virt` points to the Higher-Half Direct Map (HHDM) of an allocated contiguous VMO.
        unsafe {
            let header = &*(base_ptr as *const VmoRingHeaderRaw);
            if header.magic != VMO_RING_MAGIC {
                continue;
            }

            let sq_tail = header.sq_tail.load(Ordering::Acquire);
            let mut sq_head = header.sq_head.load(Ordering::Relaxed);

            if sq_head == sq_tail {
                continue;
            }

            let sq_entries = (base_ptr.add(64)) as *mut VmoSqEntry;
            let cq_entries = (base_ptr.add(64 + 32 * core::mem::size_of::<VmoSqEntry>())) as *mut VmoCqEntry;
            let data_base = base_ptr.add(2048);

            while sq_head != sq_tail {
                let sq_idx = (sq_head as usize) % VMO_RING_QUEUE_DEPTH;
                let sqe = *sq_entries.add(sq_idx);

                let result = match sqe.opcode {
                    VMO_RING_OP_NOP => 0,
                    VMO_RING_OP_ECHO => {
                        // Echo operation: verify and return payload length
                        sqe.len as i32
                    }
                    VMO_RING_OP_3D_TRANSFORM => {
                        // High-throughput 3D vertex scaling/transform
                        if sqe.buffer_offset < 60 * 1024 && sqe.len >= 12 {
                            let vertex_ptr = data_base.add(sqe.buffer_offset as usize) as *mut f32;
                            let count = (sqe.len as usize / 12).min(256);
                            for v in 0..count {
                                let x = *vertex_ptr.add(v * 3);
                                let y = *vertex_ptr.add(v * 3 + 1);
                                let z = *vertex_ptr.add(v * 3 + 2);
                                *vertex_ptr.add(v * 3) = x * 1.05;
                                *vertex_ptr.add(v * 3 + 1) = y * 1.05;
                                *vertex_ptr.add(v * 3 + 2) = z * 1.05;
                            }
                        }
                        sqe.len as i32
                    }
                    VMO_RING_OP_NET_POLL => {
                        if crate::net::snapshot().link_up { 1 } else { 0 }
                    }
                    VMO_RING_OP_MEM_CRC => {
                        // Computes a fast CRC/checksum over the payload
                        if sqe.buffer_offset < 60 * 1024 {
                            let slice_len = (sqe.len as usize).min(4096);
                            let slice = core::slice::from_raw_parts(
                                data_base.add(sqe.buffer_offset as usize),
                                slice_len,
                            );
                            let mut crc = 0u32;
                            for &b in slice {
                                crc = crc.wrapping_mul(31).wrapping_add(b as u32);
                            }
                            (crc & 0x7FFF_FFFF) as i32
                        } else {
                            -22 // -EINVAL
                        }
                    }
                    _ => -38, // -ENOSYS
                };

                let cq_tail = header.cq_tail.load(Ordering::Relaxed);
                let cq_idx = (cq_tail as usize) % VMO_RING_QUEUE_DEPTH;
                *cq_entries.add(cq_idx) = VmoCqEntry {
                    user_data: sqe.user_data,
                    result,
                    flags: 0,
                };
                header.cq_tail.store(cq_tail.wrapping_add(1), Ordering::Release);

                sq_head = sq_head.wrapping_add(1);
                header.sq_head.store(sq_head, Ordering::Release);
                total_processed += 1;
            }
        }
    }

    total_processed
}
