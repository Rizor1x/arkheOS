use crate::ipc::MessageQueue;
use arkhe_abi::{Error, Message};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EndpointId {
    slot: usize,
    generation: u64,
}
struct Slot<const Q: usize> {
    generation: u64,
    occupied: bool,
    sender: bool,
    receiver: bool,
    queue: MessageQueue<Q>,
}
impl<const Q: usize> Slot<Q> {
    const fn new() -> Self {
        Self {
            generation: 1,
            occupied: false,
            sender: false,
            receiver: false,
            queue: MessageQueue::new(),
        }
    }
}

/// Each allocation grants one sender and one receiver. Duplication is not supported.
pub struct Endpoints<const N: usize, const Q: usize> {
    slots: [Slot<Q>; N],
}
impl<const N: usize, const Q: usize> Endpoints<N, Q> {
    pub const fn new() -> Self {
        Self {
            slots: [const { Slot::new() }; N],
        }
    }
    pub fn create(&mut self) -> Result<EndpointId, Error> {
        let (index, slot) = self
            .slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| !slot.occupied && slot.generation != 0)
            .ok_or(Error::NoSpace)?;
        slot.occupied = true;
        slot.sender = true;
        slot.receiver = true;
        slot.queue = MessageQueue::new();
        Ok(EndpointId {
            slot: index,
            generation: slot.generation,
        })
    }
    pub fn send(&mut self, id: EndpointId, message: Message) -> Result<(), Error> {
        let slot = self.get(id)?;
        if !slot.sender {
            return Err(Error::InvalidHandle);
        }
        if !slot.receiver {
            return Err(Error::PeerClosed);
        }
        slot.queue.send(message)
    }
    /// Leave the head message intact if capacity or user-copy validation fails.
    pub fn receive_with(
        &mut self,
        id: EndpointId,
        capacity: usize,
        copy: impl FnOnce(&[u8]) -> Result<(), Error>,
    ) -> Result<usize, Error> {
        let slot = self.get(id)?;
        if !slot.receiver {
            return Err(Error::InvalidHandle);
        }
        let message = match slot.queue.front() {
            Err(Error::WouldBlock) if !slot.sender => return Err(Error::PeerClosed),
            result => result?,
        };
        if capacity < message.payload().len() {
            return Err(Error::BufferTooSmall);
        }
        copy(message.payload())?;
        slot.queue.receive()?;
        Ok(message.payload().len())
    }
    pub fn close_sender(&mut self, id: EndpointId) -> Result<(), Error> {
        self.close(id, true)
    }
    pub fn close_receiver(&mut self, id: EndpointId) -> Result<(), Error> {
        self.close(id, false)
    }
    pub fn live_count(&self) -> usize {
        self.slots.iter().filter(|s| s.occupied).count()
    }
    fn close(&mut self, id: EndpointId, sender: bool) -> Result<(), Error> {
        let slot = self.get(id)?;
        let side = if sender {
            &mut slot.sender
        } else {
            &mut slot.receiver
        };
        if !*side {
            return Err(Error::InvalidHandle);
        }
        *side = false;
        if !slot.receiver {
            slot.queue = MessageQueue::new();
        }
        if !slot.sender && !slot.receiver {
            slot.occupied = false;
            slot.generation = slot.generation.checked_add(1).unwrap_or(0);
        }
        Ok(())
    }
    fn get(&mut self, id: EndpointId) -> Result<&mut Slot<Q>, Error> {
        let slot = self.slots.get_mut(id.slot).ok_or(Error::InvalidHandle)?;
        if !slot.occupied || slot.generation != id.generation {
            return Err(Error::InvalidHandle);
        }
        Ok(slot)
    }
}
impl<const N: usize, const Q: usize> Default for Endpoints<N, Q> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn copy_failure_small_buffers_and_backpressure_preserve_fifo() {
        let mut endpoints = Endpoints::<1, 2>::new();
        let id = endpoints.create().unwrap();
        assert_eq!(endpoints.create(), Err(Error::NoSpace));
        endpoints.send(id, Message::new(b"ab").unwrap()).unwrap();
        endpoints.send(id, Message::new(b"c").unwrap()).unwrap();
        assert_eq!(
            endpoints.send(id, Message::new(b"d").unwrap()),
            Err(Error::WouldBlock)
        );
        assert_eq!(
            endpoints.receive_with(id, 1, |_| panic!("copy must not run")),
            Err(Error::BufferTooSmall)
        );
        assert_eq!(
            endpoints.receive_with(id, 2, |_| Err(Error::InvalidArgument)),
            Err(Error::InvalidArgument)
        );
        endpoints
            .receive_with(id, 2, |bytes| {
                assert_eq!(bytes, b"ab");
                Ok(())
            })
            .unwrap();
        endpoints
            .receive_with(id, 2, |bytes| {
                assert_eq!(bytes, b"c");
                Ok(())
            })
            .unwrap();
        assert_eq!(
            endpoints.receive_with(id, 2, |_| Ok(())),
            Err(Error::WouldBlock)
        );
    }
    #[test]
    fn closes_drain_queued_messages_and_prevent_stale_object_reuse() {
        let mut endpoints = Endpoints::<1, 2>::new();
        let old = endpoints.create().unwrap();
        endpoints.send(old, Message::new(b"tail").unwrap()).unwrap();
        endpoints.close_sender(old).unwrap();
        assert_eq!(endpoints.close_sender(old), Err(Error::InvalidHandle));
        assert_eq!(endpoints.receive_with(old, 4, |_| Ok(())), Ok(4));
        assert_eq!(
            endpoints.receive_with(old, 4, |_| Ok(())),
            Err(Error::PeerClosed)
        );
        endpoints.close_receiver(old).unwrap();
        assert_eq!(endpoints.live_count(), 0);
        let next = endpoints.create().unwrap();
        assert_ne!(old, next);
        assert_eq!(
            endpoints.send(old, Message::new(b"").unwrap()),
            Err(Error::InvalidHandle)
        );
        assert_eq!(endpoints.close_receiver(old), Err(Error::InvalidHandle));
        endpoints.close_receiver(next).unwrap();
        assert_eq!(
            endpoints.send(next, Message::new(b"").unwrap()),
            Err(Error::PeerClosed)
        );
        endpoints.close_sender(next).unwrap();
    }
    #[test]
    fn empty_payload_zero_capacity_and_generation_exhaustion() {
        let mut endpoints = Endpoints::<1, 1>::new();
        endpoints.slots[0].generation = u64::MAX;
        let id = endpoints.create().unwrap();
        endpoints.send(id, Message::new(b"").unwrap()).unwrap();
        assert_eq!(
            endpoints.receive_with(id, 0, |data| {
                assert!(data.is_empty());
                Ok(())
            }),
            Ok(0)
        );
        endpoints.close_receiver(id).unwrap();
        endpoints.close_sender(id).unwrap();
        assert_eq!(endpoints.create(), Err(Error::NoSpace));
        assert_eq!(Endpoints::<0, 1>::new().create(), Err(Error::NoSpace));
        let mut zero = Endpoints::<1, 0>::new();
        let id = zero.create().unwrap();
        assert_eq!(
            zero.send(id, Message::new(b"").unwrap()),
            Err(Error::WouldBlock)
        );
    }
}
