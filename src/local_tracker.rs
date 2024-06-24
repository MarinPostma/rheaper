use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::io::BufWriter;
use std::fs::File;
use std::io::Write as _;

use backtrace::resolve;
use hashbrown::HashMap;

use crate::proto::{AllocEvent, Frame};

pub(crate) struct LocalTracker {
    pub(crate) bts: HashMap<u64, Vec<usize>>,
    pub(crate) events: Vec<AllocEvent>,
    pub(crate) file: BufWriter<std::fs::File>,
    pub(crate) path: PathBuf,
}

impl LocalTracker {
    pub fn maybe_flush(&mut self, threshold: usize) {
        if self.events.len() > threshold {
            self.flush_all();
        }
    }

    fn flush_all(&mut self) {
        use std::io::Write as _;

        for item in self.events.drain(..) {
            item.serialize(&mut self.file).unwrap();
        }

        self.file.flush().unwrap();
    }

    pub fn finalize(&mut self, id: usize) {
        self.flush_all();

        let path = self.path.join("backtraces").join(format!("bt-{id}"));
        let mut sym_file = BufWriter::new(File::create(path).unwrap());
        for (id, bt) in self.bts.iter() {
            let mut frames = Vec::new();
            for frame in bt {
                let mut called = false;
                resolve(*frame as *mut usize as *mut c_void, |sym| {
                    if !called {
                        called = true;
                        if sym.filename().is_some() && sym.lineno().is_some() && sym.name().is_some() {
                            frames.push(Some(Frame {
                                file: sym.filename().map(ToOwned::to_owned),
                                lineno: sym.lineno(),
                                sym_name: sym.name().map(|s| s.to_string()),
                            }));
                        }
                    }
                });

                if !called {
                    frames.push(None);
                }
            }

            serde_json::to_writer(&mut sym_file, &super::proto::Backtrace { frames, id: *id }).unwrap();

            writeln!(&mut sym_file).unwrap();

            sym_file.flush().unwrap();
        }
    }

    pub(crate) fn init(path: &Path) -> LocalTracker {
        let thread_id = std::thread::current().id();
        let file = BufWriter::new(
            File::create(path.join(format!("events/events-{thread_id:?}"))).unwrap(),
        );

        LocalTracker {
            bts: HashMap::new(),
            events: Vec::with_capacity(10_000),
            file,
            path: path.to_path_buf(),
        }
    }
}
