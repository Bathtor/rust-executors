// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

use super::*;
use crossbeam_deque::*;
use crossbeam_channel::*;
use std::sync::mpsc;
use std::sync::{Arc, Weak, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::thread;
use std::cell::RefCell;
use std::vec::Vec;
use time;
use rand::{ThreadRng, Rng};
use std::ops::Deref;
use std::fmt::{Debug, Formatter};
use std::collections::LinkedList;
use std::collections::BTreeMap;
use std::iter::{FromIterator, IntoIterator};

const CHECK_GLOBAL_INTERVAL: u64 = timeconstants::NS_PER_MS;
const MAX_WAIT_WORKER_MS: u64 = 10;
const MAX_WAIT_SHUTDOWN_MS: u64 = 5 * timeconstants::MS_PER_S;

thread_local!(
    static LOCAL_JOB_QUEUE: RefCell<Option<Deque<Job>>> = RefCell::new(Option::None)
);

#[derive(Clone)]
pub struct ThreadPool {
    core: Arc<Mutex<ThreadPoolCore>>,
    global_sender: Sender<Job>,
    threads: usize,
    shutdown: Arc<AtomicBool>,
}

impl ThreadPool {
    pub fn new(threads: usize) -> ThreadPool {
        assert!(threads > 0);
        let (tx, rx) = unbounded();
        let shutdown = Arc::new(AtomicBool::new(false));
        let core = ThreadPoolCore::new(tx.clone(), rx.clone(), shutdown.clone(), threads);
        let pool = ThreadPool {
            core: Arc::new(Mutex::new(core)),
            global_sender: tx.clone(),
            threads,
            shutdown,
        };
        {
            let mut guard = pool.core.lock().unwrap();
            for _ in 0..threads {
                guard.spawn_worker(pool.core.clone());
            }
        }
        pool
    }
}

impl !Sync for ThreadPool {}

impl Executor for ThreadPool {
    fn execute<F>(&self, job: F)
    where
        F: FnOnce() + Send + 'static,
    {
        // NOTE: This check costs about 150k schedulings/s in a 2 by 2 experiment over 20 runs.
        if !self.shutdown.load(Ordering::SeqCst) {
            LOCAL_JOB_QUEUE.with(|qo| {
                let msg = Job(Box::new(job));
                match *qo.borrow_mut() {
                    Some(ref q) => q.push(msg),
                    None => {
                        debug!("Scheduling on global pool.");
                        self.global_sender.send(msg).expect(
                            "Couldn't schedule job in queue!",
                        )
                    }
                }
            });
        } else {
            warn!("Ignoring job as pool is shutting down.")
        }
    }

    fn shutdown_async(&self) {
        let shutdown_pool = self.clone();
        // Need to make sure that pool is shut down before drop is called.
        thread::spawn(move || {
            shutdown_pool.shutdown().expect("Pool didn't shut down");
        });
    }

    fn shutdown(self) -> Result<(), String> {
        if !self.shutdown.compare_and_swap(
            false,
            true,
            Ordering::SeqCst,
        )
        {
            let latch = Arc::new(CountdownEvent::new(self.threads));
            debug!("Shutting down {} threads", self.threads);
            {
                let guard = self.core.lock().unwrap();
                for worker in guard.workers.values() {
                    worker
                        .control
                        .send(ControlMsg::Stop(latch.clone()))
                        .expect("Couldn't send stop msg to thread!");
                }
            }
            let remaining = latch.wait_timeout(Duration::from_millis(MAX_WAIT_SHUTDOWN_MS));
            if remaining == 0 {
                debug!("All threads shut down");
                Result::Ok(())
            } else {
                let msg = format!(
                    "Pool failed to shut down in time. {:?} threads remaining.",
                    remaining
                );
                Result::Err(String::from(msg))
            }
        } else {
            Result::Err(String::from("Pool is already shutting down!"))
        }
    }
}

//fn spawn_worker(core: Arc<Mutex<ThreadPoolCore>>) {
//    let id = {
//        let mut guard = core.lock().unwrap();
//        guard.new_worker_id()
//    };
//    let core2 = core.clone();
//    thread::spawn(move || {
//        let mut worker = ThreadPoolWorker::new(id, core2);
//        worker.run()
//    });
//}

struct WorkerEntry {
    control: mpsc::Sender<ControlMsg>,
    stealer: Option<JobStealer>,
}

struct ThreadPoolCore {
    _global_sender: Sender<Job>,
    global_receiver: Receiver<Job>,
    shutdown: Arc<AtomicBool>,
    _threads: usize,
    ids: u64,
    workers: BTreeMap<u64, WorkerEntry>,
}

impl ThreadPoolCore {
    fn new(
        global_sender: Sender<Job>,
        global_receiver: Receiver<Job>,
        shutdown: Arc<AtomicBool>,
        threads: usize,
    ) -> ThreadPoolCore {
        ThreadPoolCore {
            _global_sender: global_sender,
            global_receiver,
            shutdown,
            _threads: threads,
            ids: 0,
            workers: BTreeMap::new(),
        }
    }

    fn new_worker_id(&mut self) -> u64 {
        let id = self.ids;
        self.ids += 1;
        id
    }

    fn add_stealer(&mut self, stealer: JobStealer) {
        let id = stealer.id;
        match self.workers.get_mut(&id) {
            Some(worker) => {
                debug!("Registered stealer for #{}", id);
                worker.stealer = Some(stealer);
            }
            None => panic!("Adding stealer for non-existant worker!"),
        }
        self.send_stealers();
    }

    fn drop_worker(&mut self, id: u64) {
        debug!("Dropping worker #{}", id);
        self.workers.remove(&id).expect(
            "Key should have been present!",
        );
        self.send_stealers();
    }

    fn send_stealers(&mut self) {
        let stealers: Vec<JobStealer> = self.workers
            .values()
            .filter(|w| w.stealer.is_some())
            .map(|w| w.stealer.clone().unwrap())
            .collect();
        for (wid, worker) in self.workers.iter() {
            let l: LinkedList<JobStealer> =
                stealers.iter().filter(|s| s.id != *wid).cloned().collect();
            worker.control.send(ControlMsg::Stealers(l)).log_warn(
                "Could not send control message",
            );
        }
    }

    fn spawn_worker(&mut self, core: Arc<Mutex<ThreadPoolCore>>) {
        let id = self.new_worker_id();
        let (tx_control, rx_control) = mpsc::channel();
        let worker = WorkerEntry {
            control: tx_control,
            stealer: Option::None,
        };
        let recv = self.global_receiver.clone();
        self.workers.insert(id, worker);
        thread::spawn(move || {
            let mut worker = ThreadPoolWorker::new(id, recv, rx_control, core);
            worker.run()
        });
    }
}

impl Drop for ThreadPoolCore {
    fn drop(&mut self) {
        if !self.shutdown.load(Ordering::SeqCst) {
            warn!("Threadpools should be shut down before deallocating.");
            // the threads won't leak, as they'll get errors on their control queues next time they check
        }
    }
}

struct ThreadPoolWorker {
    id: u64,
    core: Weak<Mutex<ThreadPoolCore>>,
    global_recv: Receiver<Job>,
    control: mpsc::Receiver<ControlMsg>,
    stealers: Vec<JobStealer>,
    random: ThreadRng,
}

impl ThreadPoolWorker {
    fn new(
        id: u64,
        global_recv: Receiver<Job>,
        control: mpsc::Receiver<ControlMsg>,
        core: Arc<Mutex<ThreadPoolCore>>,
    ) -> ThreadPoolWorker {
        ThreadPoolWorker {
            id,
            core: Arc::downgrade(&core),
            global_recv,
            control,
            stealers: Vec::new(),
            random: rand::thread_rng(),
        }
    }
    fn id(&self) -> &u64 {
        &self.id
    }
    fn run(&mut self) {
        debug!("Worker {} starting", self.id());
        let local_stealer_raw = LOCAL_JOB_QUEUE.with(|q| {
            let dq = Deque::new();
            let stealer = dq.stealer();
            *q.borrow_mut() = Some(dq);
            stealer
        });
        let local_stealer = JobStealer::new(local_stealer_raw, self.id);
        self.register_stealer(local_stealer.clone());
        let sentinel = Sentinel::new(self.core.clone(), self.id);
        let max_wait = Duration::from_millis(MAX_WAIT_WORKER_MS);
        let mut last_global_check = time::precise_time_ns();
        'main: loop {
            // Try the local queue usually
            'local: while let Steal::Data(d) = local_stealer.steal() {
                let Job(f) = d;
                f.call_box();
                let now = time::precise_time_ns();
                let time_since_check = now - last_global_check;
                if (time_since_check > CHECK_GLOBAL_INTERVAL) {
                    break 'local;
                }
            }
            // drain the control queue
            'ctrl: loop {
                match self.control.try_recv() {
                    Ok(msg) => {
                        match msg {
                            ControlMsg::Stealers(l) => self.update_stealers(l),
                            ControlMsg::Stop(latch) => {
                                latch.decrement().expect("stop latch decrements");
                                break 'main;
                            }
                        }
                    }
                    Err(mpsc::TryRecvError::Empty) => break 'ctrl,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        debug!("Worker {} self-terminating.", self.id());
                        sentinel.cancel();
                        panic!("Threadpool wasn't shut down properly!");
                    }
                }

            }
            last_global_check = time::precise_time_ns();
            // sometimes try the global queue
            if let Ok(msg) = self.global_recv.try_recv() {
                let Job(f) = msg;
                f.call_box();
            } else {
                // try to steal something!
                'stealing: for stealer in self.stealers.iter() {
                    if let Steal::Data(d) = stealer.steal() {
                        let Job(f) = d;
                        f.call_box();
                        break 'stealing; // only steal once before checking locally again
                    }
                }
                // there wasn't anything to steal either...let's just wait for a bit
                if let Ok(msg) = self.global_recv.recv_timeout(max_wait) {
                    last_global_check = time::precise_time_ns();
                    let Job(f) = msg;
                    f.call_box();
                }
                // aaaaand starting over with 'local
            }
        }
        sentinel.cancel();
        self.unregister_stealer(local_stealer);
        LOCAL_JOB_QUEUE.with(|q| { *q.borrow_mut() = None; });
        debug!("Worker {} shutting down", self.id());
    }

    fn register_stealer(&mut self, stealer: JobStealer) {
        match self.core.upgrade() {
            Some(core) => {
                let mut guard = core.lock().unwrap();
                guard.add_stealer(stealer);
            }
            None => panic!("Core was already shut down!"),
        }
    }

    fn unregister_stealer(&mut self, _stealer: JobStealer) {
        match self.core.upgrade() {
            Some(core) => {
                let mut guard = core.lock().unwrap();
                guard.drop_worker(self.id);
            }
            None => panic!("Core was already shut down!"),
        }
    }

    fn update_stealers(&mut self, l: LinkedList<JobStealer>) {
        let mut v = Vec::from_iter(l.into_iter());
        self.random.shuffle(&mut v);
        self.stealers = v;
    }
}

