#![forbid(unsafe_code)]
#![no_std]

//! Zircon-style capability-based kernel objects for FipSOs.
//! Strictly forbids `unsafe_code`; all hardware and memory abstractions go through `ostd`.

extern crate alloc;

pub mod channel;
pub mod executor;
pub mod handle;
pub mod object;
pub mod process;
pub mod thread;
pub mod vmo;

pub use channel::{
    Channel, ChannelError, ChannelReadFuture, MessagePacket, MAX_CHANNEL_MSG_BYTES,
    MAX_CHANNEL_MSG_HANDLES,
};
pub use executor::{yield_now, Executor, Task, TaskId, YieldNow};
pub use handle::{HandleEntry, HandleError, HandleId, HandleTable, Rights};
pub use object::{KernelObject, Koid, ObjectType};
pub use process::{Process, ProcessState, ProcessThreadRecord, SchedulerRegistry};
pub use thread::{Thread, ThreadState};
pub use vmo::{
    Vmo, VmoCqEntry, VmoError, VmoRingHeader, VmoSqEntry, VmoUsage, VMO_RING_MAGIC,
    VMO_RING_OP_3D_TRANSFORM, VMO_RING_OP_ECHO, VMO_RING_OP_MEM_CRC, VMO_RING_OP_NET_POLL,
    VMO_RING_OP_NOP, VMO_RING_QUEUE_DEPTH, VMO_RING_VERSION,
};
