// Copyright 2017-2020 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! A reusable thread-pool-parking mechanism.
//!
//! This module is mostly meant for internal use within this crate,
//! but could be used to build custom thread-pools as well.

use arr_macro::arr;
use std::{
    collections::BTreeMap,
    fmt,
    sync::{
        atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering},
        Arc,
        Mutex,
    },
    thread,
    thread::Thread,
};

/// Get a parker for up to 32 threads
pub fn small() -> StaticParker<SmallThreadData> {
    StaticParker::new(SmallThreadData::new())
}

/// Get a parker for up to 64 threads
pub fn large() -> StaticParker<LargeThreadData> {
    StaticParker::new(LargeThreadData::new())
}

/// Get a parker for an unbounded number of threads
pub fn dynamic() -> StaticParker<DynamicThreadData> {
    StaticParker::new(DynamicThreadData::new())
}

/// Indicates the return condition from a park attempt.
#[derive(Clone, Debug, Copy)]
pub enum ParkResult {
    /// Simply retry parking later.
    /// Usually, indicates that implementation wanted avoid blocking on a lock.
    /// The parker was left in the "prepared" state (s. [prepare_park](Parker::prepare_park)).
    Retry,
    /// Recheck managed resource before retrying.
    /// The parker was moved out of the "prepared" state (s. [abort_park](Parker::abort_park)).
    Abort,
    /// Thread parked and then was awoken via `unpark`.
    /// The parker was moved out of the "prepared" state (s. [unpark_one](Parker::unpark_one) and [unpark_all](Parker::unpark_all)).
    Woken,
}

/// The core trait that every parker implementation must provide
pub trait Parker: Sync + Send + Clone {
    /// Maximum number of threads supported by this parker implementation.
    fn max_threads(&self) -> Option<usize>;

    /// Set thread state to `Awake`, no matter what the current state is.
    ///
    /// This is mostly meant for cleanup after a thread panic, where the state is unkown.
    fn init(&self, thread_id: usize) -> ();

    /// Prepare to go to sleep.
    /// You *must* call this before doing one last check on the resource being managed.
    /// If the resource is available afterwards, you *must* call [abort_park](Parker::abort_park).
    /// If it is not available afterwards, then call [park](Parker::park).
    ///
    /// The provided `thread_id` must fit into the underlying parker implementation, or a panic will be issued!
    ///
    fn prepare_park(&self, thread_id: usize) -> ();

    /// Abort attempt at going to sleep.
    /// You *must* call this if you have called [prepare_park](Parker::prepare_park), but then the resource became available.
    ///
    /// The provided `thread_id` must fit into the underlying parker implementation, or a panic will be issued!
    ///
    fn abort_park(&self, thread_id: usize) -> ();

    /// Parks this thread until it gets woken up.
    ///
    /// The provided `thread_id` must fit into the underlying parker implementation, or a panic will be issued!
    ///
    /// A parked thread may spuriously wake up or the method may return immediately without parking the thread at all.
    /// Parking is only guaranteed to happen if there is no contention on the inner lock.
    fn park(&self, thread_id: usize) -> ParkResult;

    /// Unparks at least one thread, if any threads are sleeping.
    /// It may unpark more than one thread for efficiency.
    fn unpark_one(&self) -> ();

    /// Unpark all sleeping threads.
    fn unpark_all(&self) -> ();
}

/// A trait-object-like wrapper around a concrete parker instance
#[derive(Clone, Debug)]
pub struct DynParker {
    inner: Arc<dyn ThreadData + Sync + Send>,
}
impl DynParker {
    fn new<T: 'static + ThreadData + Sync + Send>(data: T) -> DynParker {
        DynParker {
            inner: Arc::new(data),
        }
    }
}
impl Parker for DynParker {
    #[inline(always)]
    fn max_threads(&self) -> Option<usize> {
        self.inner.max_threads()
    }

    #[inline(always)]
    fn init(&self, thread_id: usize) -> () {
        self.inner.init(thread_id);
    }

    #[inline(always)]
    fn prepare_park(&self, thread_id: usize) -> () {
        self.inner.prepare_park(thread_id);
    }

    #[inline(always)]
    fn abort_park(&self, thread_id: usize) -> () {
        self.inner.abort_park(thread_id);
    }

