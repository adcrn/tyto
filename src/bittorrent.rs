// This package contains structures specific to the BitTorrent protocol.
// Most of the information is coming from the following link:
// https://wiki.theory.org/index.php/BitTorrentSpecification

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use bip_bencode::{BDecodeOpt, BRefAccess, BencodeRef};
use bytes::{BufMut, BytesMut};
use url::{form_urlencoded, Url};

// These two peer types could probably be implemented more elegantly
// with a trait, but there's only two types right now, so it's not a lot of work
pub struct Peerv4 {
    peer_id: String, // This should be 20 bytes in length
    ip: Ipv4Addr,
    port: u16,
}

pub struct Peerv6 {
    peer_id: String, // This should be 20 bytes in length
    ip: Ipv6Addr,
    port: u16,
}

impl Peerv4 {
    fn compact(&self) -> Vec<u8> {
        let mut ip: u32 = 0;
        let octets = self.ip.octets();
        let mut num_octets = (octets.len() - 1) as u32;

        // Shift the octet by 3xN bits where N is the position
        // of the octet from the end of the address
        for x in octets.iter() {
            if (num_octets > 0) {
                ip += u32::from(*x) << (8 * num_octets);
                num_octets -= 1;
            } else {
                ip += u32::from(*x);
            }
        }

        let mut full_compact_peer = vec![];
        full_compact_peer.put_u32_be(ip);
        full_compact_peer.put_u16_be(self.port);

        full_compact_peer
    }
}

impl Peerv6 {
    // BEP 07: IPv6 Tracker Extension
    fn compact(&self) -> Vec<u8> {
        let mut ip: u128 = 0;
        let octets = self.ip.octets();
        let mut num_octets = (octets.len() - 1) as u128;

        // Same as peerv4, but each octet should be a hexadecimal number
        for x in octets.iter() {
            if (num_octets > 0) {
                ip += u128::from(*x) << (8 * num_octets);
                num_octets -= 1;
            } else {
                ip += u128::from(*x);
            }
        }

        // Had some trouble getting i128 features to work with
        // vectors, so this is a workaround; slowdown should be minimal
        let mut full_compact_peer = vec![];
        full_compact_peer.put_slice(&ip.to_be_bytes());
        full_compact_peer.put_slice(&self.port.to_be_bytes());

        full_compact_peer
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    Started,
    Stopped,
    Completed,
    None,
}

pub struct AnnounceRequest {
    pub info_hash: String,
    pub peer: String,
    pub port: u16,
    pub uploaded: u32,
    pub downloaded: u32,
    pub left: u32,
    pub compact: bool,
    pub no_peer_id: bool,
    pub event: Event,
}

impl AnnounceRequest {
    pub fn new(url_string: &str) -> Result<AnnounceRequest, &str> {
        // Get rid of these unwraps later
        let url = Url::parse(url_string).unwrap();
        let query = url.query().unwrap();
        let request_kv_pairs = form_urlencoded::parse(query.as_bytes()).into_owned();

        let mut info_hash: String = "".to_string();
        let mut peer: String = "".to_string();
        let mut port = 0;
        let mut uploaded = 0;
        let mut downloaded = 0;
        let mut left = 0;
        let mut compact = false;
        let mut no_peer_id = false;
        let mut event = Event::None;

        for (key, value) in request_kv_pairs {
            match key.as_str() {
                "info_hash" => info_hash = value,
                "peer" => peer = value,
                "port" => match value.parse::<u16>() {
                    Ok(n) => port = n,
                    _ => return Err("Unable to parse port"),
                },
                "uploaded" => match value.parse::<u32>() {
                    Ok(n) => uploaded = n,
                    _ => return Err("Unable to parse uploaded quantity"),
                },
                "downloaded" => match value.parse::<u32>() {
                    Ok(n) => downloaded = n,
                    _ => return Err("Unable to parse downloaded quantity"),
                },
                "left" => match value.parse::<u32>() {
                    Ok(n) => left = n,
                    _ => return Err("Unable to parse remaining quantity"),
                },
                "compact" => match value.parse::<u32>() {
                    Ok(n) => compact = n != 0,
                    _ => return Err("Unable to parse compact value as boolean"),
                },
                "no_peer_id" => match value.parse::<u32>() {
                    Ok(n) => no_peer_id = n != 0,
                    _ => return Err("Unable to parse no_peer_id as boolean"),
                },
                "event" => event = string_to_event(value),
                _ => {}
            }
        }

        Ok(AnnounceRequest {
            info_hash,
            peer,
            port,
            uploaded,
            downloaded,
            left,
            compact,
            no_peer_id,
            event,
        })
    }
}

// Peer types are functionally the same, but due to different
// byte lengths, they should be separated for client compatibility
#[derive(Default)]
pub struct AnnounceResponse {
    pub failure_reason: String,
    pub interval: u32,
    pub tracker_id: String,
    pub complete: u32,
    pub incomplete: u32,
    pub peers: Vec<Peerv4>,
    pub peers6: Vec<Peerv6>,
}

impl AnnounceResponse {
    pub fn new(
        interval: u32,
        complete: u32,
        incomplete: u32,
        peers: Vec<Peerv4>,
        peers6: Vec<Peerv6>,
    ) -> Result<AnnounceResponse, &'static str> {
        Ok(AnnounceResponse {
            failure_reason: "".to_string(),
            interval,
            tracker_id: "".to_string(),
            complete,
            incomplete,
            peers,
            peers6,
        })
    }

