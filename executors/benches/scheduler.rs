//! Benchmark implementation details of the theaded scheduler. These benches are
//! intended to be used as a form of regression testing and not as a general
//! purpose benchmark demonstrating real-world performance.

use executors::crossbeam_workstealing_pool;
use executors::futures_executor::*;
use executors::Executor;
use futures::channel::oneshot;

use bencher::{benchmark_group, benchmark_main, Bencher};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{mpsc, Arc};

fn spawn_many(b: &mut Bencher) {
    const NUM_SPAWN: usize = 10_000;

    let rt = rt();

    let (tx, rx) = mpsc::sync_channel(1);
    let rem = Arc::new(AtomicUsize::new(0));

    b.iter(|| {
        rem.store(NUM_SPAWN, Relaxed);

        //futures::executor::block_on(async {
        for _ in 0..NUM_SPAWN {
            let tx = tx.clone();
            let rem = rem.clone();

            rt.spawn(async move {
                if 1 == rem.fetch_sub(1, Relaxed) {
                    tx.send(()).unwrap();
                }
            });
        }

        let _ = rx.recv().unwrap();
        //});
    });

    rt.shutdown().expect("shutdown");
}

// fn yield_many(b: &mut Bencher) {
//     const NUM_YIELD: usize = 1_000;
//     const TASKS: usize = 200;

//     let rt = rt();

//     let (tx, rx) = mpsc::sync_channel(TASKS);

//     b.iter(move || {
//         for _ in 0..TASKS {
//             let tx = tx.clone();

//             rt.spawn(async move {
//                 for _ in 0..NUM_YIELD {
//                     tokio::task::yield_now().await;
//                 }

//                 tx.send(()).unwrap();
//             });
//         }

//         for _ in 0..TASKS {
//             let _ = rx.recv().unwrap();
//         }
//     });
// }

fn ping_pong(b: &mut Bencher) {
    const NUM_PINGS: usize = 1_000;

    let rt = rt();

    let (done_tx, done_rx) = mpsc::sync_channel(1000);
    let rem = Arc::new(AtomicUsize::new(0));

    b.iter(|| {
        let done_tx = done_tx.clone();
        let rem = rem.clone();
        rem.store(NUM_PINGS, Relaxed);

        futures::executor::block_on(async {
            let rt_new = rt.clone();
            rt.spawn(async move {
                for _ in 0..NUM_PINGS {
                    let rem = rem.clone();
                    let done_tx = done_tx.clone();
                    let rt_new2 = rt_new.clone();
                    rt_new.spawn(async move {
                        let (tx1, rx1) = oneshot::channel();
                        let (tx2, rx2) = oneshot::channel();

                        rt_new2.spawn(async move {
                            rx1.await.unwrap();
                            tx2.send(()).unwrap();
                        });

                        tx1.send(()).unwrap();
                        rx2.await.unwrap();

                        if 1 == rem.fetch_sub(1, Relaxed) {
                            done_tx.send(()).unwrap();
                        }
                    });
                }
            });

            done_rx.recv().unwrap();
        });
    });
    rt.shutdown().expect("shutdown");
}

fn chained_spawn(b: &mut Bencher) {
    const ITER: usize = 1_000;

    let rt = rt();

    fn iter(done_tx: mpsc::SyncSender<()>, n: usize) {
        if n == 0 {
            done_tx.send(()).unwrap();
        } else {
            let _ = crossbeam_workstealing_pool::execute_locally(move || {
                iter(done_tx, n - 1);
            });
        }
    }

    let (done_tx, done_rx) = mpsc::sync_channel(1000);
    let rt_inner = rt.clone();
    b.iter(move || {
        let done_tx = done_tx.clone();
        rt_inner.execute(move || {
            iter(done_tx, ITER);
        });

        done_rx.recv().unwrap();
    });
    rt.shutdown().expect("shutdown");
}

fn chained_spawn_no_local(b: &mut Bencher) {
    const ITER: usize = 1_000;

    let rt = rt();

    fn iter(rt: impl FuturesExecutor, done_tx: mpsc::SyncSender<()>, n: usize) {
        if n == 0 {
            done_tx.send(()).unwrap();
        } else {
            let rt_new = rt.clone();
            rt.execute(move || {
                iter(rt_new, done_tx, n - 1);
            });
        }
    }

    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let rt_inner = rt.clone();
    b.iter(move || {
        let done_tx = done_tx.clone();
        let rt_new = rt_inner.clone();
        rt_inner.execute(move || {
            iter(rt_new, done_tx, ITER);
        });

        done_rx.recv().unwrap();
    });
    rt.shutdown().expect("shutdown");
}

fn chained_spawn_async(b: &mut Bencher) {
    const ITER: usize = 1_000;

    let rt = rt();

    fn iter(rt: impl FuturesExecutor, done_tx: mpsc::SyncSender<()>, n: usize) {
        if n == 0 {
            done_tx.send(()).unwrap();
        } else {
            let rt_new = rt.clone();
            rt.spawn(async move {
                iter(rt_new, done_tx, n - 1);
            });
        }
    }

    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let rt_inner = rt.clone();
    b.iter(move || {
        let done_tx = done_tx.clone();
        //futures::executor::block_on(async {
        let rt_new = rt_inner.clone();
        rt_inner.spawn(async move {
            iter(rt_new, done_tx, ITER);
        });

        done_rx.recv().unwrap();
        //});
    });
    rt.shutdown().expect("shutdown");
}

fn rt() -> impl FuturesExecutor {
    crossbeam_workstealing_pool::small_pool(4)
}

benchmark_group!(scheduler, chained_spawn, chained_spawn_no_local, chained_spawn_async);

benchmark_main!(scheduler);