    #[inline(always)]
    fn park(&self, thread_id: usize) -> ParkResult {
        self.inner.park(thread_id)
    }

    #[inline(always)]
    fn unpark_one(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_one();
        }
    }

    #[inline(always)]
    fn unpark_all(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_all();
        }
    }
}

/// A concrete parker instance
///
/// The generic parameter `T` implements the internal state management of the parker.
#[derive(Debug)]
pub struct StaticParker<T>
where
    T: ThreadData + Sync + Send + 'static,
{
    inner: Arc<T>,
}
impl<T> StaticParker<T>
where
    T: ThreadData + Sync + Send + 'static,
{
    fn new(data: T) -> StaticParker<T> {
        StaticParker {
            inner: Arc::new(data),
        }
    }

    /// Get a heap allocated version of this parker.
    ///
    /// Only works, if no handles to this parker were already shared and returns `Err` otherwise.
    pub fn dynamic(self) -> Result<DynParker, Self> {
        Arc::try_unwrap(self.inner)
            .map(DynParker::new)
            .map_err(|arc_data| StaticParker { inner: arc_data })
    }
}

impl<T> Clone for StaticParker<T>
where
    T: ThreadData + Sync + Send + 'static,
{
    fn clone(&self) -> Self {
        StaticParker {
            inner: self.inner.clone(),
        }
    }
}

impl<T> Parker for StaticParker<T>
where
    T: ThreadData + Send + Sync + 'static,
{
    #[inline(always)]
    fn max_threads(&self) -> Option<usize> {
        self.inner.max_threads()
    }

    #[inline(always)]
    fn init(&self, thread_id: usize) -> () {
        self.inner.init(thread_id);
    }

    #[inline(always)]
    fn prepare_park(&self, thread_id: usize) -> () {
        self.inner.prepare_park(thread_id);
    }

    #[inline(always)]
    fn abort_park(&self, thread_id: usize) -> () {
        self.inner.abort_park(thread_id);
    }

    #[inline(always)]
    fn park(&self, thread_id: usize) -> ParkResult {
        self.inner.park(thread_id)
    }

    #[inline(always)]
    fn unpark_one(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_one();
        }
    }

    #[inline(always)]
    fn unpark_all(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_all();
        }
    }
}

/// A trait that manages the internal state of a parker implementation
pub trait ThreadData: std::fmt::Debug {
    /// Maximum number of threads supported by this parker implementation.
    fn max_threads(&self) -> Option<usize>;

    /// Initialise thread at `thread_id`.
    fn init(&self, thread_id: usize) -> ();

    /// Prepare to go to sleep.
    fn prepare_park(&self, thread_id: usize) -> ();

    /// Abort attempt at going to sleep.
    fn abort_park(&self, thread_id: usize) -> ();

    /// Parks this thread until it gets woken up.
    fn park(&self, thread_id: usize) -> ParkResult;

    /// Return `true` if no threads are parked, `false` otherwise.
    fn all_awake(&self) -> bool;

    /// Unparks at least one thread, if any threads are sleeping.
    /// It may unpark more than one thread for efficiency.
    fn unpark_one(&self) -> ();

    /// Unpark all sleeping threads.
    fn unpark_all(&self) -> ();
}

#[derive(Debug)]
enum ParkState {
    Awake,
    Asleep(Thread),
    NoSleep,
    Waking,
}

/// Parker state large enough for up to 32 threads
#[derive(Debug)]
pub struct SmallThreadData {
    sleep_set: AtomicU32,
    sleeping: Mutex<[ParkState; 32]>,
}
impl SmallThreadData {
    /// The maximum number of threads supported by this parker
    pub const MAX_THREADS: usize = 32;

    fn new() -> SmallThreadData {
        SmallThreadData {
            sleep_set: AtomicU32::new(0),
            sleeping: Mutex::new(arr![ParkState::Awake; 32]),
        }
    }
}
impl ThreadData for SmallThreadData {
    fn max_threads(&self) -> Option<usize> {
        Some(SmallThreadData::MAX_THREADS)
    }

