#![forbid(unsafe_code)]

use alloc::collections::VecDeque;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use ostd::sync::SpinLock;

use crate::handle::HandleId;
use crate::object::{KernelObject, Koid, ObjectType};

/// Maximum number of byte payload allowed in a single Zircon-style `MessagePacket` (64 KiB).
pub const MAX_CHANNEL_MSG_BYTES: usize = 64 * 1024;

/// Maximum number of capability handles transferable in a single `MessagePacket` (64 handles).
pub const MAX_CHANNEL_MSG_HANDLES: usize = 64;

/// Errors returned by Zircon-style `Channel` IPC operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// The remote peer endpoint of the channel has been dropped/closed.
    PeerClosed,
    /// The message data payload exceeds `MAX_CHANNEL_MSG_BYTES`.
    PayloadTooLarge(usize),
    /// The number of attached capability handles exceeds `MAX_CHANNEL_MSG_HANDLES`.
    TooManyHandles(usize),
}

/// Zircon-style IPC message packet carrying raw byte payload and capability `HandleId` descriptors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessagePacket {
    pub data: Vec<u8>,
    pub handles: Vec<HandleId>,
}

impl MessagePacket {
    /// Creates a new `MessagePacket` containing `data` and transferred `handles`.
    pub fn new(data: Vec<u8>, handles: Vec<HandleId>) -> Self {
        Self { data, handles }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EndpointSide {
    SideA,
    SideB,
}

struct DirectionQueue {
    messages: VecDeque<MessagePacket>,
    reader_waker: Option<Waker>,
    peer_closed: bool,
}

impl DirectionQueue {
    const fn new() -> Self {
        Self {
            messages: VecDeque::new(),
            reader_waker: None,
            peer_closed: false,
        }
    }
}

struct ChannelShared {
    a_to_b: SpinLock<DirectionQueue>,
    b_to_a: SpinLock<DirectionQueue>,
}

/// Bidirectional capability-transferring IPC endpoint (inspired by Zircon `zx::channel`).
pub struct Channel {
    koid: Koid,
    peer_koid: Koid,
    side: EndpointSide,
    shared: Arc<ChannelShared>,
}

impl Channel {
    /// Creates an entangled pair of bidirectional IPC `Channel` endpoints `(endpoint_a, endpoint_b)`.
    pub fn create_pair() -> (Arc<Self>, Arc<Self>) {
        let koid_a = Koid::allocate();
        let koid_b = Koid::allocate();

        let shared = Arc::new(ChannelShared {
            a_to_b: SpinLock::new(DirectionQueue::new()),
            b_to_a: SpinLock::new(DirectionQueue::new()),
        });

        let endpoint_a = Arc::new(Self {
            koid: koid_a,
            peer_koid: koid_b,
            side: EndpointSide::SideA,
            shared: Arc::clone(&shared),
        });

        let endpoint_b = Arc::new(Self {
            koid: koid_b,
            peer_koid: koid_a,
            side: EndpointSide::SideB,
            shared,
        });

        (endpoint_a, endpoint_b)
    }

    /// Returns the `Koid` of the entangled peer endpoint.
    #[inline]
    pub fn peer_koid(&self) -> Koid {
        self.peer_koid
    }

    #[inline]
    fn outbound_queue(&self) -> &SpinLock<DirectionQueue> {
        match self.side {
            EndpointSide::SideA => &self.shared.a_to_b,
            EndpointSide::SideB => &self.shared.b_to_a,
        }
    }

    #[inline]
    fn inbound_queue(&self) -> &SpinLock<DirectionQueue> {
        match self.side {
            EndpointSide::SideA => &self.shared.b_to_a,
            EndpointSide::SideB => &self.shared.a_to_b,
        }
    }

    /// Enqueues a `MessagePacket` (bytes + `HandleId` capabilities) to the peer endpoint
    /// and wakes any asynchronous task awaiting `read_msg_async` on the peer.
    pub fn write_msg(&self, packet: MessagePacket) -> Result<(), ChannelError> {
        if packet.data.len() > MAX_CHANNEL_MSG_BYTES {
            return Err(ChannelError::PayloadTooLarge(packet.data.len()));
        }
        if packet.handles.len() > MAX_CHANNEL_MSG_HANDLES {
            return Err(ChannelError::TooManyHandles(packet.handles.len()));
        }

        let waker_to_notify = {
            let mut guard = self.outbound_queue().lock();
            if guard.peer_closed {
                return Err(ChannelError::PeerClosed);
            }
            guard.messages.push_back(packet);
            guard.reader_waker.take()
        };

        if let Some(waker) = waker_to_notify {
            waker.wake();
        }

        Ok(())
    }

    /// Attempts a non-blocking read from the inbound queue of this channel endpoint.
    pub fn try_read_msg(&self) -> Result<Option<MessagePacket>, ChannelError> {
        let mut guard = self.inbound_queue().lock();
        if let Some(pkt) = guard.messages.pop_front() {
            return Ok(Some(pkt));
        }
        if guard.peer_closed {
            return Err(ChannelError::PeerClosed);
        }
        Ok(None)
    }

    /// Asynchronously waits for the next `MessagePacket` on this endpoint using the kernel `Waker`
    /// without blocking the CPU.
    pub fn read_msg_async(&self) -> ChannelReadFuture<'_> {
        ChannelReadFuture { channel: self }
    }
}

impl Drop for Channel {
    fn drop(&mut self) {
        let waker_to_notify = {
            let mut outbound = self.outbound_queue().lock();
            outbound.peer_closed = true;
            outbound.reader_waker.take()
        };
        self.inbound_queue().lock().peer_closed = true;

        if let Some(waker) = waker_to_notify {
            waker.wake();
        }
    }
}

impl fmt::Debug for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("koid", &self.koid)
            .field("peer_koid", &self.peer_koid)
            .field("side", &self.side)
            .finish()
    }
}

impl KernelObject for Channel {
    fn koid(&self) -> Koid {
        self.koid
    }

    fn object_type(&self) -> ObjectType {
        ObjectType::Channel
    }

    fn as_channel(&self) -> Option<&Channel> {
        Some(self)
    }
}

/// Future returned by [`Channel::read_msg_async`] that resolves when a [`MessagePacket`]
/// arrives on the channel endpoint or the peer closes.
pub struct ChannelReadFuture<'a> {
    channel: &'a Channel,
}

impl<'a> Future for ChannelReadFuture<'a> {
    type Output = Result<MessagePacket, ChannelError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut guard = self.channel.inbound_queue().lock();
        if let Some(packet) = guard.messages.pop_front() {
            return Poll::Ready(Ok(packet));
        }
        if guard.peer_closed {
            return Poll::Ready(Err(ChannelError::PeerClosed));
        }
        guard.reader_waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
