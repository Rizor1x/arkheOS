use arkhe_abi::{Error, Message};

pub struct MessageQueue<const N: usize> {
    slots: [Option<Message>; N],
    head: usize,
    len: usize,
}

impl<const N: usize> Default for MessageQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> MessageQueue<N> {
    pub const fn new() -> Self {
        Self {
            slots: [None; N],
            head: 0,
            len: 0,
        }
    }

    pub fn send(&mut self, message: Message) -> Result<(), Error> {
        if self.len == N {
            return Err(Error::WouldBlock);
        }
        let tail = (self.head + self.len) % N;
        self.slots[tail] = Some(message);
        self.len += 1;
        Ok(())
    }

    pub fn front(&self) -> Result<Message, Error> {
        if self.len == 0 {
            return Err(Error::WouldBlock);
        }
        self.slots[self.head].ok_or(Error::WouldBlock)
    }

    pub fn receive(&mut self) -> Result<Message, Error> {
        if self.len == 0 {
            return Err(Error::WouldBlock);
        }
        let message = self.slots[self.head].take().ok_or(Error::WouldBlock)?;
        self.head = (self.head + 1) % N;
        self.len -= 1;
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_fifo_order_after_wrap_and_backpressure() {
        let mut queue = MessageQueue::<2>::new();
        let a = Message::new(b"a").unwrap();
        let b = Message::new(b"b").unwrap();
        let c = Message::new(b"c").unwrap();
        queue.send(a).unwrap();
        queue.send(b).unwrap();
        assert_eq!(queue.send(c), Err(Error::WouldBlock));
        assert_eq!(queue.receive(), Ok(a));
        queue.send(c).unwrap();
        assert_eq!(queue.receive(), Ok(b));
        assert_eq!(queue.receive(), Ok(c));
        assert_eq!(queue.receive(), Err(Error::WouldBlock));
    }

    #[test]
    fn zero_capacity_queue_returns_errors() {
        let mut queue = MessageQueue::<0>::new();
        assert_eq!(
            queue.send(Message::new(b"").unwrap()),
            Err(Error::WouldBlock)
        );
        assert_eq!(queue.receive(), Err(Error::WouldBlock));
    }
}
