//! CRC-16/XMODEM (poly 0x1021, init 0, no reflection, no final xor).

/// CRC-16/XMODEM of `data`.
pub fn crc16_xmodem(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_check_value() {
        // The catalogue check value for CRC-16/XMODEM.
        assert_eq!(crc16_xmodem(b"123456789"), 0x31C3);
    }

    #[test]
    fn empty_is_zero() {
        assert_eq!(crc16_xmodem(&[]), 0);
    }
}