    fn init(&self, thread_id: usize) -> () {
        assert!(thread_id < 32);
        if let Ok(mut guard) = self.sleeping.lock() {
            guard[thread_id] = ParkState::Awake;
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn prepare_park(&self, thread_id: usize) -> () {
        assert!(thread_id < 32);
        self.sleep_set.set_at(thread_id);
    }

    fn abort_park(&self, thread_id: usize) -> () {
        assert!(thread_id < 32);
        self.sleep_set.unset_at(thread_id);
    }

    fn park(&self, thread_id: usize) -> ParkResult {
        assert!(thread_id < 32);
        if let Ok(mut guard) = self.sleeping.try_lock() {
            //self.sleep_set.set_at(thread_id);
            match guard[thread_id] {
                ParkState::Awake => {
                    guard[thread_id] = ParkState::Asleep(std::thread::current());
                }
                ParkState::Asleep(_) => unreachable!("Threads must clean up after waking up!"),
                ParkState::NoSleep => {
                    self.sleep_set.unset_at(thread_id);
                    guard[thread_id] = ParkState::Awake;
                    return ParkResult::Abort;
                }
                ParkState::Waking => unreachable!("Threads must clean up after waking up!"),
            }
        } else {
            return ParkResult::Retry; // if the lock can't be acquired, don't park, because maybe a client is trying to wake up things anyway
        }
        thread::park();
        if let Ok(mut guard) = self.sleeping.lock() {
            self.sleep_set.unset_at(thread_id);
            match guard[thread_id] {
                ParkState::Awake => unreachable!("Threads must be asleep to wake from park!"),
                ParkState::Waking | ParkState::Asleep(_) => {
                    guard[thread_id] = ParkState::Awake;
                    ParkResult::Woken
                }
                ParkState::NoSleep => {
                    unreachable!("Threads must be awake to be prevented from sleeping!");
                }
            }
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    #[inline(always)]
    fn all_awake(&self) -> bool {
        self.sleep_set.load(Ordering::SeqCst) == 0u32
    }

    fn unpark_one(&self) -> () {
        if let Ok(mut guard) = self.sleeping.lock() {
            if let Ok(index) = self.sleep_set.get_lowest() {
                match guard[index] {
                    // Thread is just about to go sleep, but I got lock first
                    ParkState::Awake => {
                        // Prevent the thread from going to sleep
                        guard[index] = ParkState::NoSleep;
                    }
                    ParkState::Asleep(ref t) => {
                        t.unpark();
                        guard[index] = ParkState::Waking;
                    }
                    // Another client already unparked this one or prevented it from going to sleep
                    ParkState::Waking | ParkState::NoSleep => {
                        // Try to find another thread to wake up
                        for index in 0..32 {
                            if let ParkState::Asleep(ref t) = guard[index] {
                                t.unpark();
                                guard[index] = ParkState::Waking;
                                return;
                            }
                        }
                    }
                }
            } // else just return, as nothing is sleeping or trying to go to sleep
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn unpark_all(&self) -> () {
        if let Ok(mut guard) = self.sleeping.lock() {
            if !self.all_awake() {
                for index in 0..32 {
                    match guard[index] {
                        ParkState::Awake => {
                            // Prevent all awake thread from going to sleep
                            guard[index] = ParkState::NoSleep;
                        }
                        ParkState::Asleep(ref t) => {
                            // Wake up all sleeping threads
                            t.unpark();
                            guard[index] = ParkState::Waking;
                        }
                        // Leave these as they are
                        ParkState::NoSleep | ParkState::Waking => (),
                    }
                }
            } // else just return, as nothing is sleeping or trying to go to sleep
        } else {
            panic!("Mutex is poisoned!");
        }
    }
}

/// Parker state for up to 64 threads
pub struct LargeThreadData {
    sleep_set: AtomicU64,
    sleeping: Mutex<[ParkState; 64]>,
}
impl LargeThreadData {
    /// The maximum number of threads supported by this parker
    pub const MAX_THREADS: usize = 64;

    fn new() -> LargeThreadData {
        LargeThreadData {
            sleep_set: AtomicU64::new(0),
            sleeping: Mutex::new(arr![ParkState::Awake; 64]),
        }
    }
}

// Can't implement Debug for [_; 64], so implement it for LargeThreadData instead.
impl fmt::Debug for LargeThreadData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sset = self.sleep_set.load(Ordering::SeqCst);
        write!(f, "LargeThreadData {{ sleep_set: {:032b}, sleeping: Mutex([...;64]) [array contents elided for brevity] }}", sset)
    }
}

impl ThreadData for LargeThreadData {
    fn max_threads(&self) -> Option<usize> {
        Some(LargeThreadData::MAX_THREADS)
    }

    fn init(&self, thread_id: usize) -> () {
        assert!(thread_id < 64);
        if let Ok(mut guard) = self.sleeping.lock() {
            guard[thread_id] = ParkState::Awake;
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn prepare_park(&self, thread_id: usize) -> () {
        assert!(thread_id < 64);
        self.sleep_set.set_at(thread_id);
    }

    fn abort_park(&self, thread_id: usize) -> () {
        assert!(thread_id < 64);
        self.sleep_set.unset_at(thread_id);
    }

    fn park(&self, thread_id: usize) -> ParkResult {
        assert!(thread_id < 64);
        if let Ok(mut guard) = self.sleeping.try_lock() {
            //self.sleep_set.set_at(thread_id);
            match guard[thread_id] {
                ParkState::Awake => {
                    guard[thread_id] = ParkState::Asleep(std::thread::current());
                }
                ParkState::Asleep(_) => unreachable!("Threads must clean up after waking up!"),
                ParkState::NoSleep => {
                    self.sleep_set.unset_at(thread_id);
                    guard[thread_id] = ParkState::Awake;
                    return ParkResult::Abort;
                }
                ParkState::Waking => unreachable!("Threads must clean up after waking up!"),
            }
        } else {
            return ParkResult::Retry; // if the lock can't be acquired, don't park, because maybe a client is trying to wake up things anyway
        }
        thread::park();
        if let Ok(mut guard) = self.sleeping.lock() {
            self.sleep_set.unset_at(thread_id);
            match guard[thread_id] {
                ParkState::Awake => unreachable!("Threads must be asleep to wake from park!"),
                ParkState::Waking | ParkState::Asleep(_) => {
                    guard[thread_id] = ParkState::Awake;
                    ParkResult::Woken
                }
                ParkState::NoSleep => {
                    unreachable!("Threads must be awake to be prevented from sleeping!");
                }
            }
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    #[inline(always)]
    fn all_awake(&self) -> bool {
        self.sleep_set.load(Ordering::SeqCst) == 0u64
    }

    fn unpark_one(&self) -> () {
        if let Ok(mut guard) = self.sleeping.lock() {
            if let Ok(index) = self.sleep_set.get_lowest() {
                match guard[index] {
                    // Thread is just about to go sleep, but I got lock first
                    ParkState::Awake => {
                        // Prevent the thread from going to sleep
                        guard[index] = ParkState::NoSleep;
                    }
                    ParkState::Asleep(ref t) => {
                        t.unpark();
                        guard[index] = ParkState::Waking;
                    }
                    // Another client already unparked this one or prevented it from going to sleep
                    ParkState::Waking | ParkState::NoSleep => {
                        // Try to find another thread to wake up
                        for index in 0..64 {
                            if let ParkState::Asleep(ref t) = guard[index] {
                                t.unpark();
                                guard[index] = ParkState::Waking;
                                return;
                            }
                        }
                    }
                }
            } // else just return, as nothing is sleeping or trying to go to sleep
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn unpark_all(&self) -> () {
        if let Ok(mut guard) = self.sleeping.lock() {
            if !self.all_awake() {
                for index in 0..64 {
                    match guard[index] {
                        ParkState::Awake => {
                            // Prevent all awake thread from going to sleep
                            guard[index] = ParkState::NoSleep;
                        }
                        ParkState::Asleep(ref t) => {
                            // Wake up all sleeping threads
                            t.unpark();
                            guard[index] = ParkState::Waking;
                        }
                        // Leave these as they are
                        ParkState::NoSleep | ParkState::Waking => (),
                    }
                }
            } // else just return, as no is sleeping or trying to sleep
        } else {
            panic!("Mutex is poisoned!");
        }
    }
}

#[derive(Debug)]
struct InnerData {
    sleeping: BTreeMap<usize, ParkState>,
    no_sleep: usize,
}
impl InnerData {
    fn new() -> InnerData {
        InnerData {
            sleeping: BTreeMap::new(),
            no_sleep: 0,
        }
    }
}

/// Park state for an unbounded number of threads
///
/// Uses a dynamic datastructure internally
/// to keep track of the state for each thread.
///
/// # Note
///
/// Technically, since thread-ids are `usize`,
/// there still is an upper bound for the maximum
/// number of threads in a pool, i.e. `usize::MAX`.
#[derive(Debug)]
pub struct DynamicThreadData {
    sleep_count: AtomicUsize,
    data: Mutex<InnerData>,
}
impl DynamicThreadData {
    fn new() -> DynamicThreadData {
        DynamicThreadData {
            sleep_count: AtomicUsize::new(0),
            data: Mutex::new(InnerData::new()),
        }
    }
}
impl ThreadData for DynamicThreadData {
    fn max_threads(&self) -> Option<usize> {
        None
    }

    fn init(&self, thread_id: usize) -> () {
        if let Ok(mut guard) = self.data.lock() {
            // Doesn't matter if it was there or not
            let _ = guard.sleeping.remove(&thread_id);
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn prepare_park(&self, _thread_id: usize) -> () {
        //println!("Preparing to park {}", _thread_id);
        self.sleep_count.fetch_add(1usize, Ordering::SeqCst);
    }

    fn abort_park(&self, _thread_id: usize) -> () {
        //println!("Aborting park {}", _thread_id);
        self.sleep_count.fetch_sub(1usize, Ordering::SeqCst);
    }

    fn park(&self, thread_id: usize) -> ParkResult {
        if let Ok(mut guard) = self.data.try_lock() {
            if guard.no_sleep == 0 {
                let old = guard
                    .sleeping
                    .insert(thread_id, ParkState::Asleep(std::thread::current()));
                if old.is_some() {
                    unreachable!("Inconsistent sleeping map (before park)!");
                }
            } else {
                //println!("Aborting park due to no_sleep {}", thread_id);
                guard.no_sleep -= 1; // must be >0 because we hold the lock
                self.sleep_count.fetch_sub(1usize, Ordering::SeqCst);
                return ParkResult::Abort;
            }
        } else {
            return ParkResult::Retry; // if the lock can't be acquired, don't park, because maybe a client is trying to wake up things anyway
        }
        //println!("Parking {}", thread_id);
        thread::park();
        //println!("Not parking {} anymore, but waiting for lock.", thread_id);
        if let Ok(mut guard) = self.data.lock() {
            //println!("Got lock.");
            self.sleep_count.fetch_sub(1usize, Ordering::SeqCst);
            guard
                .sleeping
                .remove(&thread_id)
                .expect("Inconsistent sleeping map (after park)!");
            //println!("Woke {}", thread_id);
            ParkResult::Woken
        } else {
            //eprintln!("Fuck, I'll panic!");
            panic!("Mutex is poisoned!");
        }
    }

    #[inline(always)]
    fn all_awake(&self) -> bool {
        self.sleep_count.load(Ordering::SeqCst) == 0usize
    }

    fn unpark_one(&self) -> () {
        if let Ok(mut guard) = self.data.lock() {
            if self.sleep_count.load(Ordering::SeqCst) > 0usize {
                for state in guard.sleeping.values_mut() {
                    //println!("Hanging on to the lock (one) {}...", count);
                    match state {
                        ParkState::Asleep(t) => {
                            //println!("Unparking {:?}", t);
                            t.unpark();
                            *state = ParkState::Waking;
                            return;
                        }
                        ParkState::Waking => (), // skip
                        ParkState::Awake | ParkState::NoSleep => {
                            unreachable!("These should not be in the map at all!");
                        }
                    }
                }
                //println!("Preventing a thread from going to sleep.");
                // no one to wake -> prevent next thread from going to sleep
                guard.no_sleep = guard.no_sleep.saturating_add(1); // don't overflow this during shutdown
            } // no one is sleeping, nothing to do
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn unpark_all(&self) -> () {
        if let Ok(mut guard) = self.data.lock() {
            if self.sleep_count.load(Ordering::SeqCst) > 0usize {
                for state in guard.sleeping.values_mut() {
                    //println!("Hanging on to the lock (all)...");
                    match state {
                        ParkState::Asleep(t) => {
                            t.unpark();
                            *state = ParkState::Waking;
                        }
                        ParkState::Waking => (), // skip
                        ParkState::Awake | ParkState::NoSleep => {
                            unreachable!("These should not be in the map at all!");
                        }
                    }
                }
                guard.no_sleep = std::usize::MAX; // with no concrete idea of who is going to sleep, this is the only way we can prevent them from doing so -.-
            } // no one is sleeping, nothing to do
        } else {
            panic!("Mutex is poisoned!");
        }
    }
}

#[derive(Clone, Debug, Copy, PartialEq, Eq)]
struct BitSetEmpty;
const BIT_SET_EMPTY: BitSetEmpty = BitSetEmpty {};

trait AtomicBitSet {
    fn set_at(&self, at: usize) -> ();
    fn unset_at(&self, at: usize) -> ();
    fn get_lowest(&self) -> Result<usize, BitSetEmpty>;
}

impl AtomicBitSet for AtomicU32 {
    fn set_at(&self, at: usize) -> () {
        assert!(at < 32);
        let mask = 1u32 << at;
        self.fetch_or(mask, Ordering::SeqCst);
    }

    fn unset_at(&self, at: usize) -> () {
        assert!(at < 32);
        let mask = !(1u32 << at);
        self.fetch_and(mask, Ordering::SeqCst);
    }

    fn get_lowest(&self) -> Result<usize, BitSetEmpty> {
        let cur = self.load(Ordering::SeqCst);
        if (cur == 0u32) {
            Err(BIT_SET_EMPTY)
        } else {
            let mut mask = 1u32;
            let mut index = 0;
            while index < 32 {
                if (mask & cur) != 0u32 {
                    return Ok(index);
                } else {
                    index += 1;
                    mask <<= 1;
                }
            }
            unreachable!("Bitset was empty despite empty check!");
        }
    }
}

impl AtomicBitSet for AtomicU64 {
    fn set_at(&self, at: usize) -> () {
        assert!(at < 64);
        let mask = 1u64 << at;
        self.fetch_or(mask, Ordering::SeqCst);
    }

    fn unset_at(&self, at: usize) -> () {
        assert!(at < 64);
        let mask = !(1u64 << at);
        self.fetch_and(mask, Ordering::SeqCst);
    }

    fn get_lowest(&self) -> Result<usize, BitSetEmpty> {
        let cur = self.load(Ordering::SeqCst);
        if (cur == 0u64) {
            Err(BIT_SET_EMPTY)
        } else {
            let mut mask = 1u64;
            let mut index = 0;
            while index < 64 {
                if (mask & cur) != 0u64 {
                    return Ok(index);
                } else {
                    index += 1;
                    mask <<= 1;
                }
            }
            unreachable!("Bitset was empty despite empty check!");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parker_printing() {
        {
            let p = small();
            println!("Small Parker: {:?}", p);
            let dyn_p = p.dynamic();
            println!("Small Parker (Dynamic): {:?}", dyn_p);
        }
        {
            let p = large();
            println!("Large Parker: {:?}", p);
            let dyn_p = p.dynamic();
            println!("Large Parker (Dynamic): {:?}", dyn_p);
        }
        {
            let p = dynamic();
            println!("Dynamic Parker: {:?}", p);
            let dyn_p = p.dynamic();
            println!("Dynamic Parker (Dynamic): {:?}", dyn_p);
        }
    }

    #[allow(clippy::unnecessary_wraps)]
    fn res_ok(v: usize) -> Result<usize, BitSetEmpty> {
        Ok(v)
    }
    fn res_err() -> Result<usize, BitSetEmpty> {
        Err(BIT_SET_EMPTY)
    }

    #[test]
    fn test_bitset() {
        let data = AtomicU32::new(0);
        let bs: &dyn AtomicBitSet = &data;
        assert_eq!(res_err(), bs.get_lowest());
        bs.set_at(1);
        assert_eq!(res_ok(1), bs.get_lowest());
        bs.set_at(5);
        assert_eq!(res_ok(1), bs.get_lowest());
        bs.unset_at(1);
        assert_eq!(res_ok(5), bs.get_lowest());
        bs.unset_at(5);
        assert_eq!(res_err(), bs.get_lowest());
    }
}
