// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

use arr_macro::arr;
use std::collections::BTreeMap;
use std::fmt;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::Thread;

pub fn small() -> StaticParker<SmallThreadData> {
    StaticParker::new(SmallThreadData::new())
}
pub fn large() -> StaticParker<LargeThreadData> {
    StaticParker::new(LargeThreadData::new())
}
pub fn dynamic() -> StaticParker<DynamicThreadData> {
    StaticParker::new(DynamicThreadData::new())
}

pub trait Parker: Send + Clone {
    /// Maximum number of threads supported by this parker implementation.
    fn max_threads(&self) -> Option<usize>;

    /// Parks this thread until it gets woken up.
    ///
    /// The provided id must fit into the underlying parker implementation, or a panic will be issued!
    ///
    /// A parked thread may spuriously wake up or the method may return immediately without parking the thread at all.
    /// Parking is only guaranteed to happen if there is no contention on the inner lock.
    fn park(&self, thread_id: usize) -> ();

    /// Unparks at least one thread, if any threads are sleeping.
    /// It may unpark more than one thread for efficiency.
    fn unpark_one(&self) -> ();

    /// Unpark all sleeping threads.
    fn unpark_all(&self) -> ();
}

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
    fn max_threads(&self) -> Option<usize> {
        self.inner.max_threads()
    }

    fn park(&self, thread_id: usize) -> () {
        self.inner.park(thread_id);
    }

    fn unpark_one(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_one();
        }
    }

    fn unpark_all(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_all();
        }
    }
}

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
            .map(|data| DynParker::new(data))
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
    fn max_threads(&self) -> Option<usize> {
        self.inner.max_threads()
    }

    fn park(&self, thread_id: usize) -> () {
        self.inner.park(thread_id);
    }

    fn unpark_one(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_one();
        }
    }

    fn unpark_all(&self) -> () {
        if (!self.inner.all_awake()) {
            self.inner.unpark_all();
        }
    }
}

pub trait ThreadData: std::fmt::Debug {
    /// Maximum number of threads supported by this parker implementation.
    fn max_threads(&self) -> Option<usize>;

    /// Parks this thread until it gets woken up.
    fn park(&self, thread_id: usize) -> ();

    /// Return `true` if no threads are parked, `false` otherwise.
    fn all_awake(&self) -> bool;

    /// Unparks at least one thread, if any threads are sleeping.
    /// It may unpark more than one thread for efficiency.
    fn unpark_one(&self) -> ();

    /// Unpark all sleeping threads.
    fn unpark_all(&self) -> ();
}

#[derive(Debug)]
pub struct SmallThreadData {
    sleep_set: AtomicU32,
    sleeping: Mutex<[Option<Thread>; 32]>,
}
impl SmallThreadData {
    pub const MAX_THREADS: usize = 32;

    fn new() -> SmallThreadData {
        SmallThreadData {
            sleep_set: AtomicU32::new(0),
            sleeping: Mutex::new(arr![None; 32]),
        }
    }
}
impl ThreadData for SmallThreadData {
    fn max_threads(&self) -> Option<usize> {
        Some(SmallThreadData::MAX_THREADS)
    }

