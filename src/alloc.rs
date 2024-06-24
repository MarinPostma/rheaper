use std::alloc::GlobalAlloc;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::sync::atomic::{
    AtomicBool, AtomicUsize,
    Ordering::{Relaxed, SeqCst},
};
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use backtrace::trace_unsynchronized;
use crc::CRC_64_ECMA_182;

use crate::{local_tracker::LocalTracker, proto::AllocEvent};

static mut GLOBAL: Option<GlobalTracker> = None;
static ENABLING: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);
static GUARDS: AtomicUsize = AtomicUsize::new(0);

pub struct TrackerConfig {
    pub max_stack_depth: usize,
    pub max_trackers: usize,
    pub tracker_event_buffer_size: usize,
}

#[derive(Default)]
struct Trackers {
    trackers: Vec<Box<LocalTracker>>,
    /// indexes of the available tracked
    available_trackers: Vec<usize>,
}

impl Trackers {
    fn acquire_or_create(&mut self, path: &Path) -> TrackerGuard {
        if let Some(id) = self.available_trackers.pop() {
            let inner = unsafe {
                NonNull::new_unchecked(&mut *self.trackers[id])
            };
            TrackerGuard { inner, id }
        } else {
            let mut tracker = Box::new(LocalTracker::init(path));
            let inner = unsafe {
                NonNull::new_unchecked(&mut *tracker)
            };
            let id = self.trackers.len();
            self.trackers.push(tracker);
            TrackerGuard { inner, id }
        }
    }
}

struct GlobalTracker {
    started_at: Instant,
    seq: AtomicUsize,
    config: TrackerConfig,
    trackers: Mutex<Trackers>,
    profile_path: PathBuf,
}

struct TrackerGuard {
    inner: NonNull<LocalTracker>,
    id: usize,
}

impl TrackerGuard {
    fn with(&self, f: impl FnOnce(&mut LocalTracker)) {
        GUARDS.fetch_add(1, SeqCst);
        if !ENABLED.load(Relaxed) {
            GUARDS.fetch_sub(1, SeqCst);
            return;
        }

        unsafe {
            f(&mut *self.inner.as_ptr());
        }

        GUARDS.fetch_sub(1, SeqCst);
    }
}

impl Drop for TrackerGuard {
    fn drop(&mut self) {
        unsafe {
            if let Some(global) = GLOBAL.as_ref() {
                global
                    .trackers
                    .lock()
                    .unwrap()
                    .available_trackers
                    .push(self.id);
            }
        }
    }
}

impl GlobalTracker {
    fn acquire_tracker(&self) -> TrackerGuard {
        self.trackers
            .lock()
            .unwrap()
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

pub fn enable_tracking(config: TrackerConfig) {
    ENABLING
        .compare_exchange(false, true, SeqCst, SeqCst)
        .expect("already initializing");

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let path = format!("hip-{}", now.as_secs());
    let path = <str as AsRef<Path>>::as_ref(path.as_str());

    std::fs::create_dir_all(&path).unwrap();
    std::fs::create_dir_all(path.join("events")).unwrap();
    std::fs::create_dir_all(path.join("backtraces")).unwrap();

    let global = GlobalTracker {
        started_at: Instant::now(),
        config,
        trackers: Default::default(),
        profile_path: path.to_owned(),
        seq: AtomicUsize::new(0),
    };

    unsafe {
        GLOBAL = Some(global);
    }

    ENABLED
        .compare_exchange(false, true, SeqCst, SeqCst)
        .expect("already initialized");
}

pub fn disable_tracking() {
    ENABLED.store(false, SeqCst);
    while GUARDS.load(SeqCst) != 0 {
        std::thread::sleep(Duration::from_millis(5));
    }

    // now we have full ownership over GLOBAL again.
    unsafe {
        if let Some(global) = GLOBAL.as_ref() {
            println!("recorded {} allocation events", global.seq.load(Relaxed));
            let mut trackers = global.trackers.lock().unwrap();
            trackers.trackers.iter_mut().enumerate().for_each(|(i, t)| t.finalize(i));
        }
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
        if ENABLED.load(Relaxed) {
            let _ = DISABLE_TRACKING.try_with(|dis| {
                if !dis.load(Relaxed) {
                    untracked(|| {
                        let _ = TRACKER.try_with(|local| {
                            let global = unsafe { GLOBAL.as_ref().unwrap() };
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

unsafe impl<A: GlobalAlloc> GlobalAlloc for Allocator<A> {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let ptr = self.inner.alloc(layout);

        with_local(|local, global| {
            // TODO: make stack depth configurable
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
            let seq = global.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            local.events.push(AllocEvent::Alloc {
                seq,
                bt: id,
                after,
                size: layout.size(),
                addr: ptr as *mut usize as usize,
                thread_id: 0
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
            let seq = global.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
