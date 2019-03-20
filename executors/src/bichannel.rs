// Copyright 2017 Lars Kroll. See the LICENSE
// file at the top-level directory of this distribution.
//
// Licensed under the MIT license
// <LICENSE or http://opensource.org/licenses/MIT>.
// This file may not be copied, modified, or distributed
// except according to those terms.

//! A simple abstraction for bidirectional 1-to-1 channels built over
//! [`std::sync::mpsc`](https://doc.rust-lang.org/std/sync/mpsc/).
//!
//! Bichannels have two asymmetrical endpoints [`LeftEnd`](), [`RightEnd`]()
//! which provide mirror images of send/receive functions to each other.
//!

use std::sync::mpsc;
use std::sync::mpsc::*;
use std::time::Duration;

/// Creates a new asynchronous bidirectional channel, returning the 
/// the two asymmetrical Endpoints. 
/// All data sent on an Endpoint will become available on the other Endpoint
/// in the same order as it was sent, and no send will block the calling thread 
/// (this channel has an "infinite buffer")
/// The recv will block until a message is available.
///
/// Neither Endpoint maye be cloned, but both may be send to different threads.
///
/// If either Endpoint is disconnected while trying to send, 
/// the send/recv methods will return a SendError/RecvError. 
/// 
pub fn bichannel<Left, Right>() -> (Endpoint<Right, Left>, Endpoint<Left, Right>) {
    let (tx_left, rx_left) = mpsc::channel::<Left>();
    let (tx_right, rx_right) = mpsc::channel::<Right>();
    let endpoint_left = Endpoint::new(tx_left, rx_right);
    let endpoint_right = Endpoint::new(tx_right, rx_left);
    (endpoint_left, endpoint_right)
}

pub struct Endpoint<In, Out> {
    sender: Sender<Out>,
    receiver: Receiver<In>,
}

impl<In, Out> Endpoint<In, Out> {
    fn new(sender: Sender<Out>, receiver: Receiver<In>) -> Endpoint<In, Out> {
        Endpoint {
            sender, 
            receiver,
        }
    }
    pub fn send(&self, t: Out) -> Result<(), SendError<Out>> {
        self.sender.send(t)
    }
    pub fn try_recv(&self) -> Result<In, TryRecvError> {
        self.receiver.try_recv()
    }
    pub fn recv(&self) -> Result<In, RecvError> {
        self.receiver.recv()
    }
    pub fn recv_timeout(&self, timeout: Duration) -> Result<In, RecvTimeoutError> {
        self.receiver.recv_timeout(timeout)
    }
    pub fn iter(&self) -> Iter<'_, In> {
        self.receiver.iter()
    }
    pub fn try_iter(&self) -> TryIter<'_, In> {
        self.receiver.try_iter()
    }
}

//impl<In, Out> !Sync for Endpoint<In, Out> {}
unsafe impl<In: Send, Out: Send> Send for Endpoint<In, Out> {}

#[cfg(test)]
mod tests {
    use env_logger;

    use super::*;
    use std::time::Duration;
    use std::thread;
    use std::sync::Arc;
    use crate::common::ignore;
    use synchronoise::CountdownEvent;
    struct Ping;
    struct Pong;

    const ROUNDTRIPS: usize = 10;

    #[test]
    fn ping_pong() {
        let _ = env_logger::try_init();

        let latch = Arc::new(CountdownEvent::new(2));
        let latch_left = latch.clone();
        let latch_right = latch.clone();
        let (left, right) = bichannel::<Ping, Pong>();
        thread::spawn(move || {
            for _ in 0..ROUNDTRIPS {
                left.send(Ping).expect("should send ping");
                left.recv().expect("should get pong");
            }
            ignore(latch_left.decrement());
        });
        thread::spawn(move || {
            for _ in 0..ROUNDTRIPS {
                right.recv().expect("should get pong");
                right.send(Pong).expect("should send pong");
            }
            ignore(latch_right.decrement());
        });
        let res = latch.wait_timeout(Duration::from_secs(5));
        assert_eq!(res, 0);
    }
}