#![no_std]
#![forbid(unsafe_code)]

/// Wait outcomes below this value are exit codes; faults OR it with the x86 vector.
pub const FAULT_STATUS_BASE: u64 = 1 << 32;
pub const ABI_VERSION: u32 = 1;
pub const MAX_IO_BYTES: usize = 256;
pub const MAX_PATH_BYTES: usize = 100;
pub const MAX_MESSAGE_BYTES: usize = 64;
pub const CHANNEL_PAIR_BYTES: usize = 16;
pub const SPAWN_GRANTS_BYTES: usize = 24;

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
    ProcessSpawn = 11,
    ProcessWait = 12,
    ChannelCreate = 13,
    ProcessSpawnWithGrants = 14,
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
            11 => Ok(Self::ProcessSpawn),
            12 => Ok(Self::ProcessWait),
            13 => Ok(Self::ChannelCreate),
            14 => Ok(Self::ProcessSpawnWithGrants),
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
    BadExecutable = -10,
    NoMemory = -11,
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

/// ChannelCreate writes two process-local handles as little-endian u64 values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelPair {
    pub sender: Handle,
    pub receiver: Handle,
}

impl ChannelPair {
    pub fn encode(self) -> [u8; CHANNEL_PAIR_BYTES] {
        let mut bytes = [0; CHANNEL_PAIR_BYTES];
        bytes[..8].copy_from_slice(&self.sender.raw().to_le_bytes());
        bytes[8..].copy_from_slice(&self.receiver.raw().to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: &[u8; CHANNEL_PAIR_BYTES] =
            bytes.try_into().map_err(|_| Error::InvalidArgument)?;
        let words = bytes.as_chunks::<8>().0;
        Ok(Self {
            sender: Handle::from_raw(u64::from_le_bytes(words[0])),
            receiver: Handle::from_raw(u64::from_le_bytes(words[1])),
        })
    }
}

/// ProcessSpawnWithGrants retains the parent's console handle and moves nonzero
/// channel sides only on success. The descriptor has three little-endian u64 handles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SpawnGrants {
    pub console: Handle,
    pub sender: Handle,
    pub receiver: Handle,
}

impl SpawnGrants {
    pub const NONE: Self = Self {
        console: Handle::from_raw(0),
        sender: Handle::from_raw(0),
        receiver: Handle::from_raw(0),
    };

    pub fn encode(self) -> [u8; SPAWN_GRANTS_BYTES] {
        let mut bytes = [0; SPAWN_GRANTS_BYTES];
        bytes[..8].copy_from_slice(&self.console.raw().to_le_bytes());
        bytes[8..16].copy_from_slice(&self.sender.raw().to_le_bytes());
        bytes[16..].copy_from_slice(&self.receiver.raw().to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let bytes: &[u8; SPAWN_GRANTS_BYTES] =
            bytes.try_into().map_err(|_| Error::InvalidArgument)?;
        let words = bytes.as_chunks::<8>().0;
        Ok(Self {
            console: Handle::from_raw(u64::from_le_bytes(words[0])),
            sender: Handle::from_raw(u64::from_le_bytes(words[1])),
            receiver: Handle::from_raw(u64::from_le_bytes(words[2])),
        })
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

    #[test]
    fn channel_pair_has_fixed_byte_order_and_exact_length() {
        let pair = ChannelPair {
            sender: Handle::from_raw(0x0102_0304_0506_0708),
            receiver: Handle::from_raw(0x1112_1314_1516_1718),
        };
        let bytes = [8, 7, 6, 5, 4, 3, 2, 1, 24, 23, 22, 21, 20, 19, 18, 17];
        assert_eq!(pair.encode(), bytes);
        assert_eq!(ChannelPair::decode(&bytes), Ok(pair));
        for length in 0..CHANNEL_PAIR_BYTES {
            assert_eq!(
                ChannelPair::decode(&bytes[..length]),
                Err(Error::InvalidArgument)
            );
        }
        assert_eq!(
            ChannelPair::decode(&[0; CHANNEL_PAIR_BYTES + 1]),
            Err(Error::InvalidArgument)
        );
        let mut unaligned = [0; CHANNEL_PAIR_BYTES + 1];
        unaligned[1..].copy_from_slice(&bytes);
        assert_eq!(ChannelPair::decode(&unaligned[1..]), Ok(pair));
    }

    #[test]
    fn spawn_grants_have_exact_order_length_and_allow_absent_handles() {
        let grants = SpawnGrants {
            console: Handle::from_raw(0x0102_0304_0506_0708),
            sender: Handle::from_raw(0x1112_1314_1516_1718),
            receiver: Handle::from_raw(0x2122_2324_2526_2728),
        };
        let bytes = [
            8, 7, 6, 5, 4, 3, 2, 1, 24, 23, 22, 21, 20, 19, 18, 17, 40, 39, 38, 37, 36, 35, 34, 33,
        ];
        assert_eq!(grants.encode(), bytes);
        assert_eq!(SpawnGrants::decode(&bytes), Ok(grants));
        assert_eq!(SpawnGrants::NONE.encode(), [0; SPAWN_GRANTS_BYTES]);
        assert_eq!(
            SpawnGrants::decode(&[0; SPAWN_GRANTS_BYTES]),
            Ok(SpawnGrants::NONE)
        );
        for length in 0..SPAWN_GRANTS_BYTES {
            assert_eq!(
                SpawnGrants::decode(&bytes[..length]),
                Err(Error::InvalidArgument)
            );
        }
        assert_eq!(
            SpawnGrants::decode(&[0; SPAWN_GRANTS_BYTES + 1]),
            Err(Error::InvalidArgument)
        );
        let mut unaligned = [0; SPAWN_GRANTS_BYTES + 1];
        unaligned[1..].copy_from_slice(&bytes);
        assert_eq!(SpawnGrants::decode(&unaligned[1..]), Ok(grants));
        assert_eq!(Syscall::try_from(14), Ok(Syscall::ProcessSpawnWithGrants));
    }
}
