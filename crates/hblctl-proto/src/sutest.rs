//! The 252-byte `sutest` record: one factory-test command or its result.
//!
//! ```text
//! 0x00 u32 LE  cmd     command id (e.g. 49 ProdConfig, 52 OsSystemCommand, 65 HblShell)
//! 0x04 u32 LE  func    sub-function of the command (e.g. ProdConfig: 0 print, 1 set, 2 get, 3 init)
//! 0x08 u32 LE  cookie  chosen by the client, echoed in the result
//! 0x0C u32 LE  crc     CRC-16/XMODEM of bytes 0x10..0xFC, zero-extended
//! 0x10 ..0xFC  data    236 bytes; command-specific (requests put arguments from 0x14)
//! ```
//! Field meanings inside `data` are defined per command by the command registry, not here.

use crate::DecodeError;
use crate::crc::crc16_xmodem;

/// Size of the record on the wire.
pub const SIZE: usize = 252;
/// Offset where the CRC-covered region starts.
pub const DATA_OFFSET: usize = 0x10;
/// Size of the CRC-covered region.
pub const DATA_SIZE: usize = SIZE - DATA_OFFSET;

#[derive(Clone, PartialEq, Eq)]
pub struct Sutest {
    pub cmd: u32,
    pub func: u32,
    pub cookie: u32,
    /// Bytes 0x10..0xFC.
    pub data: [u8; DATA_SIZE],
}

impl std::fmt::Debug for Sutest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let used = self.data.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
        f.debug_struct("Sutest")
            .field("cmd", &self.cmd)
            .field("func", &self.func)
            .field("cookie", &format_args!("0x{:08x}", self.cookie))
            .field("data", &format_args!("{:02x?}", &self.data[..used]))
            .finish()
    }
}

impl Sutest {
    pub fn new(cmd: u32, func: u32, cookie: u32) -> Self {
        Self {
            cmd,
            func,
            cookie,
            data: [0; DATA_SIZE],
        }
    }

    /// Write `bytes` at absolute record offset `offset` (must be >= 0x10).
    pub fn put(&mut self, offset: usize, bytes: &[u8]) -> Result<(), DecodeError> {
        let start = offset.checked_sub(DATA_OFFSET).ok_or(DecodeError::Short {
            need: DATA_OFFSET,
            have: offset,
        })?;
        let end = start + bytes.len();
        if end > DATA_SIZE {
            return Err(DecodeError::PayloadTooLong {
                len: bytes.len(),
                max: DATA_SIZE.saturating_sub(start),
            });
        }
        self.data[start..end].copy_from_slice(bytes);
        Ok(())
    }

    /// Read `len` bytes at absolute record offset `offset`.
    pub fn get(&self, offset: usize, len: usize) -> Result<&[u8], DecodeError> {
        let start = offset.checked_sub(DATA_OFFSET).ok_or(DecodeError::Short {
            need: DATA_OFFSET,
            have: offset,
        })?;
        self.data.get(start..start + len).ok_or(DecodeError::Short {
            need: offset + len,
            have: SIZE,
        })
    }

    pub fn u32_at(&self, offset: usize) -> Result<u32, DecodeError> {
        Ok(u32::from_le_bytes(
            self.get(offset, 4)?.try_into().expect("4 bytes"),
        ))
    }

    pub fn crc(&self) -> u16 {
        crc16_xmodem(&self.data)
    }

    pub fn encode(&self) -> [u8; SIZE] {
        let mut b = [0u8; SIZE];
        b[0..4].copy_from_slice(&self.cmd.to_le_bytes());
        b[4..8].copy_from_slice(&self.func.to_le_bytes());
        b[8..12].copy_from_slice(&self.cookie.to_le_bytes());
        b[12..16].copy_from_slice(&(self.crc() as u32).to_le_bytes());
        b[DATA_OFFSET..].copy_from_slice(&self.data);
        b
    }

    /// Decode and verify the CRC.
    pub fn decode(b: &[u8]) -> Result<Self, DecodeError> {
        if b.len() < SIZE {
            return Err(DecodeError::Short {
                need: SIZE,
                have: b.len(),
            });
        }
        let w = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().expect("4 bytes"));
        let mut data = [0u8; DATA_SIZE];
        data.copy_from_slice(&b[DATA_OFFSET..SIZE]);
        let s = Self {
            cmd: w(0),
            func: w(4),
            cookie: w(8),
            data,
        };
        let stored = w(12);
        if stored != s.crc() as u32 {
            return Err(DecodeError::Crc {
                stored,
                computed: s.crc(),
            });
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// ProdConfig get wifiRegion, as built by the public reference clients:
    /// cmd 49, func 2, cookie 0x4842, field 13 at 0x14.
    fn prodconfig_get_wifi_region() -> Sutest {
        let mut s = Sutest::new(49, 2, 0x4842);
        s.put(0x14, &13u32.to_le_bytes()).unwrap();
        s
    }

    #[test]
    fn layout_matches_reference_clients() {
        let b = prodconfig_get_wifi_region().encode();
        assert_eq!(b.len(), 252);
        assert_eq!(&b[0..12], &[49, 0, 0, 0, 2, 0, 0, 0, 0x42, 0x48, 0, 0]);
        assert_eq!(&b[0x14..0x18], &[13, 0, 0, 0]);
        let crc = u32::from_le_bytes(b[12..16].try_into().unwrap());
        assert_eq!(crc, crc16_xmodem(&b[0x10..0xFC]) as u32);
        assert_eq!(crc >> 16, 0, "crc is a zero-extended u16");
    }

    #[test]
    fn shell_string_payload() {
        let mut s = Sutest::new(65, 1, 0x4842);
        s.put(0x14, b"id\0").unwrap();
        let b = s.encode();
        assert_eq!(&b[0x14..0x17], b"id\0");
    }

    #[test]
    fn rejects_bad_crc_and_short_input() {
        let mut b = prodconfig_get_wifi_region().encode();
        assert!(Sutest::decode(&b[..251]).is_err());
        b[0x20] ^= 1;
        assert!(matches!(Sutest::decode(&b), Err(DecodeError::Crc { .. })));
    }

    #[test]
    fn put_bounds() {
        let mut s = Sutest::new(0, 0, 0);
        assert!(
            s.put(0x0C, &[1]).is_err(),
            "header bytes are not writable through put"
        );
        assert!(s.put(0xFC - 3, &[1, 2, 3]).is_ok());
        assert!(s.put(0xFC - 2, &[1, 2, 3]).is_err());
        assert!(s.get(0xF8, 4).is_ok() && s.get(0xF9, 4).is_err());
    }

    proptest! {
        #[test]
        fn roundtrip(cmd: u32, func: u32, cookie: u32, data in proptest::collection::vec(any::<u8>(), DATA_SIZE)) {
            let mut s = Sutest::new(cmd, func, cookie);
            s.data.copy_from_slice(&data);
            prop_assert_eq!(Sutest::decode(&s.encode()).unwrap(), s);
        }

        #[test]
        fn decode_never_panics(b in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = Sutest::decode(&b);
        }
    }
}
