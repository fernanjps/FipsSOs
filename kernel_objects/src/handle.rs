#![forbid(unsafe_code)]

use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use bitflags::bitflags;
use core::fmt;

use crate::channel::Channel;
use crate::object::{KernelObject, Koid, ObjectType};
use crate::vmo::Vmo;

bitflags! {
    /// Capability rights associated with a `Handle` in a `HandleTable`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct Rights: u32 {
        /// Permission to read data from the underlying kernel object.
        const READ      = 1 << 0;
        /// Permission to write/modify data in the underlying kernel object.
        const WRITE     = 1 << 1;
        /// Permission to execute code mapped from the underlying kernel object.
        const EXECUTE   = 1 << 2;
        /// Permission to map the kernel object (e.g., `Vmo`) into an address space or device IOMMU.
        const MAP       = 1 << 3;
        /// Permission to transfer the handle over an IPC channel to another process/driver.
        const TRANSFER  = 1 << 4;

        /// Default full rights for newly created objects.
        const ALL       = Self::READ.bits()
                        | Self::WRITE.bits()
                        | Self::EXECUTE.bits()
                        | Self::MAP.bits()
                        | Self::TRANSFER.bits();
    }
}

impl fmt::Display for Rights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        bitflags::parser::to_writer(self, f)
    }
}

/// Process-local capability descriptor (Handle ID).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HandleId(u32);

impl HandleId {
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for HandleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "handle:{:#06x}", self.0)
    }
}

/// Errors returned when querying or manipulating a `HandleTable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleError {
    /// The specified `HandleId` does not exist in this `HandleTable`.
    NotFound(HandleId),
    /// The handle exists, but lacks the required capability `Rights`.
    AccessDenied {
        handle: HandleId,
        required: Rights,
        actual: Rights,
    },
    /// The object referenced by the handle is not of the expected `ObjectType`.
    WrongObjectType {
        handle: HandleId,
        expected: ObjectType,
        actual: ObjectType,
    },
}

/// Entry stored inside a `HandleTable`, pairing a shared `KernelObject` with capability `Rights`.
#[derive(Clone)]
pub struct HandleEntry {
    handle_id: HandleId,
    object: Arc<dyn KernelObject>,
    rights: Rights,
}

impl HandleEntry {
    #[inline]
    pub fn handle_id(&self) -> HandleId {
        self.handle_id
    }

    #[inline]
    pub fn koid(&self) -> Koid {
        self.object.koid()
    }

    #[inline]
    pub fn object_type(&self) -> ObjectType {
        self.object.object_type()
    }

    #[inline]
    pub fn rights(&self) -> Rights {
        self.rights
    }

    #[inline]
    pub fn object(&self) -> &Arc<dyn KernelObject> {
        &self.object
    }

    /// Returns a reference to the underlying `Vmo` if this handle points to a `Vmo`.
    pub fn as_vmo(&self) -> Result<&Vmo, HandleError> {
        self.object
            .as_vmo()
            .ok_or(HandleError::WrongObjectType {
                handle: self.handle_id,
                expected: ObjectType::Vmo,
                actual: self.object.object_type(),
            })
    }

    /// Returns a reference to the underlying `Channel` if this handle points to a `Channel`.
    pub fn as_channel(&self) -> Result<&Channel, HandleError> {
        self.object
            .as_channel()
            .ok_or(HandleError::WrongObjectType {
                handle: self.handle_id,
                expected: ObjectType::Channel,
                actual: self.object.object_type(),
            })
    }
}

impl fmt::Debug for HandleEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandleEntry")
            .field("handle_id", &self.handle_id)
            .field("koid", &self.koid())
            .field("object_type", &self.object_type())
            .field("rights", &self.rights)
            .finish()
    }
}

/// Capability table mapping local `HandleId` descriptors to kernel objects and `Rights`.
pub struct HandleTable {
    entries: BTreeMap<HandleId, HandleEntry>,
    next_id: u32,
}

impl HandleTable {
    /// Creates a new empty `HandleTable`.
    pub const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            next_id: 1,
        }
    }

    /// Inserts a `KernelObject` into the table with the specified `Rights`, returning its `HandleId`.
    pub fn insert(&mut self, object: Arc<dyn KernelObject>, rights: Rights) -> HandleId {
        let id = HandleId(self.next_id);
        self.next_id += 1;

        let entry = HandleEntry {
            handle_id: id,
            object,
            rights,
        };
        self.entries.insert(id, entry);
        id
    }

    /// Looks up a handle and verifies that it possesses at least `required_rights`.
    pub fn get(
        &self,
        handle: HandleId,
        required_rights: Rights,
    ) -> Result<&HandleEntry, HandleError> {
        let entry = self
            .entries
            .get(&handle)
            .ok_or(HandleError::NotFound(handle))?;

        if !entry.rights.contains(required_rights) {
            return Err(HandleError::AccessDenied {
                handle,
                required: required_rights,
                actual: entry.rights,
            });
        }

        Ok(entry)
    }

    /// Duplicates an existing handle with equal or reduced rights (`new_rights` must be a subset).
    pub fn duplicate(
        &mut self,
        handle: HandleId,
        new_rights: Rights,
    ) -> Result<HandleId, HandleError> {
        let existing = self.get(handle, new_rights)?;
        let obj = Arc::clone(&existing.object);
        Ok(self.insert(obj, new_rights))
    }

    /// Removes and returns a `HandleEntry` from the table, closing the capability.
    pub fn remove(&mut self, handle: HandleId) -> Result<HandleEntry, HandleError> {
        self.entries
            .remove(&handle)
            .ok_or(HandleError::NotFound(handle))
    }

    /// Number of active handles in the table.
    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the table has no active handles.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for HandleTable {
    fn default() -> Self {
        Self::new()
    }
}