    // If a failure reason is present, no other keys should be defined
    pub fn fail(failure_reason: String) -> AnnounceResponse {
        AnnounceResponse {
            failure_reason,
            ..Default::default()
        }
    }
}

#[derive(Debug, Default)]
pub struct ScrapeFile {
    pub complete: u32,
    pub downloaded: u32,
    pub incomplete: u32,
    pub name: String,
}

pub struct ScrapeRequest {
    info_hashes: Vec<String>,
}

impl ScrapeRequest {
    pub fn new(url_string: &str) -> Result<ScrapeRequest, &str> {
        let url = Url::parse(url_string).unwrap();
        let query = url.query().unwrap();
        let request_kv_pairs = form_urlencoded::parse(query.as_bytes()).into_owned();
        let mut info_hashes = Vec::new();

        for (key, value) in request_kv_pairs {
            match key.as_str() {
                "info_hash" => info_hashes.push(value),
                _ => return Err("Malformed scrape request"),
            }
        }

        Ok(ScrapeRequest { info_hashes })
    }
}

pub struct ScrapeResponse {
    pub files: Vec<ScrapeFile>,
}

impl ScrapeResponse {
    pub fn new() -> Result<ScrapeResponse, ()> {
        Ok(ScrapeResponse { files: vec![] })
    }

    pub fn add_file(&mut self, scrape_file: ScrapeFile) {
        self.files.push(scrape_file);
    }
}

