// This file is part of masscanned.
// Copyright 2021 - The IVRE project
//
// Masscanned is free software: you can redistribute it and/or modify it
// under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// Masscanned is distributed in the hope that it will be useful, but WITHOUT
// ANY WARRANTY; without even the implied warranty of MERCHANTABILITY
// or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU General Public
// License for more details.
//
// You should have received a copy of the GNU General Public License
// along with Masscanned. If not, see <http://www.gnu.org/licenses/>.

use log::*;

use pnet::packet::{
    tcp::{MutableTcpPacket, TcpFlags, TcpPacket},
    Packet,
};

use crate::client::ClientInfo;
use crate::proto;
use crate::synackcookie;
use crate::Masscanned;

pub fn repl<'a, 'b>(
    tcp_req: &'a TcpPacket,
    masscanned: &Masscanned,
    mut client_info: &mut ClientInfo,
) -> Option<MutableTcpPacket<'b>> {
    debug!("receiving TCP packet: {:?}", tcp_req);
    /* Fill client info with source and dest. TCP port */
    client_info.port.src = Some(tcp_req.get_source());
    client_info.port.dst = Some(tcp_req.get_destination());
    /* Construct response TCP packet */
    let mut tcp_repl;
    match tcp_req.get_flags() {
        /* Answer to data */
        flags if flags & (TcpFlags::PSH | TcpFlags::ACK) == (TcpFlags::PSH | TcpFlags::ACK) => {
            /* First check the synack cookie */
            let ackno = if tcp_req.get_acknowledgement() > 0 {
                tcp_req.get_acknowledgement() - 1
            } else {
                /* underflow hack */
                0xFFFFFFFF
            };
            /* Compute syncookie */
            if let Ok(cookie) = synackcookie::generate(&client_info, &masscanned.synack_key) {
                if cookie != ackno {
                    info!("PSH-ACK ignored: synackcookie not valid");
                    return None;
                }
                client_info.cookie = Some(cookie);
            }
            warn!("ACK to PSH-ACK on port {}", tcp_req.get_destination());
            let payload = tcp_req.payload();
            /* Any answer to upper-layer protocol? */
            if let Some(repl) = proto::repl(&payload, masscanned, &mut client_info) {
                tcp_repl = MutableTcpPacket::owned(
                    [vec![0; MutableTcpPacket::minimum_packet_size()], repl].concat(),
                )
                .expect("error constructing a TCP packet");
                tcp_repl.set_flags(TcpFlags::ACK | TcpFlags::PSH);
            } else {
                tcp_repl =
                    MutableTcpPacket::owned(vec![0; MutableTcpPacket::minimum_packet_size()])
                        .expect("error constructing a TCP packet");
                tcp_repl.set_flags(TcpFlags::ACK);
            }
            tcp_repl.set_acknowledgement(tcp_req.get_sequence() + (tcp_req.payload().len() as u32));
            tcp_repl.set_sequence(tcp_req.get_acknowledgement());
        }
        /* Answer to ACK: nothing */
        flags if flags == TcpFlags::ACK => {
            /* answer here when server needs to speak first after handshake */
            return None;
        }
        /* Answer to RST and FIN: nothing */
        flags if (flags == TcpFlags::RST || flags == (TcpFlags::FIN | TcpFlags::ACK)) => {
            return None;
        }
        /* Answer to SYN */
        flags if flags & TcpFlags::SYN == TcpFlags::SYN => {
            tcp_repl = MutableTcpPacket::owned(vec![0; MutableTcpPacket::minimum_packet_size()])
                .expect("error constructing a TCP packet");
            tcp_repl.set_flags(TcpFlags::ACK);
            tcp_repl.set_flags(TcpFlags::SYN | TcpFlags::ACK);
            tcp_repl.set_acknowledgement(tcp_req.get_sequence() + 1);
            /* generate a SYNACK-cookie (same as masscan) */
            tcp_repl.set_sequence(
                synackcookie::generate(&client_info, &masscanned.synack_key).unwrap(),
            );
            warn!("SYN-ACK to ACK on port {}", tcp_req.get_destination());
        }
        _ => {
            info!("TCP flag not handled: {}", tcp_req.get_flags());
            return None;
        }
    }
    /* Set source and dest. port for response packet from client info */
    /* Note: client info could have been modified by upper layers (e.g., STUN) */
    tcp_repl.set_source(client_info.port.dst.unwrap());
    tcp_repl.set_destination(client_info.port.src.unwrap());
    /* Set TCP headers */
    tcp_repl.set_data_offset(5);
    tcp_repl.set_window(65535);
    debug!("sending TCP packet: {:?}", tcp_repl);
    Some(tcp_repl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::ClientInfoSrcDst;
    use pnet::util::MacAddr;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_synack_cookie_ipv4() {
        let masscanned = Masscanned {
            mac: MacAddr(0, 0, 0, 0, 0, 0),
            ip_addresses: None,
            synack_key: [0x06a0a1d63f305e9b, 0xd4d4bcbb7304875f],
            iface: None,
        };
        /* reference */
        let ip_src = IpAddr::V4(Ipv4Addr::new(27, 198, 143, 1));
        let ip_dst = IpAddr::V4(Ipv4Addr::new(90, 64, 122, 203));
        let tcp_sport = 65000;
        let tcp_dport = 80;
        let mut client_info = ClientInfo {
            mac: ClientInfoSrcDst {
                src: None,
                dst: None,
            },
            ip: ClientInfoSrcDst {
                src: Some(ip_src),
                dst: Some(ip_dst),
            },
            transport: None,
            port: ClientInfoSrcDst {
                src: Some(tcp_sport),
                dst: Some(tcp_dport),
            },
            cookie: None,
        };
        let cookie = synackcookie::generate(&client_info, &masscanned.synack_key).unwrap();
        let mut tcp_req =
            MutableTcpPacket::owned(vec![0; MutableTcpPacket::minimum_packet_size()]).unwrap();
        tcp_req.set_source(tcp_sport);
        tcp_req.set_destination(tcp_dport);
        tcp_req.set_flags(TcpFlags::SYN);
        let some_tcp_repl = repl(&tcp_req.to_immutable(), &masscanned, &mut client_info);
        if some_tcp_repl == None {
            assert!(false);
            return;
        }
        let tcp_repl = some_tcp_repl.unwrap();
        assert!(synackcookie::_check(
            &client_info,
            tcp_repl.get_sequence(),
            &masscanned.synack_key
        ));
        assert!(cookie == tcp_repl.get_sequence());
    }

    #[test]
    fn test_synack_cookie_ipv6() {
        let masscanned = Masscanned {
            mac: MacAddr(0, 0, 0, 0, 0, 0),
            ip_addresses: None,
            synack_key: [0x06a0a1d63f305e9b, 0xd4d4bcbb7304875f],
            iface: None,
        };
        /* reference */
        let ip_src = IpAddr::V6(Ipv6Addr::new(234, 52, 183, 47, 184, 172, 64, 141));
        let ip_dst = IpAddr::V6(Ipv6Addr::new(25, 179, 227, 231, 53, 216, 45, 144));
        let tcp_sport = 65000;
        let tcp_dport = 80;
        let mut client_info = ClientInfo {
            mac: ClientInfoSrcDst {
                src: None,
                dst: None,
            },
            ip: ClientInfoSrcDst {
                src: Some(ip_src),
                dst: Some(ip_dst),
            },
            transport: None,
            port: ClientInfoSrcDst {
                src: Some(tcp_sport),
                dst: Some(tcp_dport),
            },
            cookie: None,
        };
        let cookie = synackcookie::generate(&client_info, &masscanned.synack_key).unwrap();
        let mut tcp_req =
            MutableTcpPacket::owned(vec![0; MutableTcpPacket::minimum_packet_size()]).unwrap();
        tcp_req.set_source(tcp_sport);
        tcp_req.set_destination(tcp_dport);
        tcp_req.set_flags(TcpFlags::SYN);
        let some_tcp_repl = repl(&tcp_req.to_immutable(), &masscanned, &mut client_info);
        if some_tcp_repl == None {
            assert!(false);
            return;
        }
        let tcp_repl = some_tcp_repl.unwrap();
        assert!(synackcookie::_check(
            &client_info,
            tcp_repl.get_sequence(),
            &masscanned.synack_key
        ));
        assert!(cookie == tcp_repl.get_sequence());
    }
}