trait FnBox {
    fn call_box(self: Box<Self>);
}

impl<F: FnOnce()> FnBox for F {
    fn call_box(self: Box<F>) {
        (*self)()
    }
}

#[derive(Clone)]
struct JobStealer {
    inner: Stealer<Job>,
    id: u64,
}

impl JobStealer {
    fn new(stealer: Stealer<Job>, id: u64) -> JobStealer {
        JobStealer { inner: stealer, id }
    }
}

impl PartialEq for JobStealer {
    fn eq(&self, other: &JobStealer) -> bool {
        self.id == other.id
    }
}

impl Eq for JobStealer {}

impl Deref for JobStealer {
    type Target = Stealer<Job>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Debug for JobStealer {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "JobStealer#{}(_)", self.id)
    }
}

struct Job(Box<FnBox + Send + 'static>);

enum ControlMsg {
    Stealers(LinkedList<JobStealer>),
    Stop(Arc<CountdownEvent>),
}


impl Debug for ControlMsg {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        match *self {
            ControlMsg::Stealers(ref l) => write!(f, "Stealers({:?})", l),
            ControlMsg::Stop(_) => write!(f, "Stop(_)"),
        }
    }
}

struct Sentinel {
    core: Weak<Mutex<ThreadPoolCore>>,
    id: u64,
    active: bool,
}

