//! The movement prediction's replay on the server (`lan::prediction`): each guest's client ticks
//! played again with the client's own movement code, in the guest's level.
use super::Game;
use super::lan::LanHost;
use horizon_client::lan::entity::EntityState;
use horizon_client::lan::guest::Guest;
use horizon_client::lan::prediction::{Replay, near};
use horizon_client::motion::{MotionWorld, PlayerMotion};
use horizon_client::world::World;

impl Game<'_> {
    /// Guest `conn`'s client ticks since the server's last tick, played again, before the server
    /// moves the guest's own body.
    pub(super) fn guest_replay(&mut self, host: &mut LanHost, conn: u32) {
        if !host.hub.recording() {
            return;
        }
        let tolerance = host
            .hub
            .anticheat
            .as_deref()
            .map_or(f64::INFINITY, |ac| ac.config.prediction.tolerance);
        let Some(g) = host.hub.guest_mut(conn) else {
            return;
        };
        // What moves the player isn't its own movement (a vehicle, a bed), or the server holds it:
        // the replay starts again at the teleport that ends it (dismountTo, stopSleeping, respawn).
        let held = g.vehicle.is_some()
            || g.sleeping
            || g.dead
            || g.spectator()
            || g.awaiting_teleport.is_some()
            || !g.client_loaded();
        let (Some(record), Some(server)) = (g.anticheat.record.as_deref_mut(), g.motion.as_deref())
        else {
            return;
        };
        if let Some((at, kept_motion)) = record.take_restart() {
            let mut replay = Replay::new(server, at, record);
            // The client's motion through the teleport is its own: the replay finds it again.
            if kept_motion {
                replay.excuse();
            }
            g.replay = Some(Box::new(replay));
        }
        if held {
            g.replay = None;
            return;
        }
        let Some(replay) = g.replay.as_deref_mut() else {
            return;
        };
        let run = |body: &mut _, keys, sprinting| {
            MotionWorld {
                entities: &[],
                player: body,
                scene: &mut self.world,
                assets: &self.pack.assets,
                collision_binding: self.collision,
                resources: self.motion,
            }
            .replayed_tick(keys, sprinting)
        };
        // Ticks no longer kept (a server that stalled for seconds): wait for the next teleport.
        let Some(over) = replay.catch_up(record, server, tolerance, run) else {
            g.replay = None;
            return;
        };
        if over.ticks == 0 {
            return;
        }
        let g = host.hub.guest(conn).expect("found above");
        if excused(g, &self.world, host.states.as_slice(), &host.hub.guests) {
            if let Some(replay) = host
                .hub
                .guest_mut(conn)
                .and_then(|g| g.replay.as_deref_mut())
            {
                replay.excuse();
            }
            return;
        }
        let agreed = g.replay.as_deref().expect("played above").agreed();
        for event in host.hub.anticheat_prediction(conn, &over, agreed) {
            self.lan_log_event(event);
        }
    }
}

/// Whether guest `g`'s last moves may be off for a reason the replay can't play yet: an entity near
/// it (a push, a boat or shulker to stand on, a firework it flies with), a block change near it the
/// client hadn't seen or the server hasn't sent, a moving piston, a chunk around it the client doesn't
/// have, or a riptide launch (the trident's push is the client's own).
fn excused(g: &Guest, world: &World, entities: &[EntityState], guests: &[Guest]) -> bool {
    let at = g.position;
    let close = |p: [f64; 3]| {
        (p[0] - at[0]).abs() <= 3.0
            && (p[2] - at[2]).abs() <= 3.0
            && (-3.0..=4.0).contains(&(p[1] - at[1]))
    };
    let unseen = g
        .anticheat
        .record
        .as_deref()
        .is_some_and(|r| r.unseen_block(|b| near(at, b)));
    let unsent = world
        .block_log
        .as_ref()
        .is_some_and(|log| log.iter().any(|&(x, y, z)| near(at, [x, y, z])));
    let pistons = world.moving_pistons.iter().any(|&(p, _)| near(at, p));
    // A client doesn't move in a chunk it doesn't have.
    let (cx, cz) = ((at[0].floor() as i32) >> 4, (at[2].floor() as i32) >> 4);
    let unloaded =
        (-1..=1).any(|dx| (-1..=1).any(|dz| !g.sent_chunks.contains(&(cx + dx, cz + dz))));
    let riptide = g
        .motion
        .as_deref()
        .is_some_and(|m: &PlayerMotion| m.auto_spin_attack_ticks > 0);
    // Items and experience orbs neither push nor hold anyone up; the list holds the player itself.
    let entity = entities.iter().any(|e| {
        e.id != g.entity_id
            && !matches!(
                e.kind.as_str(),
                "minecraft:item" | "minecraft:experience_orb"
            )
            && close(e.position)
    });
    let player = guests
        .iter()
        .any(|o| o.conn != g.conn && o.placed && o.dimension == g.dimension && close(o.position));
    unseen || unsent || pistons || unloaded || riptide || entity || player
}
