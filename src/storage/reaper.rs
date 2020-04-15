use crate::bittorrent::Peer;
use crate::storage;

use std::time::Duration;

use actix::prelude::*;
use actix_web::web;

#[derive(Clone)]
pub struct Reaper {
    interval: Duration,
    peer_timeout: Duration,
    state: web::Data<storage::Stores>,
}

impl Reaper {
    pub fn new(
        interval_secs: u64,
        peer_timeout_secs: u64,
        state: web::Data<storage::Stores>,
    ) -> Reaper {
        Reaper {
            interval: Duration::new(interval_secs, 0),
            peer_timeout: Duration::new(peer_timeout_secs, 0),
            state,
        }
    }

    // Had to clone self to avoid wacky lifetime error
    fn reap_peers(&mut self, ctx: &mut Context<Self>) {
        let self2 = self.clone();
        ctx.spawn(actix::fut::wrap_future(async move {
            info!("Reaping peers...");

            let mut seeds_reaped = 0;
            let mut leeches_reaped = 0;

            let info_hashes: Vec<String> = self2
                .state
                .peer_store
                .records
                .read()
                .await
                .iter()
                .map(|(info_hash, _)| info_hash.clone())
                .collect();

            for info_hash in info_hashes {
                if let Some(swarm) = self2
                    .state
                    .peer_store
                    .records
                    .write()
                    .await
                    .get_mut(&info_hash)
                {
                    let seeds_1 = swarm.seeders.len();
                    let leeches_1 = swarm.leechers.len();

                    swarm.seeders.retain(|peer| match peer {
                        Peer::V4(p) => p.last_announced.elapsed() < self2.peer_timeout,
                        Peer::V6(p) => p.last_announced.elapsed() < self2.peer_timeout,
                    });
                    swarm.leechers.retain(|peer| match peer {
                        Peer::V4(p) => p.last_announced.elapsed() < self2.peer_timeout,
                        Peer::V6(p) => p.last_announced.elapsed() < self2.peer_timeout,
                    });

                    seeds_reaped += seeds_1 - swarm.seeders.len();
                    leeches_reaped += leeches_1 - swarm.leechers.len();
                }
            }

            info!(
                "Reaped {} seeders and {} leechers.",
                seeds_reaped, leeches_reaped
            );
        }));
    }
}

impl Actor for Reaper {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Context<Self>) {
        info!("Reaper is now lurking...");
        // This will go through all of the swarms and remove
        // any peers that have not announced in a defined time
        ctx.run_interval(self.interval, Self::reap_peers);
    }
}
