//! The client side of `horizon:lod`: far terrain a server samples, as the
//! `LodSource` of a client of another machine's world. Each sampling thread
//! sends one tile request and waits for its reply, which the network thread
//! hands over ([`Inbox::deliver`]), within the server's [`Offer`].
use super::{Bucket, Offer, Request, SIDE, decode_reply, reply_id};
use crate::lod::{Column, Surface};
use crate::streaming::{LodSampler, LodSource};
use std::{
    collections::HashMap,
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// How long a client waits for a tile before drawing it empty.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A connection's far terrain requests: their replies come in on the network
/// thread ([`Self::deliver`]) and go to the sampler waiting for each. One per
/// connection, so the server's limits hold across the far terrains (one per
/// dimension) a connection has in turn.
#[derive(Default)]
pub struct Inbox {
    state: Mutex<InboxState>,
    changed: Condvar,
}
#[derive(Default)]
struct InboxState {
    /// Requests sent and not answered: when each went, and the sampler
    /// waiting for it (None once given up).
    waiting: HashMap<u32, (Instant, Option<mpsc::Sender<Vec<u8>>>)>,
    next: u32,
    bucket: Option<Bucket>,
    /// Bumped by [`Inbox::cancel`]: samplers of an older one stop.
    epoch: u64,
}
impl Inbox {
    /// A reply from the server: to its sampler, and its slot freed.
    pub fn deliver(&self, data: Vec<u8>) {
        let Some(id) = reply_id(&data) else {
            return;
        };
        let mut state = self.state.lock().unwrap();
        if let Some((_, Some(tx))) = state.waiting.remove(&id) {
            let _ = tx.send(data);
        }
        drop(state);
        self.changed.notify_all();
    }
    /// The far terrain ends (another dimension, a disconnect): its samplers
    /// stop waiting at once. Requests still at the server keep their slot
    /// until it answers.
    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.epoch += 1;
        for (_, tx) in state.waiting.values_mut() {
            *tx = None;
        }
        drop(state);
        self.changed.notify_all();
    }
    fn epoch(&self) -> u64 {
        self.state.lock().unwrap().epoch
    }
    /// A slot for one request, within the offer's limits, once there is one;
    /// None once the far terrain of `epoch` was cancelled.
    fn acquire(&self, epoch: u64, offer: Offer) -> Option<(u32, mpsc::Receiver<Vec<u8>>)> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.epoch != epoch {
                return None;
            }
            let now = Instant::now();
            // A request the server never answered (a reconfiguration lost
            // it) frees its slot in time.
            state
                .waiting
                .retain(|_, (sent, _)| now.duration_since(*sent) < TIMEOUT);
            let rate = f64::from(offer.per_second);
            let free = state.waiting.len() < offer.in_flight as usize;
            let bucket = state
                .bucket
                .get_or_insert_with(|| Bucket::new(rate, rate, now));
            if free && bucket.take(now) {
                let id = state.next;
                state.next = state.next.wrapping_add(1);
                let (tx, rx) = mpsc::channel();
                state.waiting.insert(id, (now, Some(tx)));
                return Some((id, rx));
            }
            state = self
                .changed
                .wait_timeout(state, Duration::from_millis(20))
                .unwrap()
                .0;
        }
    }
}

/// Far terrain sampled by the server: the `LodSource` of a client of
/// another machine's world.
pub struct RemoteSource {
    inbox: Arc<Inbox>,
    epoch: u64,
    offer: Offer,
    /// Sends a request's payload to the server.
    send: Box<dyn Fn(Vec<u8>) + Send + Sync>,
    /// This client's biome indices by name.
    biomes: HashMap<String, u16>,
    /// Block states this client knows.
    states: u32,
    /// Samplers made: no more than requests may wait.
    samplers: AtomicUsize,
}
impl RemoteSource {
    pub fn new(
        inbox: Arc<Inbox>,
        offer: Offer,
        send: Box<dyn Fn(Vec<u8>) + Send + Sync>,
        biomes: HashMap<String, u16>,
        states: u32,
    ) -> Arc<Self> {
        Arc::new(Self {
            epoch: inbox.epoch(),
            inbox,
            offer,
            send,
            biomes,
            states,
            samplers: AtomicUsize::new(0),
        })
    }
    /// One tile's columns, empty when the server refused it or the far
    /// terrain ended first (as a failed local sample is).
    fn tile(&self, x: i32, z: i32, stride: i32, approximate: bool) -> Vec<Column> {
        let empty = || vec![Column::default(); SIDE * SIDE];
        let Some((id, reply)) = self.inbox.acquire(self.epoch, self.offer) else {
            return empty();
        };
        (self.send)(
            Request {
                id,
                x,
                z,
                stride,
                approximate,
            }
            .encode(),
        );
        let Ok(data) = reply.recv_timeout(TIMEOUT) else {
            return empty();
        };
        match decode_reply(&data, &self.biomes, self.states) {
            Ok(Some(columns)) => columns,
            Ok(None) => empty(),
            Err(e) => {
                log::warn!("Horizon: far terrain from the server: {e}");
                empty()
            }
        }
    }
}
impl LodSource for RemoteSource {
    fn sampler(self: Arc<Self>) -> Box<dyn LodSampler> {
        self.samplers.fetch_add(1, Ordering::Relaxed);
        Box::new(RemoteSampler {
            source: self,
            approximate: false,
        })
    }
}
struct RemoteSampler {
    source: Arc<RemoteSource>,
    approximate: bool,
}
impl LodSampler for RemoteSampler {
    /// Each sampling thread waits for one request: no more threads than the
    /// server lets requests wait.
    fn fork(&self) -> Option<Box<dyn LodSampler>> {
        let samplers = &self.source.samplers;
        if samplers.load(Ordering::Relaxed) >= self.source.offer.in_flight as usize {
            return None;
        }
        Some(self.source.clone().sampler())
    }
    fn set_approximate(&mut self, approximate: bool) {
        self.approximate = approximate;
    }
    fn sample(&mut self, _x: i32, _z: i32) -> Option<Surface> {
        None
    }
    fn grid(&mut self, x: i32, z: i32, stride: i32, side: usize) -> Vec<Column> {
        if side != SIDE {
            return vec![Column::default(); side * side];
        }
        self.source.tile(x, z, stride, self.approximate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lod_net::refusal;

    #[test]
    fn a_cancelled_inbox_frees_its_samplers() {
        let inbox = Arc::new(Inbox::default());
        let offer = Offer::new(64);
        let epoch = inbox.epoch();
        let (id, reply) = inbox.acquire(epoch, offer).unwrap();
        inbox.cancel();
        assert!(reply.recv_timeout(Duration::from_secs(1)).is_err());
        assert!(inbox.acquire(epoch, offer).is_none());
        // The late reply still frees the slot it held.
        inbox.deliver(refusal(id));
        assert!(inbox.state.lock().unwrap().waiting.is_empty());
    }
}
