#![no_std]
#![forbid(unsafe_code)]

pub const ABI_VERSION: u32 = 1;
pub const MAX_IO_BYTES: usize = 256;
pub const MAX_PATH_BYTES: usize = 100;
pub const MAX_MESSAGE_BYTES: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u64)]
pub enum Syscall {
    Yield = 0,
    Send = 1,
    Receive = 2,
    Close = 3,
    DebugWrite = 4,
    Exit = 5,
    Version = 6,
    ClockTicks = 7,
    FileOpen = 8,
    FileRead = 9,
    SleepTicks = 10,
}

impl TryFrom<u64> for Syscall {
    type Error = Error;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Self::Yield),
            1 => Ok(Self::Send),
            2 => Ok(Self::Receive),
            3 => Ok(Self::Close),
            4 => Ok(Self::DebugWrite),
            5 => Ok(Self::Exit),
            6 => Ok(Self::Version),
            7 => Ok(Self::ClockTicks),
            8 => Ok(Self::FileOpen),
            9 => Ok(Self::FileRead),
            10 => Ok(Self::SleepTicks),
            _ => Err(Error::Unsupported),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum Error {
    InvalidArgument = -1,
    PermissionDenied = -2,
    InvalidHandle = -3,
    WouldBlock = -4,
    NoSpace = -5,
    Unsupported = -6,
    NotFound = -7,
    PeerClosed = -8,
    BufferTooSmall = -9,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Message {
    bytes: [u8; MAX_MESSAGE_BYTES],
    len: u8,
}

impl Message {
    pub fn new(payload: &[u8]) -> Result<Self, Error> {
        if payload.len() > MAX_MESSAGE_BYTES {
            return Err(Error::InvalidArgument);
        }
        let mut bytes = [0; MAX_MESSAGE_BYTES];
        bytes[..payload.len()].copy_from_slice(payload);
        Ok(Self {
            bytes,
            len: payload.len() as u8,
        })
    }

    pub fn payload(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_syscalls_without_truncation() {
        assert_eq!(Syscall::try_from(0), Ok(Syscall::Yield));
        assert_eq!(Syscall::try_from(u64::MAX), Err(Error::Unsupported));
        assert_eq!(Syscall::try_from(1 << 32), Err(Error::Unsupported));
    }

    #[test]
    fn bounds_message_payload() {
        assert_eq!(Message::new(&[0; 65]), Err(Error::InvalidArgument));
        assert_eq!(Message::new(&[7; 64]).unwrap().payload(), &[7; 64]);
        assert!(Message::new(&[]).unwrap().payload().is_empty());
    }
}
