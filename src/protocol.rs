// Wire fields are little-endian and independent of host struct layout.
pub const HEADER: usize = 16;
pub const PAGE: usize = 4096;
pub const MESSAGE: usize = HEADER + PAGE;
pub const INFO: u8 = 0;
pub const READ: u8 = 1;
pub const WRITE: u8 = 2;
pub const FLUSH: u8 = 3;
pub const STOP: u8 = 4;

pub struct Request {
    pub opcode: u8,
    pub tag: u32,
    pub lba: u64,
}

impl Request {
    pub fn decode(bytes: &[u8], block_size: usize) -> Option<Self> {
        let opcode = *bytes.first()?;
        let size = match opcode {
            WRITE => HEADER + block_size,
            INFO | READ | FLUSH | STOP => HEADER,
            _ => return None,
        };
        if bytes.len() != size || bytes[1..4] != [0; 3] {
            return None;
        }
        let lba = u64::from_le_bytes(bytes[8..16].try_into().ok()?);
        if !matches!(opcode, READ | WRITE) && lba != 0 {
            return None;
        }
        Some(Self {
            opcode,
            tag: u32::from_le_bytes(bytes[4..8].try_into().ok()?),
            lba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_lengths_reserved_bytes_and_lba() {
        let mut bytes = [0; MESSAGE];
        bytes[0] = WRITE;
        bytes[4..8].copy_from_slice(&23u32.to_le_bytes());
        bytes[8..16].copy_from_slice(&42u64.to_le_bytes());
        let request = Request::decode(&bytes, PAGE).unwrap();
        assert_eq!((request.opcode, request.tag, request.lba), (WRITE, 23, 42));
        assert!(Request::decode(&bytes[..MESSAGE - 1], PAGE).is_none());
        bytes[1] = 1;
        assert!(Request::decode(&bytes, PAGE).is_none());
        bytes[1] = 0;
        bytes[0] = FLUSH;
        assert!(Request::decode(&bytes[..HEADER], PAGE).is_none());
    }
}
