//! Benchmark implementation details of the theaded scheduler. These benches are
//! intended to be used as a form of regression testing and not as a general
//! purpose benchmark demonstrating real-world performance.

use executors::futures_executor::*;

use criterion::{criterion_group, criterion_main, Criterion, Throughput};

const CHAINING_DEPTH: usize = 1000;
const SPAWN_NUM: usize = 1000;
const PINGS_NUM: usize = 1000;

pub fn chained_spawn_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Chained Spawn");
    group.throughput(Throughput::Elements(CHAINING_DEPTH as u64));
    // CBWP
    group.bench_function("Local Function CBWP", chained_spawn::local_function_cbwp);
    group.bench_function(
        "No-Local Function CBWP",
        chained_spawn::no_local_function_cbwp,
    );
    group.bench_function("Async CBWP", chained_spawn::async_cbwp);
    // CBCP
    group.bench_function("Local Function CBCP", chained_spawn::local_function_cbcp);
    group.bench_function(
        "No-Local Function CBCP",
        chained_spawn::no_local_function_cbcp,
    );
    group.bench_function("Async CBCP", chained_spawn::async_cbcp);
    group.finish();
}

pub fn spawn_many_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Spawn Many");
    group.throughput(Throughput::Elements(SPAWN_NUM as u64));
    // CBWP
    group.bench_function("Function CBWP", spawn_many::function_cbwp);
    group.bench_function("Async CBWP", spawn_many::async_cbwp);
    // CBCP
    group.bench_function("Function CBCP", spawn_many::function_cbcp);
    group.bench_function("Async CBCP", spawn_many::async_cbcp);
    group.finish();
}

pub fn ping_pong_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("Ping Pong");
    group.throughput(Throughput::Elements(PINGS_NUM as u64));
    // CBWP
    //group.bench_function("Local Function CBWP", spawn_many::function_cbwp);
    group.bench_function("Async CBWP", ping_pong::async_cbwp);
    // CBCP
    //group.bench_function("Local Function CBCP", spawn_many::function_cbcp);
    group.bench_function("Async CBCP", ping_pong::async_cbcp);
    group.finish();
}

mod chained_spawn {
    use super::*;
    use criterion::Bencher;
    use std::sync::mpsc;

    fn cbwp_rt() -> impl FuturesExecutor {
        executors::crossbeam_workstealing_pool::small_pool(4)
    }

    fn cbcp_rt() -> impl FuturesExecutor {
        executors::crossbeam_channel_pool::ThreadPool::new(4)
    }

    pub fn local_function_cbwp(b: &mut Bencher) {
        let rt = cbwp_rt();
        chained_spawn_fun(b, rt);
    }

    pub fn no_local_function_cbwp(b: &mut Bencher) {
        let rt = cbwp_rt();
        chained_spawn_no_local(b, rt);
    }

    pub fn async_cbwp(b: &mut Bencher) {
        let rt = cbwp_rt();
        chained_spawn_async(b, rt);
    }

    pub fn local_function_cbcp(b: &mut Bencher) {
        let rt = cbcp_rt();
        chained_spawn_fun(b, rt);
    }

    pub fn no_local_function_cbcp(b: &mut Bencher) {
        let rt = cbcp_rt();
        chained_spawn_no_local(b, rt);
    }

    pub fn async_cbcp(b: &mut Bencher) {
        let rt = cbcp_rt();
        chained_spawn_async(b, rt);
    }

    fn chained_spawn_fun(b: &mut Bencher, rt: impl FuturesExecutor) {
        fn iter(done_tx: mpsc::SyncSender<()>, n: usize) {
            if n == 0 {
                done_tx.send(()).unwrap();
            } else {
                let _ = executors::try_execute_locally(move || {
                    iter(done_tx, n - 1);
                });
            }
        }

        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let rt_inner = rt.clone();
        b.iter(move || {
            let done_tx = done_tx.clone();
            rt_inner.execute(move || {
                iter(done_tx, CHAINING_DEPTH);
            });

            done_rx.recv().unwrap();
        });
        rt.shutdown().expect("shutdown");
    }

    fn chained_spawn_no_local(b: &mut Bencher, rt: impl FuturesExecutor) {
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
                iter(rt_new, done_tx, CHAINING_DEPTH);
            });

            done_rx.recv().unwrap();
        });
        rt.shutdown().expect("shutdown");
    }

    fn chained_spawn_async(b: &mut Bencher, rt: impl FuturesExecutor) {
        fn iter(rt: impl FuturesExecutor, done_tx: mpsc::SyncSender<()>, n: usize) {
            if n == 0 {
                done_tx.send(()).unwrap();
            } else {
                let rt_new = rt.clone();
                rt.spawn(async move {
                    iter(rt_new, done_tx, n - 1);
                })
                .detach();
            }
        }

        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let rt_inner = rt.clone();
        b.iter(move || {
            let done_tx = done_tx.clone();
            futures::executor::block_on(async {
                let rt_new = rt_inner.clone();
                rt_inner
                    .spawn(async move {
                        iter(rt_new, done_tx, CHAINING_DEPTH);
                    })
                    .detach();

                done_rx.recv().unwrap();
            });
        });
        rt.shutdown().expect("shutdown");
    }
}

