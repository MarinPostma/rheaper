use std::alloc::GlobalAlloc;
use std::cell::RefCell;
use std::cell::RefMut;
use std::path::{Path, PathBuf};
use std::sync::atomic::{
    AtomicBool, AtomicUsize,
    Ordering::{Relaxed, SeqCst},
};
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

use crate::{id::Id, local_tracker::LocalTracker, proto::AllocEvent};

static mut DATA: Option<TrackerData> = None;
static ENABLING: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static DISABLE_TRACKING: AtomicBool = AtomicBool::new(false);
    pub static TRACKER: RefCell<Option<LocalTracker>> = RefCell::new(None);
}

struct TrackerData {
    seq: AtomicUsize,
    started_at: Instant,
    path: PathBuf,
}

impl TrackerData {
    fn new() -> Self {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
        let path = format!("hip-{}", now.as_secs());
        let path = <str as AsRef<Path>>::as_ref(path.as_str());

        std::fs::create_dir_all(&path).unwrap();
        std::fs::create_dir_all(path.join("events")).unwrap();
        std::fs::create_dir_all(path.join("backtraces")).unwrap();

        Self {
            seq: Default::default(),
            started_at: Instant::now(),
            path: path.to_path_buf(),
        }
    }
}

pub struct Allocator<A> {
    inner: A,
}

pub fn enable_tracking() {
    ENABLING
        .compare_exchange(false, true, SeqCst, SeqCst)
        .expect("already initializing");
    let data = TrackerData::new();
    unsafe {
        DATA = Some(data);
    }

    ENABLED
        .compare_exchange(false, true, SeqCst, SeqCst)
        .expect("already initialized");
}

pub fn disable_tracking() {
    ENABLED.store(false, SeqCst);
    TRACKER.with(|tracker| {
        untracked(|| {
            if let Some(tracker) = &mut *tracker.borrow_mut() {
                tracker.finalize();
            }
        });
    })
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

fn get_or_init<'a>(local: &'a RefCell<Option<LocalTracker>>) -> RefMut<'a, LocalTracker> {
    let mut local = local.borrow_mut();
    if local.is_none() {
        unsafe {
            *local = Some(LocalTracker::init(&DATA.as_ref().unwrap().path));
        }
    }
    RefMut::map(local, |t| t.as_mut().unwrap())
}

unsafe impl<A: GlobalAlloc> GlobalAlloc for Allocator<A> {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        let ptr = self.inner.alloc(layout);
        if ENABLED.load(Relaxed) {
            DISABLE_TRACKING.with(|dis| {
                if !dis.load(Relaxed) {
                    untracked(|| {
                        TRACKER.with(|local| {
                            let data = unsafe { DATA.as_ref().unwrap() };
                            let mut local = get_or_init(local);

                            let bt = backtrace::Backtrace::new_unresolved();
                            let id = Id::new(&bt);
                            let after = data.started_at.elapsed();
                            let seq = data.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            local.events.push(AllocEvent::Alloc {
                                seq,
                                bt: id.0,
                                after,
                                size: layout.size(),
                                addr: ptr as *mut usize as usize,
                            });

                            if !local.bts.contains_key(&id) {
                                local.bts.insert(id, bt);
                            }

                            local.maybe_flush();
                        });
                    });
                }
            });
        }

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        self.inner.dealloc(ptr, layout);
        if ENABLED.load(Relaxed) {
            DISABLE_TRACKING.with(|dis| {
                if !dis.load(Relaxed) {
                    untracked(|| {
                        TRACKER.with(|local| {
                            let data = unsafe { DATA.as_ref().unwrap() };
                            let mut local = get_or_init(local);

                            let after = data.started_at.elapsed();
                            let seq = data.seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            local.events.push(AllocEvent::Dealloc {
                                seq,
                                after,
                                addr: ptr as *mut usize as usize,
                            });

                            local.maybe_flush();
                        });
                    });
                }
            });
        }
    }
}
