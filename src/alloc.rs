use std::alloc::GlobalAlloc;
use std::cell::RefCell;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::atomic::{
    AtomicBool, AtomicUsize,
    Ordering::Relaxed,
};
use std::sync::{Arc, Weak};
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use backtrace::trace_unsynchronized;
use crc::CRC_64_ECMA_182;
use parking_lot::{Mutex, RwLock};

use crate::{local_tracker::LocalTracker, proto::AllocEvent};

/// The global tracker configuration. each thread that allocated acquire a LocalTracker from the
/// GlobalTracker tracker pool.
static GLOBAL: RwLock<Option<GlobalTracker>> = RwLock::new(None);

pub struct TrackerConfig {
    pub max_stack_depth: usize,
    pub max_trackers: usize,
    pub tracker_event_buffer_size: usize,
    pub sample_rate: f64,
    pub profile_dir: PathBuf,
}

#[derive(Default)]
struct TrackersPool {
    trackers: Vec<Arc<Mutex<LocalTracker>>>,
    /// indexes of the available tracked
    available_trackers: Vec<usize>,
}

impl TrackersPool {
    fn acquire_or_create(&mut self, path: &Path) -> TrackerGuard {
        if let Some(id) = self.available_trackers.pop() {
            TrackerGuard {
                inner: Arc::downgrade(&self.trackers[id]),
                id,
            }
        } else {
            let tracker = Arc::new(Mutex::new(LocalTracker::init(path)));
            let id = self.trackers.len();
            let inner = Arc::downgrade(&tracker);
            self.trackers.push(tracker);
            TrackerGuard { inner, id }
        }
    }
}

struct GlobalTracker {
    started_at: Instant,
    seq: AtomicUsize,
    config: TrackerConfig,
    pool: Mutex<TrackersPool>,
    profile_path: PathBuf,
}

struct TrackerGuard {
    inner: Weak<Mutex<LocalTracker>>,
    id: usize,
}

impl TrackerGuard {
    fn with(&self, f: impl FnOnce(&mut LocalTracker)) {
        if let Some(tracker) = self.inner.upgrade() {
            f(&mut *tracker.lock());
        }
    }
}

impl Drop for TrackerGuard {
    fn drop(&mut self) {
        let guard = GLOBAL.read();
        if let Some(global) = guard.as_ref() {
            global.pool.lock().available_trackers.push(self.id);
        }
    }
}

impl GlobalTracker {
    fn acquire_tracker(&self) -> TrackerGuard {
        self.pool
            .lock()
            .acquire_or_create(&self.profile_path)
    }
}

thread_local! {
    static DISABLE_TRACKING: AtomicBool = AtomicBool::new(false);
    pub static TRACKER: RefCell<Option<TrackerGuard>> = RefCell::new(None);
}

pub struct Allocator<A> {
    inner: A,
}

#[derive(Debug)]
pub enum Error {
    AlreadyEnabled,
}

impl Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AlreadyEnabled => f.write_str("tracking already enabled"),
        }
    }
}

/// Enable tracking, and return the raw profile data path
pub fn enable_tracking(config: TrackerConfig) -> Result<PathBuf, Error> {
    dbg!();
    let mut guard = GLOBAL.write();
    dbg!();
    if guard.is_some() {
        return Err(Error::AlreadyEnabled);
    }

    dbg!();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let path = format!("hip-{}", now.as_secs());
    let path = config.profile_dir.join(path);

    std::fs::create_dir_all(&path).unwrap();
    std::fs::create_dir_all(path.join("events")).unwrap();
    std::fs::create_dir_all(path.join("backtraces")).unwrap();

    let global = GlobalTracker {
        started_at: Instant::now(),
        config,
        pool: Default::default(),
        profile_path: path.to_owned(),
        seq: AtomicUsize::new(0),
    };

    *guard = Some(global);

    Ok(path.to_owned())
}

pub fn disable_tracking() {
    let mut guard = GLOBAL.write();
    if let Some(global) = guard.take() {
        println!("recorded {} allocation events", global.seq.load(Relaxed));
        let mut trackers = global.pool.lock();
        trackers
            .trackers
            .drain(..)
            .enumerate()
            .for_each(|(id, t)| {
                match Arc::try_unwrap(t) {
                    Ok(mut tracker) => {
                        tracker.get_mut().finalize(id);
                    }
                    Err(_arc) => {
                        panic!();
                    }
                }
            });
    }
}

pub(crate) fn untracked(f: impl FnOnce()) {
    DISABLE_TRACKING.with(|disable| {
        disable.store(true, Relaxed);
        f();
        disable.store(false, Relaxed);
    });
}

impl<A> Allocator<A> {
    pub const fn from_allocator(allocator: A) -> Self {
        Self { inner: allocator }
    }
}

fn with_local(f: impl FnOnce(&mut LocalTracker, &GlobalTracker)) {
    if let Some(guard) = GLOBAL.try_read() {
        if let Some(global) = guard.as_ref() {
            let _ = DISABLE_TRACKING.try_with(|dis| {
                if !dis.load(Relaxed) {
                    untracked(|| {
                        let _ = TRACKER.try_with(|local| {
                            let mut local = local.borrow_mut();
                            if local.is_none() {
                                *local = Some(global.acquire_tracker());
                            }

                            local.as_ref().unwrap().with(|l| f(l, &global))
                        });
                    });
                }
            });
        }
    }
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for Allocator<A> {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let ptr = self.inner.alloc(layout);

        with_local(|local, global| {
            if global.config.sample_rate != 1.0 {
                if rand::random::<f64>() > global.config.sample_rate {
                    return;
                }
            }

            let mut trace = Vec::with_capacity(global.config.max_stack_depth);

            let crc = crc::Crc::<u64>::new(&CRC_64_ECMA_182);
            let mut digest = crc.digest();

            trace_unsynchronized(|f| {
                let ip = f.ip() as *mut usize as usize;
                trace.push(ip);
                digest.update(&ip.to_be_bytes());
                trace.len() < global.config.max_stack_depth
            });

            let id = digest.finalize();

            let after = global.started_at.elapsed();
            let seq = global
                .seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            local.events.push(AllocEvent::Alloc {
                seq,
                bt: id,
                after,
                size: layout.size(),
                addr: ptr as *mut usize as usize,
                thread_id: 0,
            });

            local.bts.insert(id, trace);

            local.maybe_flush(global.config.tracker_event_buffer_size);
        });

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        self.inner.dealloc(ptr, layout);
        with_local(|local, global| {
            let after = global.started_at.elapsed();
            let seq = global
                .seq
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            local.events.push(AllocEvent::Dealloc {
                seq,
                after,
                addr: ptr as *mut usize as usize,
                thread_id: 0,
            });

            local.maybe_flush(global.config.tracker_event_buffer_size);
        });
    }
}
