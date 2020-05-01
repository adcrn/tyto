pub mod middleware;

use actix_web::{web, HttpRequest, HttpResponse, Responder};

use crate::bencode;
use crate::bittorrent::{AnnounceRequest, AnnounceResponse, ScrapeRequest, ScrapeResponse};
use crate::state::State;
use crate::statistics::ReturnedStatistics;
use crate::util::Event;

pub async fn parse_announce(data: web::Data<State>, req: HttpRequest) -> impl Responder {
    let announce_request = AnnounceRequest::new(req.query_string(), req.connection_info().remote());

    match announce_request {
        Ok(parsed_req) => {
            // There are only three types of events that lead to
            // actual change between swarms on the storage layer
            match parsed_req.event {
                // Started should be sent whenever a client
                // starts or resumes the leeching process
                Event::Started => {
                    data.peer_store
                        .put_leecher(parsed_req.info_hash.clone(), parsed_req.peer)
                        .await;
                    data.torrent_store
                        .new_leech(parsed_req.info_hash.clone())
                        .await;

                    // Get randomized peer list
                    let (peers, peers6) = data
                        .peer_store
                        .get_peers(parsed_req.info_hash.clone(), parsed_req.numwant.unwrap())
                        .await;

                    let (complete, incomplete) = data
                        .torrent_store
                        .get_announce_stats(parsed_req.info_hash)
                        .await;

                    // Associate all the requisite data together and
                    // respond with the bencoded version of the data
                    let response = AnnounceResponse::new(
                        data.config.bt.announce_rate as u32,
                        complete,
                        incomplete,
                        peers,
                        peers6,
                    );

                    let mut stats = data.stats.write().await;
                    stats.add_leech();
                    stats.succ_announce();

                    let bencoded = bencode::encode_announce_response(response.unwrap());
                    HttpResponse::Ok().content_type("text/plain").body(bencoded)
                }

                // Stopped should be sent when a client stops seed or leeching
                Event::Stopped => {
                    // If the peer is present in one set, then it
                    // cannot be present in the other.
                    let mut stats = data.stats.write().await;

                    if data
                        .peer_store
                        .remove_seeder(parsed_req.info_hash.clone(), parsed_req.peer.clone())
                        .await
                    {
                        stats.sub_seed();
                    } else {
                        data.peer_store
                            .remove_leecher(parsed_req.info_hash.clone(), parsed_req.peer)
                            .await;
                        stats.sub_leech();
                    }

                    stats.succ_announce();

                    let (peers, peers6) = data
                        .peer_store
                        .get_peers(parsed_req.info_hash.clone(), parsed_req.numwant.unwrap())
                        .await;

                    let (complete, incomplete) = data
                        .torrent_store
                        .get_announce_stats(parsed_req.info_hash)
                        .await;

                    let response = AnnounceResponse::new(
                        data.config.bt.announce_rate as u32,
                        complete,
                        incomplete,
                        peers,
                        peers6,
                    );
                    let bencoded = bencode::encode_announce_response(response.unwrap());
                    HttpResponse::Ok().content_type("text/plain").body(bencoded)
                }

                // Completed should be sent when a peer receives 100%
                // of the data associated with a particular torrent
                Event::Completed => {
                    data.peer_store
                        .promote_leecher(parsed_req.info_hash.clone(), parsed_req.peer)
                        .await;
                    data.torrent_store
                        .new_seed(parsed_req.info_hash.clone())
                        .await;

                    let (peers, peers6) = data
                        .peer_store
                        .get_peers(parsed_req.info_hash.clone(), parsed_req.numwant.unwrap())
                        .await;

                    let (complete, incomplete) = data
                        .torrent_store
                        .get_announce_stats(parsed_req.info_hash)
                        .await;

                    let response = AnnounceResponse::new(
                        data.config.bt.announce_rate as u32,
                        complete,
                        incomplete,
                        peers,
                        peers6,
                    );
                    let mut stats = data.stats.write().await;
                    stats.promote_leech();
                    stats.succ_announce();

                    let bencoded = bencode::encode_announce_response(response.unwrap());
                    HttpResponse::Ok().content_type("text/plain").body(bencoded)
                }

                // None should only be sent if
                // there is no change in snatch state
                Event::None => {
                    // This updates a peer if it is present in either swarm.
                    // It is intended that a client correctly send its states.
                    // If a client starts out with this event, it will never be added.
                    data.peer_store
                        .update_peer(parsed_req.info_hash.clone(), parsed_req.peer)
                        .await;

                    let (peers, peers6) = data
                        .peer_store
                        .get_peers(parsed_req.info_hash.clone(), parsed_req.numwant.unwrap())
                        .await;

                    let (complete, incomplete) = data
                        .torrent_store
                        .get_announce_stats(parsed_req.info_hash)
                        .await;

                    let response = AnnounceResponse::new(
                        data.config.bt.announce_rate as u32,
                        complete,
                        incomplete,
                        peers,
                        peers6,
                    );
                    let bencoded = bencode::encode_announce_response(response.unwrap());
                    data.stats.write().await.succ_announce();
                    HttpResponse::Ok().content_type("text/plain").body(bencoded)
                }
            }
        }

        // If the request is not parse-able, short-circuit and respond with failure
        Err(failure) => {
            let bencoded = bencode::encode_announce_response(failure);
            data.stats.write().await.fail_announce();
            HttpResponse::Ok().content_type("text/plain").body(bencoded)
        }
    }
}

