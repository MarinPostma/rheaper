use std::io::{self, Read, Write};
use std::path::PathBuf;
use zerocopy::{Immutable, IntoBytes, KnownLayout, TryFromBytes};
use zerocopy::little_endian::U64;

#[derive(PartialEq, Debug, KnownLayout, Immutable, IntoBytes, TryFromBytes, Clone, Copy)]
#[repr(u8)]
pub(crate) enum Event {
    Alloc {
        /// duration in ns
        after: U64,
        seq: U64,
        addr: U64,
        thread_id: U64,
        bt: U64,
        size: U64,
    } = 0,
    Dealloc {
        /// duration in ns
        after: U64,
        seq: U64,
        addr: U64,
        thread_id: U64,
        _pad: [u8; 16],
    } = 1,
}

impl Event {
    pub(crate) fn serialize<W: Write>(&self, mut writer: W) -> io::Result<()> {
        writer.write_all(self.as_bytes())?;
        Ok(())
    }

    pub(crate) fn deserialize_stream<'a, R: Read + 'a>(
        mut reader: R,
    ) -> impl Iterator<Item = io::Result<Event>> + 'a {
        let mut buffer = vec![0; size_of::<Event>() * 2048];
        let mut current = 0;
        let mut init = 0;

        std::iter::from_fn(move || {
            loop {
                if init == 0 {
                    while init != buffer.len() {
                        match reader.read(&mut buffer[init..]) {
                            Ok(0) if init == 0 => return None,
                            Ok(0) => break,
                            Ok(n) => {
                                init += n;
                            }
                            Err(e) => return Some(Err(e)),
                        }
                    }
                }

                if current < init {
                    match Event::try_read_from_prefix(&buffer[current..]) {
                        Ok((e, _)) => {
                            current += size_of::<Event>();
                            return Some(Ok(e))
                        },
                        Err(_) => return Some(Err(io::Error::new(io::ErrorKind::InvalidData, "invalid event"))),
                    }
                } else {
                    init = 0;
                    current = 0;
                }
            }
        })
    }

    pub(crate) fn seq(&self) -> u64 {
        match self {
            Event::Alloc { seq, .. } => seq.get(),
            Event::Dealloc { seq, .. } => seq.get(),
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Backtrace {
    pub frames: Vec<Option<Frame>>,
    pub id: u64,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Frame {
    pub(crate) file: Option<PathBuf>,
    pub(crate) lineno: Option<u32>,
    pub(crate) sym_name: Option<String>,
}

#[cfg(test)]
mod test {
    use std::{
        io::{BufReader, BufWriter, Seek},
        time::Instant,
    };

    use tempfile::tempfile;

    use super::*;

    #[test]
    fn serde() {
        let mut file = tempfile().unwrap();
        let mut writer = BufWriter::new(&mut file);

        let event1 = Event::Alloc {
            seq: 0,
            bt: 123,
            after: Instant::now().elapsed(),
            size: 1234,
            addr: 4938,
            thread_id: 0,
        };
        let event2 = Event::Dealloc {
            seq: 1,
            after: Instant::now().elapsed(),
            addr: 53433,
            thread_id: 0,
        };

        event1.serialize(&mut writer).unwrap();
        event2.serialize(&mut writer).unwrap();

        writer.flush().unwrap();
        drop(writer);

        file.seek(io::SeekFrom::Start(0)).unwrap();

        let mut reader = BufReader::new(&mut file);
        let mut iter = Event::deserialize_stream(&mut reader);

        assert_eq!(iter.next().unwrap().unwrap(), event1);
        assert_eq!(iter.next().unwrap().unwrap(), event2);
        assert!(iter.next().is_none());
    }
}
