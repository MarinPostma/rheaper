use std::io::{self, ErrorKind, Read, Write};
use std::mem::{size_of, MaybeUninit};
use std::path::PathBuf;
use std::time::Duration;

#[derive(PartialEq, Debug)]
#[repr(C)]
pub(crate) enum AllocEvent {
    Alloc {
        seq: usize,
        bt: u64,
        after: Duration,
        size: usize,
        addr: usize,
        thread_id: usize,
    },
    Dealloc {
        seq: usize,
        after: Duration,
        addr: usize,
        thread_id: usize,
    },
}

impl AllocEvent {
    // ain't no faster serialization than straight out struct bytes
    pub(crate) fn serialize<W: Write>(&self, mut writer: W) -> io::Result<()> {
        let bytes: &[u8; size_of::<Self>()] = unsafe { std::mem::transmute(self) };
        writer.write_all(bytes)?;
        Ok(())
    }

    pub(crate) fn deserialize_stream<'a, R: Read + 'a>(
        mut reader: R,
    ) -> impl Iterator<Item = io::Result<AllocEvent>> + 'a {
        std::iter::from_fn(move || unsafe {
            let mut buffer: MaybeUninit<AllocEvent> = MaybeUninit::uninit();
            let slice =
                std::slice::from_raw_parts_mut(buffer.as_mut_ptr() as *mut u8, size_of::<Self>());
            match reader.read_exact(slice) {
                Ok(()) => Some(Ok(buffer.assume_init())),
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => None,
                Err(e) => Some(Err(e)),
            }
        })
    }

    pub(crate) fn seq(&self) -> usize {
        match self {
            AllocEvent::Alloc { seq, .. } => *seq,
            AllocEvent::Dealloc { seq, .. } => *seq,
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

        let event1 = AllocEvent::Alloc {
            seq: 0,
            bt: 123,
            after: Instant::now().elapsed(),
            size: 1234,
            addr: 4938,
            thread_id: 0,
        };
        let event2 = AllocEvent::Dealloc {
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
        let mut iter = AllocEvent::deserialize_stream(&mut reader);

        assert_eq!(iter.next().unwrap().unwrap(), event1);
        assert_eq!(iter.next().unwrap().unwrap(), event2);
        assert!(iter.next().is_none());
    }
}
