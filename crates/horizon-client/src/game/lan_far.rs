//! Far terrain for a server's Horizon clients (`horizon_client::lod_net`): a
//! `horizon:lod` request is checked against the player's limits and position,
//! then sampled from the level the player is in on the far terrain thread,
//! which sends the reply to the player itself. A refused request is answered
//! at once, so the client draws that tile empty rather than waiting.
use super::{Dim, Game, lan::LanHost};
use horizon_client::lod_net::{Job, Request, refusal};
use horizon_protocol::packets::{configuration::CustomPayload, play};
use std::sync::atomic::Ordering;

impl Game<'_> {
    /// A `horizon:lod` payload from `conn`.
    pub(super) fn guest_far_terrain(&mut self, host: &mut LanHost, conn: u32, data: &[u8]) {
        let Some(far) = host.far.as_mut() else {
            return;
        };
        let Some(guest) = host.hub.guest(conn) else {
            return;
        };
        // A request that doesn't parse has no id to answer.
        let Ok(request) = Request::decode(data) else {
            return;
        };
        let send = |sender: &horizon_client::lan::conn::PlayerSender, data: Vec<u8>| {
            let packet = play::Clientbound::CustomPayload(CustomPayload {
                channel: horizon_client::lod_net::CHANNEL.into(),
                data,
            });
            sender.send(packet.encode());
        };
        // Players who left are forgotten once they could matter.
        if far.players() > host.hub.guests.len() + 8 {
            let guests = &host.hub.guests;
            far.retain(|c| guests.iter().any(|g| g.conn == c));
        }
        let dim = Dim::from_id(&guest.dimension);
        let source = self.lod_source_in(dim);
        let admitted = source.is_some()
            && far.admit(conn, &request, guest.position, std::time::Instant::now());
        let (Some(source), true) = (source, admitted) else {
            send(&guest.sender, refusal(request.id));
            return;
        };
        let slot = far.slot(conn).expect("admitted above");
        let sender = guest.sender.clone();
        far.push(Job {
            conn,
            level: dim.id().to_owned(),
            source,
            request,
            reply: Box::new(move |data| {
                send(&sender, data);
                slot.fetch_sub(1, Ordering::AcqRel);
            }),
        });
    }
}
