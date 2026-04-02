use std::net::SocketAddr;

use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, SOA};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use tokio::net::UdpSocket;
use tracing::{debug, error, warn};

use crate::dns_store::DnsStore;

/// Run a UDP DNS server that answers A and SOA queries for records in the zone.
pub async fn run(addr: SocketAddr, zone: String, store: DnsStore) -> anyhow::Result<()> {
    let socket = UdpSocket::bind(addr).await?;
    tracing::info!("DNS server listening on {addr}");

    let zone_name = Name::from_ascii(&zone)?;

    let mut buf = vec![0u8; 512];
    loop {
        let (len, src) = socket.recv_from(&mut buf).await?;
        let query = match Message::from_vec(&buf[..len]) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("failed to parse DNS message from {src}: {e}");
                continue;
            }
        };

        let response = handle_query(&query, &zone_name, &store).await;

        let response_bytes = match response.to_vec() {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("failed to serialize DNS response: {e}");
                continue;
            }
        };

        if let Err(e) = socket.send_to(&response_bytes, src).await {
            error!("failed to send DNS response to {src}: {e}");
        }
    }
}

async fn handle_query(query: &Message, zone_name: &Name, store: &DnsStore) -> Message {
    let mut response = Message::new();
    response.set_id(query.id());
    response.set_message_type(MessageType::Response);
    response.set_op_code(OpCode::Query);
    response.set_recursion_desired(query.recursion_desired());
    response.set_recursion_available(false);
    response.set_authoritative(true);

    let Some(question) = query.queries().first() else {
        response.set_response_code(ResponseCode::FormErr);
        return response;
    };

    response.add_query(question.clone());

    let qname = question.name();
    let qtype = question.query_type();

    // Verify query is within our zone
    if !zone_name.zone_of(qname) {
        response.set_response_code(ResponseCode::Refused);
        return response;
    }

    match qtype {
        RecordType::A => handle_a_query(&mut response, qname, zone_name, store).await,
        RecordType::SOA if qname == zone_name => {
            add_soa(&mut response, zone_name, false);
            response.set_response_code(ResponseCode::NoError);
        }
        _ => {
            response.set_response_code(ResponseCode::NXDomain);
            add_soa(&mut response, zone_name, true);
        }
    }

    response
}

async fn handle_a_query(response: &mut Message, qname: &Name, zone_name: &Name, store: &DnsStore) {
    // Normalize: strip trailing dot for Redis lookup
    let fqdn = qname.to_ascii().trim_end_matches('.').to_lowercase();

    debug!(fqdn = %fqdn, "DNS A query");

    match store.get_a_record(&fqdn).await {
        Ok(Some(data)) => {
            for addr_str in &data.addresses {
                if let Ok(addr) = addr_str.parse::<std::net::Ipv4Addr>() {
                    let record =
                        Record::from_rdata(qname.clone(), data.ttl_seconds, RData::A(A(addr)));
                    response.add_answer(record);
                } else {
                    warn!(address = %addr_str, "invalid IPv4 address in DNS record");
                }
            }
            response.set_response_code(ResponseCode::NoError);
        }
        Ok(None) => {
            debug!(fqdn = %fqdn, "DNS record not found");
            response.set_response_code(ResponseCode::NXDomain);
            add_soa(response, zone_name, true);
        }
        Err(e) => {
            error!(fqdn = %fqdn, error = %e, "Redis lookup failed");
            response.set_response_code(ResponseCode::ServFail);
        }
    }
}

fn add_soa(response: &mut Message, zone_name: &Name, authority: bool) {
    let soa = SOA::new(
        Name::from_ascii("ns1")
            .unwrap()
            .append_domain(zone_name)
            .unwrap(),
        Name::from_ascii("admin")
            .unwrap()
            .append_domain(zone_name)
            .unwrap(),
        1,      // serial
        3600,   // refresh
        900,    // retry
        604800, // expire
        60,     // minimum TTL
    );
    let record = Record::from_rdata(zone_name.clone(), 3600, RData::SOA(soa));
    if authority {
        response.add_name_server(record);
    } else {
        response.add_answer(record);
    }
}