    fn park(&self, thread_id: usize) -> () {
        assert!(thread_id < 32);
        if let Ok(mut guard) = self.sleeping.try_lock() {
            self.sleep_set.set_at(thread_id);
            guard[thread_id] = Some(std::thread::current());
        } else {
            return; // if the lock can't be acquired, don't park, because maybe a client is trying to wake up things anyway
        }
        thread::park();
        if let Ok(mut guard) = self.sleeping.lock() {
            self.sleep_set.unset_at(thread_id);
            guard[thread_id] = None;
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    #[inline(always)]
    fn all_awake(&self) -> bool {
        self.sleep_set.load(Ordering::SeqCst) == 0u32
    }

    fn unpark_one(&self) -> () {
        if let Ok(guard) = self.sleeping.lock() {
            if let Ok(index) = self.sleep_set.get_lowest() {
                match guard[index] {
                    Some(ref t) => t.unpark(),
                    None => panic!("Inconsistent sleep_set!"),
                }
            } // else just return, as nothing is sleeping
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn unpark_all(&self) -> () {
        if let Ok(guard) = self.sleeping.lock() {
            for index in 0..32 {
                if let Some(ref t) = guard[index] {
                    t.unpark();
                }
            }
        } else {
            panic!("Mutex is poisoned!");
        }
    }
}

pub struct LargeThreadData {
    sleep_set: AtomicU64,
    sleeping: Mutex<[Option<Thread>; 64]>,
}
impl LargeThreadData {
    pub const MAX_THREADS: usize = 64;

    fn new() -> LargeThreadData {
        LargeThreadData {
            sleep_set: AtomicU64::new(0),
            sleeping: Mutex::new(arr![None; 64]),
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

    fn park(&self, thread_id: usize) -> () {
        assert!(thread_id < 64);
        if let Ok(mut guard) = self.sleeping.try_lock() {
            self.sleep_set.set_at(thread_id);
            guard[thread_id] = Some(std::thread::current());
        } else {
            return; // if the lock can't be acquired, don't park, because maybe a client is trying to wake up things anyway
        }
        thread::park();
        if let Ok(mut guard) = self.sleeping.lock() {
            self.sleep_set.unset_at(thread_id);
            guard[thread_id] = None;
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    #[inline(always)]
    fn all_awake(&self) -> bool {
        self.sleep_set.load(Ordering::SeqCst) == 0u64
    }

    fn unpark_one(&self) -> () {
        if let Ok(guard) = self.sleeping.lock() {
            if let Ok(index) = self.sleep_set.get_lowest() {
                match guard[index] {
                    Some(ref t) => t.unpark(),
                    None => panic!("Inconsistent sleep_set!"),
                }
            } // else just return, as nothing is sleeping
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn unpark_all(&self) -> () {
        if let Ok(guard) = self.sleeping.lock() {
            for index in 0..64 {
                if let Some(ref t) = guard[index] {
                    t.unpark();
                }
            }
        } else {
            panic!("Mutex is poisoned!");
        }
    }
}

#[derive(Debug)]
pub struct DynamicThreadData {
    sleep_count: AtomicUsize,
    sleeping: Mutex<BTreeMap<usize, Thread>>,
}
impl DynamicThreadData {
    fn new() -> DynamicThreadData {
        DynamicThreadData {
            sleep_count: AtomicUsize::new(0),
            sleeping: Mutex::new(BTreeMap::new()),
        }
    }
}
impl ThreadData for DynamicThreadData {
    fn max_threads(&self) -> Option<usize> {
        None
    }

    fn park(&self, thread_id: usize) -> () {
        if let Ok(mut guard) = self.sleeping.try_lock() {
            self.sleep_count.fetch_add(1usize, Ordering::SeqCst);
            let _ = guard.insert(thread_id, std::thread::current());
        } else {
            return; // if the lock can't be acquired, don't park, because maybe a client is trying to wake up things anyway
        }
        thread::park();
        if let Ok(mut guard) = self.sleeping.lock() {
            self.sleep_count.fetch_sub(1usize, Ordering::SeqCst);
            guard
                .remove(&thread_id)
                .expect("Inconsistent sleeping map!");
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    #[inline(always)]
    fn all_awake(&self) -> bool {
        self.sleep_count.load(Ordering::SeqCst) == 0usize
    }

    fn unpark_one(&self) -> () {
        if let Ok(guard) = self.sleeping.lock() {
            if let Some((_, ref t)) = guard.iter().next() {
                t.unpark();
            } // else no one to wake
        } else {
            panic!("Mutex is poisoned!");
        }
    }

    fn unpark_all(&self) -> () {
        if let Ok(guard) = self.sleeping.lock() {
            for e in guard.iter() {
                let (_, ref t) = e;
                t.unpark();
            }
        } else {
            panic!("Mutex is poisoned!");
        }
    }
}

#[derive(Clone, Debug, Copy)]
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
