use std::fs::File;
use std::io::{stdout, BufRead, BufReader, Write};
use std::path::Path;

use hashbrown::HashMap;
use itertools::Itertools;
use rusqlite::{Connection, Transaction};

use crate::proto::{AllocEvent, Backtrace};

pub fn parse_profile(profile_path: impl AsRef<Path>, db_path: impl AsRef<Path>) {
    let mut conn = Connection::open(db_path).unwrap();

    conn.query_row("pragma journal_mode=wal", (), |_| Ok(()))
        .unwrap();
    conn.execute("pragma synchronous=false", ()).unwrap();
    let mut tx = conn.transaction().unwrap();
    tx.execute(
        "create table backtraces (id, frame_no, file, line, sym)",
        (),
    )
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

fn read_backtraces(path: impl AsRef<Path>, tx: &mut Transaction) {
    let path = path.as_ref().join("backtraces");
    let mut entries = HashMap::new();
    let paths = std::fs::read_dir(path).unwrap();
    for e in paths {
        let mut file = BufReader::new(File::open(e.unwrap().path()).unwrap());
        let mut line = String::new();
        while file.read_line(&mut line).unwrap() != 0 {
            let bt: Backtrace = serde_json::from_str(&line).unwrap();
            entries.insert(bt.id.to_string(), bt.frames);
            line.clear();
        }
    }

    let mut s = tx
        .prepare("insert into backtraces values (?, ?, ?, ?, ?)")
        .unwrap();
    let mut count = 0;

    println!("analyzing backtraces:");
    for (id, frames) in entries {
        count += 1;
        print!("\r{count}          ");
        stdout().flush().unwrap();
        for (i, f) in frames.iter().enumerate() {
            s.execute((
                id.to_string(),
                i,
                f.as_ref()
                    .and_then(|f| f.file.as_ref())
                    .and_then(|f| f.to_str()),
                f.as_ref().and_then(|f| f.lineno),
                f.as_ref().and_then(|f| f.sym_name.as_ref()),
            ))
            .unwrap();
        }
    }

    println!();
}

fn read_events(path: impl AsRef<Path>, tx: &mut Transaction) {
    let path = path.as_ref().join("events");
    let event_paths = std::fs::read_dir(path).unwrap();
    let mut iters = Vec::new();
    for p in event_paths {
        let reader = BufReader::new(File::open(p.unwrap().path()).unwrap());
        iters.push(AllocEvent::deserialize_stream(reader).map(|e| e.unwrap()));
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
