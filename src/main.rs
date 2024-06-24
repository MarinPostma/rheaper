use itertools::Itertools;
use rusqlite::{Connection, Transaction};
use std::{
    fs::{read, File},
    io::{stdout, BufRead, BufReader, Write},
    path::Path,
};

use hashbrown::HashMap;

#[derive(serde::Deserialize)]
enum AllocEvent {
    Alloc {
        seq: usize,
        bt: u128,
        after: std::time::Duration,
        size: usize,
        addr: usize,
    },
    Dealloc {
        seq: usize,
        after: std::time::Duration,
        addr: usize,
    },
}
impl AllocEvent {
    fn seq(&self) -> usize {
        match self {
            AllocEvent::Alloc { seq, .. } => *seq,
            AllocEvent::Dealloc { seq, .. } => *seq,
        }
    }
}

#[derive(serde::Deserialize)]
struct Entry {
    frames: Vec<String>,
    id: u128,
}

fn read_backtraces(path: impl AsRef<Path>, tx: &mut Transaction) {
    let path = path.as_ref().join("backtraces");
    let mut entries = HashMap::new();
    let paths = std::fs::read_dir(path).unwrap();
    for e in paths {
        let mut file = BufReader::new(File::open(e.unwrap().path()).unwrap());
        let mut line = String::new();
        while file.read_line(&mut line).unwrap() != 0 {
            let bt: Entry = serde_json::from_str(&line).unwrap();
            entries.insert(bt.id, bt.frames);
            line.clear();
        }
    }

    let mut s = tx
        .prepare("insert into backtraces values (?, ?, ?)")
        .unwrap();
    let mut count = 0;

    println!("analyzing backtraces:");
    for (id, frames) in entries {
        count += 1;
        print!("\r{count}          ");
        stdout().flush().unwrap();
        for (i, f) in frames.iter().enumerate() {
            s.execute((id.to_string(), i, f)).unwrap();
        }
    }

    println!();
}

struct EventsIter {
    reader: BufReader<File>,
    line: String,
}

impl EventsIter {
    fn new(path: &Path) -> Self {
        let reader = BufReader::new(File::open(path).unwrap());
        Self {
            reader,
            line: String::new(),
        }
    }
}

impl Iterator for EventsIter {
    type Item = AllocEvent;

    fn next(&mut self) -> Option<Self::Item> {
        if self.reader.read_line(&mut self.line).unwrap() == 0 {
            return None;
        }

        let e: AllocEvent = serde_json::from_str(&self.line).unwrap();
        self.line.clear();
        Some(e)
    }
}

fn read_events(path: impl AsRef<Path>, tx: &mut Transaction) {
    let path = path.as_ref().join("events");
    let event_paths = std::fs::read_dir(path).unwrap();
    let mut iters = Vec::new();
    for p in event_paths {
        iters.push(EventsIter::new(&p.unwrap().path()));
    }

    let mut allocs = HashMap::new();
    let mut alloc = tx
        .prepare("insert into allocations values (?, NULL, ?, ?, ?) RETURNING rowid")
        .unwrap();
    let mut dealloc = tx
        .prepare("update allocations set dealloc_after = ? where rowid = ?")
        .unwrap();
    let mut count = 0;
    println!("analyzing events:");
    for event in iters.into_iter().kmerge_by(|a, b| a.seq() < b.seq()) {
        count += 1;
        print!("\r{count}          ");
        stdout().flush().unwrap();
        match event {
            AllocEvent::Alloc {
                addr,
                size,
                bt,
                after,
                ..
            } => {
                let id = alloc
                    .query_row(
                        (after.as_millis() as u64, bt.to_string(), size, addr),
                        |r| Ok(r.get_unwrap::<_, i32>(0)),
                    )
                    .unwrap();
                allocs.insert(addr, id);
            }
            AllocEvent::Dealloc { addr, after, .. } => {
                if let Some(id) = allocs.remove(&addr) {
                    dealloc.execute((after.as_millis() as u64, id)).unwrap();
                }
            }
        }
    }

    println!();
}

fn main() {
    let profile_path = std::env::args().nth(1).unwrap();
    let out_path = std::env::args().nth(2).unwrap();

    let mut conn = Connection::open(out_path).unwrap();

    conn.query_row("pragma journal_mode=wal", (), |_| Ok(()))
        .unwrap();
    conn.execute("pragma synchronous=false", ()).unwrap();
    let mut tx = conn.transaction().unwrap();
    tx.execute("create table backtraces (id, frame_no, frame)", ())
        .unwrap();
    tx.execute(
        "create table allocations (alloc_after, dealloc_after, bt, size, addr)",
        (),
    )
    .unwrap();

    read_backtraces(&profile_path, &mut tx);
    read_events(&profile_path, &mut tx);

    tx.commit().unwrap();
}
