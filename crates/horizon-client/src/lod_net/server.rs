//! The server side of `horizon:lod`: who may ask for which far tile
//! ([`Service::admit`]), and the one background thread that samples the tiles
//! admitted, every player in turn, and sends each reply to its player.
use super::{Bucket, IN_FLIGHT, MAX_DISTANCE, Offer, PER_SECOND, Request, SIDE, encode_reply};
use crate::streaming::{LodSampler, LodSource};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::Instant,
};

/// Tiles the server lets a player ask for at once, past [`PER_SECOND`]: the
/// network bunches requests the client spaced out.
const SERVER_BURST: f64 = 4.0 * PER_SECOND as f64;

/// One player's limits at the server.
struct Player {
    bucket: Bucket,
    in_flight: Arc<AtomicUsize>,
}

/// The server's far terrain: who may ask for what ([`Self::admit`]), and the
/// thread that samples the tiles admitted.
pub struct Service {
    pub distance: u32,
    players: HashMap<u32, Player>,
    biome_names: Arc<Vec<String>>,
    worker: Option<Worker>,
}
/// A tile admitted, waiting for the worker.
pub struct Job {
    pub conn: u32,
    /// The level's id: its sampler is kept.
    pub level: String,
    pub source: Arc<dyn LodSource>,
    pub request: Request,
    /// Sends the reply's payload to the player.
    pub reply: Box<dyn FnOnce(Vec<u8>) + Send>,
}
impl Service {
    pub fn new(distance: u32, biome_names: Vec<String>) -> Self {
        Self {
            distance: distance.min(MAX_DISTANCE),
            players: HashMap::new(),
            biome_names: Arc::new(biome_names),
            worker: None,
        }
    }
    /// The offer Horizon clients hear.
    pub fn offer(&self) -> Offer {
        Offer::new(self.distance)
    }
    /// Whether `conn`'s request may be served now to a player at
    /// `position`: a tile within the distance, with a slot and a token free.
    /// The slot is the player's until the reply goes (the job's `reply`
    /// frees it: see [`Self::slot`]).
    pub fn admit(
        &mut self,
        conn: u32,
        request: &Request,
        position: [f64; 3],
        now: Instant,
    ) -> bool {
        if !request.allowed(self.distance, position) {
            return false;
        }
        let player = self.players.entry(conn).or_insert_with(|| Player {
            bucket: Bucket::new(f64::from(PER_SECOND), SERVER_BURST, now),
            in_flight: Arc::default(),
        });
        if player.in_flight.load(Ordering::Acquire) >= IN_FLIGHT as usize
            || !player.bucket.take(now)
        {
            return false;
        }
        player.in_flight.fetch_add(1, Ordering::AcqRel);
        true
    }
    /// The counter of `conn`'s requests in flight: the reply decrements it.
    pub fn slot(&self, conn: u32) -> Option<Arc<AtomicUsize>> {
        self.players.get(&conn).map(|p| p.in_flight.clone())
    }
    /// Forget the players for whom `keep` is false (they left).
    pub fn retain(&mut self, keep: impl Fn(u32) -> bool) {
        self.players.retain(|&conn, _| keep(conn));
    }
    pub fn players(&self) -> usize {
        self.players.len()
    }
    /// Queue an admitted tile: the worker starts with the first.
    pub fn push(&mut self, job: Job) {
        let names = self.biome_names.clone();
        let worker = self.worker.get_or_insert_with(|| Worker::start(names));
        let mut queue = worker.shared.queue.lock().unwrap();
        match queue.players.iter_mut().find(|(conn, _)| *conn == job.conn) {
            Some((_, jobs)) => jobs.push_back(job),
            None => queue.players.push_back((job.conn, VecDeque::from([job]))),
        }
        drop(queue);
        worker.shared.wake.notify_one();
    }
}

struct Worker {
    shared: Arc<WorkerShared>,
    thread: Option<thread::JoinHandle<()>>,
}
struct WorkerShared {
    queue: Mutex<Queue>,
    wake: Condvar,
}
#[derive(Default)]
struct Queue {
    /// Each player's tiles, players in turn.
    players: VecDeque<(u32, VecDeque<Job>)>,
    stop: bool,
}
impl Worker {
    fn start(biome_names: Arc<Vec<String>>) -> Self {
        let shared = Arc::new(WorkerShared {
            queue: Mutex::default(),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        let thread = thread::Builder::new()
            .name("Far terrain server".into())
            .spawn(move || {
                // The ticks come first (horizon_threads).
                horizon_threads::behind_server();
                // A sampler per level, kept while its level's source is the same.
                let mut samplers: HashMap<String, (Arc<dyn LodSource>, Box<dyn LodSampler>)> =
                    HashMap::new();
                loop {
                    let job = {
                        let mut queue = worker.queue.lock().unwrap();
                        loop {
                            if queue.stop {
                                return;
                            }
                            if let Some((conn, mut jobs)) = queue.players.pop_front() {
                                let job = jobs.pop_front();
                                if !jobs.is_empty() {
                                    queue.players.push_back((conn, jobs));
                                }
                                if let Some(job) = job {
                                    break job;
                                }
                                continue;
                            }
                            queue = worker.wake.wait(queue).unwrap();
                        }
                    };
                    let same = samplers
                        .get(&job.level)
                        .is_some_and(|(source, _)| same_source(source, &job.source));
                    if !same {
                        let main = job.source.clone().sampler();
                        // A fork leaves out the client's chunk decoration threads.
                        let sampler = main.fork().unwrap_or(main);
                        samplers.insert(job.level.clone(), (job.source.clone(), sampler));
                    }
                    let (_, sampler) = samplers.get_mut(&job.level).expect("inserted above");
                    let r = job.request;
                    sampler.set_approximate(r.approximate);
                    let columns = sampler.grid(r.x, r.z, r.stride, SIDE);
                    (job.reply)(encode_reply(r.id, Some(&columns), &biome_names));
                }
            })
            .expect("far terrain server thread");
        Self {
            shared,
            thread: Some(thread),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap().stop = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
fn same_source(a: &Arc<dyn LodSource>, b: &Arc<dyn LodSource>) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn players_are_held_to_their_rate_and_slots() {
        let now = Instant::now();
        let mut service = Service::new(64, Vec::new());
        let r = Request {
            id: 0,
            x: 0,
            z: 0,
            stride: 1,
            approximate: false,
        };
        let at = [0.0; 3];
        for _ in 0..IN_FLIGHT {
            assert!(service.admit(1, &r, at, now));
        }
        // Every slot taken: refused until a reply frees one.
        assert!(!service.admit(1, &r, at, now));
        assert!(service.admit(2, &r, at, now));
        let slot = service.slot(1).unwrap();
        let mut admitted = IN_FLIGHT as usize;
        while admitted < SERVER_BURST as usize + 10 {
            slot.fetch_sub(1, Ordering::AcqRel);
            if !service.admit(1, &r, at, now) {
                break;
            }
            admitted += 1;
        }
        // The burst spent, a second gives PER_SECOND more.
        assert_eq!(admitted, SERVER_BURST as usize);
        slot.fetch_sub(1, Ordering::AcqRel);
        assert!(service.admit(1, &r, at, now + Duration::from_secs(1)));
        service.retain(|conn| conn == 2);
        assert_eq!(service.players(), 1);
    }
}
