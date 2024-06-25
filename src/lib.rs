#[cfg(feature = "allocator")]
mod alloc;
#[cfg(feature = "allocator")]
mod local_tracker;
mod proto;

#[cfg(feature = "parse")]
mod parse;

#[cfg(feature = "allocator")]
pub use alloc::{Allocator, enable_tracking, disable_tracking, TrackerConfig};

#[doc(hidden)]
#[cfg(feature = "parse")]
pub use parse::parse_profile;