fn string_to_event(s: String) -> Event {
    match s.as_ref() {
        "started" => Event::Started,
        "stopped" => Event::Stopped,
        "completed" => Event::Completed,
        "" => Event::None,
        _ => Event::None,
        // MAYBE:
        // This should probably return an error such as "Error:
        // Malformed Request" along with the PeerID of the client,
        // and ignore the request from the client.
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::Event;
    use super::{
        AnnounceRequest, AnnounceResponse, Peerv4, Peerv6, ScrapeFile, ScrapeRequest,
        ScrapeResponse,
    };

    use crate::bittorrent::string_to_event;
    use bytes::{BufMut, BytesMut};

    #[test]
    fn announce_good_request_creation() {
        let url_string = "http://tracker/announce?\
                          info_hash=%9A%813%3C%1B%16%E4%A8%3C%10%F3%05%2C%15%90%AA%DF%5E.%20\
                          &peer_id=ABCDEFGHIJKLMNOPQRST&port=6881&uploaded=0&downloaded=0\
                          &left=727955456&event=started&numwant=100&no_peer_id=1&compact=1";

        assert!(
            AnnounceRequest::new(url_string).is_ok(),
            "Announce request creation failed"
        );
    }

    #[test]
    fn announce_bad_request_creation() {
        let url_string =
            "http://tracker/announce?\
             info_hash=%9A%813%3C%1B%16%E4%A8%3C%10%F3%05%2C%15%90%AA%DF%5E.%20\
             &peer_id=ABCDEFGHIJKLMNOPQRST&port=thisisnotanumber&uploaded=0&downloaded=0\
             &left=727955456&event=started&numwant=100&no_peer_id=1&compact=thisisnotanumber";

        assert!(
            AnnounceRequest::new(url_string).is_err(),
            "Incorrect announce request parameter parsing"
        );
    }

    #[test]
    fn event_good_value_parse() {
        let s = "started".to_string();
        assert_eq!(string_to_event(s), Event::Started);
    }

    #[test]
    fn event_garbage_value_to_none() {
        let s = "garbage".to_string();
        assert_eq!(string_to_event(s), Event::None);
    }

    #[test]
    fn response_failure_return() {
        let failure_reason = "It's not you...no, it's just you".to_string();
        let response = AnnounceResponse::fail(failure_reason);
        assert_eq!(
            response.failure_reason,
            "It's not you...no, it's just you".to_string()
        );
    }

    #[test]
    fn peerv4_compact_transform() {
        let peer = Peerv4 {
            peer_id: "ABCDEFGHIJKLMNOPQRST".to_string(),
            ip: Ipv4Addr::LOCALHOST,
            port: 6681,
        };

        let mut localhost_port_byte_string = vec![];
        localhost_port_byte_string.put_u32_be(2130706433); // localhost in decimal
        localhost_port_byte_string.put_u16_be(6681);

        let compact_rep_byte_string = peer.compact();

        assert_eq!(compact_rep_byte_string, localhost_port_byte_string.to_vec());
    }

    #[test]
    fn peerv6_compact_transform() {
        let peer = Peerv6 {
            peer_id: "ABCDEFGHIJKLMNOPQRST".to_string(),
            ip: Ipv6Addr::new(
                0x2001, 0x0db8, 0x85a3, 0x0000, 0x0000, 0x8a2e, 0x0370, 0x7334,
            ),
            port: 6681,
        };

        let mut localhost_port_byte_string = vec![];
        let localhost_decimal = 42540766452641154071740215577757643572 as u128;
        let port = 6681 as u16;
        localhost_port_byte_string.put_slice(&localhost_decimal.to_be_bytes());
        localhost_port_byte_string.put_slice(&port.to_be_bytes());

        let compact_rep_byte_string = peer.compact();

        assert_eq!(compact_rep_byte_string, localhost_port_byte_string.to_vec());
    }

    #[test]
    fn scrape_good_request_creation() {
        let url_string = "http://example.com/scrape.php?info_hash=aaaaaaaaaaaaaaaaaaaa&info_hash=bbbbbbbbbbbbbbbbbbbb&info_hash=cccccccccccccccccccc";

        assert!(
            ScrapeRequest::new(url_string).is_ok(),
            "Scrape request creation failed"
        );
    }

    #[test]
    fn scrape_good_request_multiple_hashes() {
        let url_string = "http://example.com/scrape.php?info_hash=aaaaaaaaaaaaaaaaaaaa&info_hash=bbbbbbbbbbbbbbbbbbbb&info_hash=cccccccccccccccccccc";
        let scrape = ScrapeRequest::new(url_string).unwrap();
        assert_eq!(
            scrape.info_hashes,
            vec![
                "aaaaaaaaaaaaaaaaaaaa",
                "bbbbbbbbbbbbbbbbbbbb",
                "cccccccccccccccccccc"
            ]
        );
    }

    #[test]
    fn scrape_bad_request_creation() {
        let url_string = "http://example.com/scrape.php?info_hash=aaaaaaaaaaaaaaaaaaaa&info_bash=bbbbbbbbbbbbbbbbbbbb&info_slash=cccccccccccccccccccc";

        assert!(
            ScrapeRequest::new(url_string).is_err(),
            "Incorrect scrape request parsing"
        );
    }

    #[test]
    fn scrape_response_add_file() {
        let file = ScrapeFile::default();
        let mut scrape_response = ScrapeResponse::new().unwrap();
        scrape_response.add_file(file);

        assert_eq!(scrape_response.files.len(), 1);
    }
}
