<div style="margin:0 auto;">
    <img src="./assets/logo.svg"/>
</div>

Rheaper is a in-process heap profiler for rust, that plugs in place and collect allocation data, and later stores it in a SQLite database for analysis. It supports runtime activation/deactivation.

To enable heaptrack in you project, set the global allocator:

```rust
#[global_allocator]
static GLOBAL: Rheaper::Allocator<mimalloc::MiMalloc> =
    rheaper::Allocator::from_allocator(mimalloc::MiMalloc);
```

at any point, tracking can be enabled:

```rust
let profile_path = rheaper::enable_tracking(Rheaper::TrackerConfig {
    /// how deep should the call stack be sampled
    max_stack_depth: 30,
    /// each thread that allocates acquire a local tracker from the global pool. That's how many trackers can be created
    max_trackers: 200,
    /// How many alloc events are buffered per local tracker
    tracker_event_buffer_size: 5_000,
    /// How often to sample backtraces, 1.0 is always, 0.0 is never.
    sample_rate: 1.0,
    /// where th profile will be written
    profile_dir: PathBuf::new(),
});
```

Recording is stopped with:

```rust
rheaper::disable_tracking();
```

This will resolve pending backtraces, and flush profiling data to disk. After than, the profile can be analyzed

```
rheaper rip-4744532 profile.db
```

this will create a `profile.db` sqlite database. This database can be opened with sqlite to perform analytics.

## Examples

show the 10 biggest contributor by callsites:
```SQL
SELECT bt, sum(size) FROM allocations GROUP BY bt ORDER BY sum(size) DESC LIMIT 10;

bt                    sum(size)
--------------------  ---------
14887427388493581654  61044456
6831336122721510729   52678176
2113490791341760087   52678176
15757739337697011469  31692912
3437328128932597320   24435936
15830995416840919940  10029120
7763277092675977093   9981504
10258202288105587101  9981504
12697966346495929209  3928320
4493127380101385995   3883680
```

show allocations, by callsites, that lives for more than 10 seconds:
```SQL
SELECT bt, count(0), sum(size) FROM allocations where dealloc_after NOT NULL AND dealloc_after - alloc_after < 10000 GROUP BY bt ORDER BY sum(size) DESC LIMIT 5;

bt                    count(0)  sum(size)
--------------------  --------  ---------
14887427388493581654  372       61044456
6831336122721510729   372       52678176
2113490791341760087   372       52678176
15757739337697011469  372       31692912
3437328128932597320   372       24435936
```

bt can be used to query the `backtraces table`:

```SQL
select frame_no, sym from backtraces where id = '14887427388493581654' limit 10;                

frame_no  sym
--------  ------------------------------------------------------------
0         backtrace::backtrace::libunwind::trace::h313fcf731c1ed43a

1         <rheaper::alloc::Allocator<A> as core::alloc::global::Glob
          alAlloc>::alloc::{{closure}}::hd945306712c35cdc

2         rheaper::alloc::with_local::{{closure}}::{{closure}}::{{cl
          osure}}::{{closure}}::h984dd2b94bca5c66

3         rheaper::alloc::TrackerGuard::with::h2f7bcd4120d6efa7

4         rheaper::alloc::with_local::{{closure}}::{{closure}}::{{cl
          osure}}::h51d6f70563022314

5         std::thread::local::LocalKey<T>::try_with::hff003d76feecb4a9

6         rheaper::alloc::with_local::{{closure}}::{{closure}}::h233
          b8e1d7f3eb3c5

7         rheaper::alloc::untracked::{{closure}}::hc0fb5ce1fd3deaf9

8         std::thread::local::LocalKey<T>::try_with::h8850e1a558f39df6

9         std::thread::local::LocalKey<T>::with::hcab5ce8347c6de7c
```
