use crc::CRC_82_DARC;

#[derive(Copy, Clone, Hash, PartialEq, Eq)]
pub(crate) struct Id(pub u128);

impl Id {
    pub(crate) fn new(bt: &backtrace::Backtrace) -> Self {
        let crc = crc::Crc::<u128>::new(&CRC_82_DARC);
        let mut digest = crc.digest();
        for f in bt.frames() {
            let addr = f.symbol_address() as *mut usize as usize;
            digest.update(&addr.to_be_bytes());
        }

        Self(digest.finalize())
    }
}
