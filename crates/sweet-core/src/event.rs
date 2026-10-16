//! Synchronous DOM event propagation shared by language bindings.
use crate::{Document, NodeId, SweetError};

/// Callback response bits: prevent default, stop propagation, stop immediately.
pub const PREVENT_DEFAULT: u32 = 1;
pub const STOP_PROPAGATION: u32 = 2;
pub const STOP_IMMEDIATE: u32 = 4;

#[derive(Clone, Copy, Debug)]
pub struct EventVisit {
    pub current_target: NodeId,
    /// DOM eventPhase: capturing = 1, target = 2, bubbling = 3.
    pub phase: u32,
    pub capture: bool,
}

impl Document {
    /// Snapshot the ancestry before callbacks: listeners may mutate the DOM.
    /// Callbacks run without holding the document lock and may dispatch events.
    pub fn dispatch_event(
        &self,
        target: NodeId,
        bubbles: bool,
        cancelable: bool,
        mut listener: impl FnMut(EventVisit) -> u32,
    ) -> Result<bool, SweetError> {
        let mut path = vec![target];
        let mut current = self.parent(target)?;
        while let Some(id) = current {
            path.push(id);
            current = self.parent(id)?;
        }
        let mut flags = 0;
        for &id in path.iter().skip(1).rev() {
            flags |= listener(EventVisit { current_target: id, phase: 1, capture: true });
            if flags & (STOP_PROPAGATION | STOP_IMMEDIATE) != 0 {
                return Ok(!cancelable || flags & PREVENT_DEFAULT == 0);
            }
        }
        flags |= listener(EventVisit { current_target: target, phase: 2, capture: true });
        if flags & STOP_IMMEDIATE == 0 {
            flags |= listener(EventVisit { current_target: target, phase: 2, capture: false });
        }
        if bubbles {
            for &id in path.iter().skip(1) {
                if flags & (STOP_PROPAGATION | STOP_IMMEDIATE) != 0 { break; }
                flags |= listener(EventVisit { current_target: id, phase: 3, capture: false });
            }
        }
        Ok(!cancelable || flags & PREVENT_DEFAULT == 0)
    }
}