impl Sentinel {
    fn new(core: Weak<Mutex<ThreadPoolCore>>, id: u64) -> Sentinel {
        Sentinel {
            core,
            id,
            active: true,
        }
    }

    // Cancel and destroy this sentinel.
    fn cancel(mut self) {
        self.active = false;
    }
}

impl Drop for Sentinel {
    fn drop(&mut self) {
        if self.active {
            warn!("Active worker {} died! Restarting...", self.id);
            match self.core.upgrade() {
                Some(core) => {
                    let mut guard = core.lock().unwrap();
                    // cleanup
                    guard.drop_worker(self.id);
                    // restart
                    guard.spawn_worker(core.clone());
                    drop(guard);
                }
                None => warn!("Could not restart worker, as pool has been deallocated!"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate env_logger;

    use super::*;

    #[test]
    fn run_with_two_threads() {
        env_logger::init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(2);
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown();
    }

    #[test]
    fn amplify() {
        env_logger::init();

        let latch = Arc::new(CountdownEvent::new(4));
        let pool = ThreadPool::new(2);
        let pool2 = pool.clone();
        let pool3 = pool.clone();
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || {
            ignore(latch2.decrement());
            pool2.execute(move || ignore(latch2.decrement()));
        });
        pool.execute(move || {
            ignore(latch3.decrement());
            pool3.execute(move || ignore(latch3.decrement()));
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown();
    }

    #[test]
    fn keep_pool_size() {
        env_logger::init();

        let latch = Arc::new(CountdownEvent::new(2));
        let pool = ThreadPool::new(1);
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || panic!("test panic please ignore"));
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
        pool.shutdown();
    }

    #[test]
    fn shutdown_from_worker() {
        env_logger::init();

        let pool = ThreadPool::new(1);
        let pool2 = pool.clone();
        let latch = Arc::new(CountdownEvent::new(2));
        let latch2 = latch.clone();
        let latch3 = latch.clone();
        let stop_latch = Arc::new(CountdownEvent::new(1));
        let stop_latch2 = stop_latch.clone();
        pool.execute(move || ignore(latch2.decrement()));
        pool.execute(move || {
            pool2.shutdown_async();
            ignore(stop_latch2.decrement());
        });
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        pool.execute(move || ignore(latch3.decrement()));
        let res = latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    #[test]
    fn shutdown_external() {
        env_logger::init();

        let pool = ThreadPool::new(1);
        let pool2 = pool.clone();
        let run_latch = Arc::new(CountdownEvent::new(1));
        let stop_latch = Arc::new(CountdownEvent::new(1));
        let run_latch2 = run_latch.clone();
        let stop_latch2 = stop_latch.clone();
        pool.execute(move || ignore(run_latch2.decrement()));
        let res = run_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 0);
        pool.shutdown().expect("pool to shut down");
        pool2.execute(move || ignore(stop_latch2.decrement()));
        let res = stop_latch.wait_timeout(Duration::from_secs(1));
        assert_eq!(res, 1);
    }

    #[test]
    fn dealloc_on_handle_drop() {
        env_logger::init();

        let pool = ThreadPool::new(1);
        let core = Arc::downgrade(&pool.core);
        drop(pool);
        std::thread::sleep(std::time::Duration::from_secs(1));
        assert!(core.upgrade().is_none());
    }
}