mod spawn_many {
    use super::*;
    use criterion::Bencher;
    use std::sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        mpsc,
        Arc,
    };

    fn cbwp_rt() -> impl FuturesExecutor {
        executors::crossbeam_workstealing_pool::small_pool(8)
    }

    fn cbcp_rt() -> impl FuturesExecutor {
        executors::crossbeam_channel_pool::ThreadPool::new(8)
    }

    pub fn function_cbwp(b: &mut Bencher) {
        let rt = cbwp_rt();
        spawn_many_function(b, rt);
    }

    pub fn async_cbwp(b: &mut Bencher) {
        let rt = cbwp_rt();
        spawn_many_async(b, rt);
    }

    pub fn function_cbcp(b: &mut Bencher) {
        let rt = cbcp_rt();
        spawn_many_function(b, rt);
    }

    pub fn async_cbcp(b: &mut Bencher) {
        let rt = cbcp_rt();
        spawn_many_async(b, rt);
    }

    fn spawn_many_function(b: &mut Bencher, rt: impl FuturesExecutor) {
        let (tx, rx) = mpsc::sync_channel(1);
        let rem = Arc::new(AtomicUsize::new(0));

        b.iter(|| {
            rem.store(SPAWN_NUM, Relaxed);

            for _ in 0..SPAWN_NUM {
                let tx = tx.clone();
                let rem = rem.clone();

                rt.execute(move || {
                    if 1 == rem.fetch_sub(1, Relaxed) {
                        tx.send(()).unwrap();
                    }
                });
            }

            let _ = rx.recv().unwrap();
        });

        rt.shutdown().expect("shutdown");
    }

    fn spawn_many_async(b: &mut Bencher, rt: impl FuturesExecutor) {
        let (tx, rx) = mpsc::sync_channel(1);
        let rem = Arc::new(AtomicUsize::new(0));

        b.iter(|| {
            rem.store(SPAWN_NUM, Relaxed);

            futures::executor::block_on(async {
                for _ in 0..SPAWN_NUM {
                    let tx = tx.clone();
                    let rem = rem.clone();

                    rt.spawn(async move {
                        if 1 == rem.fetch_sub(1, Relaxed) {
                            tx.send(()).unwrap();
                        }
                    })
                    .detach();
                }

                let _ = rx.recv().unwrap();
            });
        });

        rt.shutdown().expect("shutdown");
    }
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

mod ping_pong {

    use super::*;
    use criterion::Bencher;
    use futures::channel::oneshot;
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering::SeqCst},
            mpsc,
            Arc,
        },
        time::Duration,
    };

    fn cbwp_rt() -> impl FuturesExecutor {
        executors::crossbeam_workstealing_pool::small_pool(4)
    }

    fn cbcp_rt() -> impl FuturesExecutor {
        executors::crossbeam_channel_pool::ThreadPool::new(4)
    }

    // pub fn local_function_cbwp(b: &mut Bencher) {
    //     let rt = cbwp_rt();
    //     chained_spawn_fun(b, rt);
    // }

    // pub fn no_local_function_cbwp(b: &mut Bencher) {
    //     let rt = cbwp_rt();
    //     chained_spawn_no_local(b, rt);
    // }

    pub fn async_cbwp(b: &mut Bencher) {
        let rt = cbwp_rt();
        ping_pong_async(b, rt);
    }

    // pub fn no_local_function_cbcp(b: &mut Bencher) {
    //     let rt = cbcp_rt();
    //     chained_spawn_no_local(b, rt);
    // }

    pub fn async_cbcp(b: &mut Bencher) {
        let rt = cbcp_rt();
        ping_pong_async(b, rt);
    }

    fn ping_pong_async(b: &mut Bencher, rt: impl FuturesExecutor) {
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        let rem = Arc::new(AtomicUsize::new(0));

        let mut count = 0u64;

        b.iter(|| {
            let done_tx = done_tx.clone();
            let rem = rem.clone();
            let outer_rem = rem.clone();
            rem.store(PINGS_NUM, SeqCst);
            count += 1u64;
            //println!("Iteration #{}", count);
            //futures::executor::block_on(async {
            let rt_new = rt.clone();
            rt.spawn(async move {
                for _i in 0..PINGS_NUM {
                    let rem = rem.clone();
                    let done_tx = done_tx.clone();
                    let rt_new2 = rt_new.clone();
                    rt_new
                        .spawn(async move {
                            let (tx1, rx1) = oneshot::channel();
                            let (tx2, rx2) = oneshot::channel();

                            rt_new2
                                .spawn(async move {
                                    rx1.await.unwrap();
                                    tx2.send(()).unwrap();
                                })
                                .detach();

                            tx1.send(()).unwrap();
                            rx2.await.unwrap();

                            let res = rem.fetch_sub(1, SeqCst);
                            if 1 == res {
                                done_tx.try_send(()).expect("done should have sent");
                            }
                            // else {
                            //     println!("Pinger {} is done, but {} remaining.", i, res);
                            // }
                        })
                        .detach();
                }
            })
            .detach();

            let res = done_rx.recv_timeout(Duration::from_millis(5000)); //.expect("should have gotten a done with 5s");
            if res.is_err() {
                panic!(
                    "done_rx timeouted within 5s. Remaining={}",
                    outer_rem.load(SeqCst)
                );
            }
            //});
        });
        rt.shutdown().expect("shutdown");
    }
}

criterion_group!(
    scheduler,
    chained_spawn_benchmark,
    spawn_many_benchmark,
    ping_pong_benchmark
);
criterion_main!(scheduler);