pub async fn parse_scrape(data: web::Data<State>, req: HttpRequest) -> impl Responder {
    let scrape_request = ScrapeRequest::new(req.query_string());
    match scrape_request {
        Ok(parsed_req) => {
            let scrape_files = data.torrent_store.get_scrapes(parsed_req.info_hashes).await;
            let mut scrape_response = ScrapeResponse::new().unwrap();

            for file in scrape_files {
                scrape_response.add_file(file.info_hash.clone(), file);
            }

            let bencoded = bencode::encode_scrape_response(scrape_response);
            data.stats.write().await.incr_scrapes();
            HttpResponse::Ok().content_type("text/plain").body(bencoded)
        }

        Err(failure) => {
            let bencoded = bencode::encode_scrape_response(failure);
            HttpResponse::Ok().content_type("text/plain").body(bencoded)
        }
    }
}

pub async fn get_stats(data: web::Data<State>) -> impl Responder {
    let global_stats = data.stats.read().await;
    let stats = ReturnedStatistics::new(&global_stats);
    web::Json(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    use actix_service::Service;
    use actix_web::{test, web, App, HttpResponse};

    use crate::config::Config;
    use crate::state::State;
    use crate::storage::{Torrent, TorrentRecords, TorrentStore};

    #[actix_rt::test]
    async fn index_get_not_allowed() {
        let config = Config::default();
        let torrent_store = TorrentStore::new(TorrentRecords::new());
        let stores = web::Data::new(State::new(config, torrent_store));
        let mut app = test::init_service(
            App::new()
                .service(
                    web::scope("announce")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_announce)),
                )
                .service(
                    web::scope("scrape")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_scrape)),
                )
                .service(
                    web::scope("/").route("", web::get().to(|| HttpResponse::MethodNotAllowed())),
                ),
        )
        .await;

        let req = test::TestRequest::with_uri("/").to_request();
        let resp = app.call(req).await.unwrap();

        assert!(resp.status().is_client_error());
    }

    #[actix_rt::test]
    async fn announce_get_malformed() {
        let config = Config::default();
        let torrent_store = TorrentStore::new(TorrentRecords::new());
        let stores = web::Data::new(State::new(config, torrent_store));
        let mut app = test::init_service(
            App::new()
                .service(
                    web::scope("announce")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_announce)),
                )
                .service(
                    web::scope("scrape")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_scrape)),
                )
                .service(
                    web::scope("/").route("", web::get().to(|| HttpResponse::MethodNotAllowed())),
                ),
        )
        .await;

        let proper_resp = "d14:failure_reason26:Malformed announce requeste".as_bytes();
        let req = test::TestRequest::with_uri("/announce?bad_stuff=123").to_request();
        let resp = test::read_response(&mut app, req).await;

        assert_eq!(resp, proper_resp);
    }

    #[actix_rt::test]
    async fn scrape_get_malformed() {
        let config = Config::default();
        let torrent_store = TorrentStore::new(TorrentRecords::new());
        let stores = web::Data::new(State::new(config, torrent_store));
        let mut app = test::init_service(
            App::new()
                .service(
                    web::scope("announce")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_announce)),
                )
                .service(
                    web::scope("scrape")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_scrape)),
                )
                .service(
                    web::scope("/").route("", web::get().to(|| HttpResponse::MethodNotAllowed())),
                ),
        )
        .await;

        let proper_resp = "d14:failure_reason24:Malformed scrape requeste".as_bytes();
        let req = test::TestRequest::with_uri("/scrape?bad_stuff=123").to_request();
        let resp = test::read_response(&mut app, req).await;

        assert_eq!(resp, proper_resp);
    }

    #[actix_rt::test]
    async fn scrape_get_success() {
        let config = Config::default();
        let torrent_store = TorrentStore::new(TorrentRecords::new());
        let stores = web::Data::new(State::new(config, torrent_store));

        let info_hash1 = "A1B2C3D4E5F6G7H8I9J0".to_string();
        let torrent1 = Torrent::new(info_hash1, 10, 34, 7, 10000000);

        let info_hash2 = "B2C3D4E5F6G7H8I9J0K1".to_string();
        let torrent2 = Torrent::new(info_hash2, 25, 57, 19, 20000000);

        {
            let mut store = stores.torrent_store.torrents.write().await;
            store.insert(torrent1.info_hash.clone(), torrent1);
            store.insert(torrent2.info_hash.clone(), torrent2);
        }

        let mut app = test::init_service(
            App::new()
                .service(
                    web::scope("announce")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_announce)),
                )
                .service(
                    web::scope("scrape")
                        .app_data(stores.clone())
                        .route("", web::get().to(parse_scrape)),
                )
                .service(
                    web::scope("/").route("", web::get().to(|| HttpResponse::MethodNotAllowed())),
                ),
        )
        .await;

        let uri = "/scrape?info_hash=A1B2C3D4E5F6G7H8I9J0\
                   &info_hash=B2C3D4E5F6G7H8I9J0K1";

        let proper_resp = "d5:filesd20:A1B2C3D4E5F6G7H8I9J0d8:completei10e10:downloadedi34e10:incompletei7ee20:B2C3D4E5F6G7H8I9J0K1d8:completei25e10:downloadedi57e10:incompletei19eeee".as_bytes();
        let req = test::TestRequest::with_uri(uri).to_request();
        let resp = test::read_response(&mut app, req).await;

        assert_eq!(resp, proper_resp);
    }
}
