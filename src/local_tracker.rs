use std::path::{Path, PathBuf};
use std::io::BufWriter;
use std::fs::File;

use hashbrown::HashMap;

use crate::{alloc::untracked, id::Id, proto::AllocEvent};

pub(crate) struct LocalTracker {
    pub(crate) bts: HashMap<Id, backtrace::Backtrace>,
    pub(crate) events: Vec<AllocEvent>,
    pub(crate) file: BufWriter<std::fs::File>,
    pub(crate) path: PathBuf,
}

impl Drop for LocalTracker {
    fn drop(&mut self) {
        untracked(|| {
            self.finalize();
        });
    }
}

impl LocalTracker {
    pub fn maybe_flush(&mut self) {
        if self.events.len() > 10_000 {
            self.flush_all();
        }
    }

    fn flush_all(&mut self) {
        use std::io::Write as _;

        for item in self.events.drain(..) {
            serde_json::to_writer(&mut self.file, &item).unwrap();
            writeln!(&mut self.file, "").unwrap();
        }

        self.file.flush().unwrap();
    }

    pub fn finalize(&mut self) {
        self.flush_all();
        for bt in self.bts.values_mut() {
            bt.resolve();
        }

        let thread_id = std::thread::current().id();
        let path = self.path.join("backtraces").join(format!("bt-{thread_id:?}"));
        let mut sym_file = BufWriter::new(File::create(path).unwrap());
        for (id, bt) in self.bts.iter() {
            let mut frames = Vec::new();
            for frame in bt.frames() {
                if let Some(sym) = &frame.symbols().get(0) {
                    if sym.filename().is_some() && sym.lineno().is_some() && sym.name().is_some() {
                        frames.push(format!(
                            "{}:{} - {}",
                            sym.filename().unwrap().display(),
                            sym.lineno().unwrap(),
                            sym.name().unwrap()
                        ));
                        continue;
                    }
                }

                frames.push("<unknown>".to_string());
            }

            serde_json::to_writer(&mut sym_file, &super::proto::Backtrace { frames, id: id.0 }).unwrap();

            use std::io::Write as _;
            writeln!(&mut sym_file, "").unwrap();

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
