//! Minimal DNS wire-format codec for A and AAAA queries and responses.
//!
//! We deliberately support just enough of RFC 1035 / RFC 3596 to ask
//! "what's the A/AAAA record for foo.svc.cluster.local?" and parse
//! CoreDNS's reply. No CNAME chasing (CoreDNS always inlines the
//! final A/AAAA alongside the CNAME), no DNSSEC, no EDNS0.
//!
//! The encoder always sets the RD (recursion desired) bit so CoreDNS
//! follows its upstream chain. The decoder tolerates compression
//! pointers in answer names (§4.1.4) because CoreDNS always uses them.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// DNS header is a fixed 12 bytes.
const HEADER_LEN: usize = 12;

/// RR type A (IPv4 address).
pub const TYPE_A: u16 = 1;
/// RR type AAAA (IPv6 address), RFC 3596.
pub const TYPE_AAAA: u16 = 28;
/// RR class IN (Internet).
const CLASS_IN: u16 = 1;

/// Maximum length of a single DNS label (RFC 1035 §2.3.4).
const MAX_LABEL_LEN: usize = 63;

/// Maximum total length of an encoded domain name in a DNS message.
const MAX_NAME_LEN: usize = 255;

/// Maximum number of name-compression pointer hops to follow when
/// decoding. RFC 1035 doesn't give a number but this guards against
/// adversarial loops.
const MAX_POINTER_HOPS: usize = 16;

/// Failures decoding a DNS wire-format message.
#[derive(Debug, thiserror::Error)]
pub enum DnsError {
    #[error("DNS message shorter than header ({0} < {HEADER_LEN})")]
    TruncatedHeader(usize),
    #[error("DNS message truncated at offset {0}")]
    Truncated(usize),
    #[error("DNS name too long ({0} > {MAX_NAME_LEN})")]
    NameTooLong(usize),
    #[error("DNS label too long ({0} > {MAX_LABEL_LEN})")]
    LabelTooLong(usize),
    #[error("DNS label contains invalid character {0:?}")]
    InvalidLabelChar(char),
    #[error("DNS pointer loop exceeded {MAX_POINTER_HOPS} hops")]
    PointerLoop,
    #[error("DNS response has no {0} answer")]
    NoAnswer(&'static str),
    #[error("DNS response id mismatch: expected {expected:#06x}, got {got:#06x}")]
    IdMismatch { expected: u16, got: u16 },
    #[error("DNS response rcode {0}")]
    Rcode(u8),
    #[error("DNS response flags indicate question, not response")]
    NotResponse,
    #[error("DNS answer has malformed rdata length")]
    BadRdLength,
    #[error("DNS query type {0} not supported by encoder")]
    UnsupportedQueryType(u16),
}

/// Build a query for `name` with the given transaction id and record
/// type. Returns the wire bytes *without* the 2-byte TCP length
/// prefix — the caller is responsible for framing.
pub fn encode_query(id: u16, name: &str, qtype: u16) -> Result<Vec<u8>, DnsError> {
    if qtype != TYPE_A && qtype != TYPE_AAAA {
        return Err(DnsError::UnsupportedQueryType(qtype));
    }

    let mut buf = Vec::with_capacity(HEADER_LEN + name.len() + 6);

    // Header: id, flags, qd=1, an/ns/ar=0
    buf.extend_from_slice(&id.to_be_bytes());
    // QR=0, Opcode=0, AA=0, TC=0, RD=1, RA=0, Z=0, RCODE=0
    buf.extend_from_slice(&0x0100u16.to_be_bytes());
    buf.extend_from_slice(&1u16.to_be_bytes()); // qdcount
    buf.extend_from_slice(&0u16.to_be_bytes()); // ancount
    buf.extend_from_slice(&0u16.to_be_bytes()); // nscount
    buf.extend_from_slice(&0u16.to_be_bytes()); // arcount

    encode_name(&mut buf, name)?;

    buf.extend_from_slice(&qtype.to_be_bytes());
    buf.extend_from_slice(&CLASS_IN.to_be_bytes());

    Ok(buf)
}

/// An A or AAAA record extracted from a response, with its TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IpRecord {
    pub addr: IpAddr,
    pub ttl: u32,
}

/// Parse a DNS response and return the first answer matching the
/// requested record type (A or AAAA). Validates that the id matches
/// the query's and that RCODE is 0 (NOERROR).
///
/// CoreDNS responses often contain both CNAME and A/AAAA in the
/// answer section for service aliases; this function skips past
/// CNAMEs (and any other non-matching records) and returns the
/// first A/AAAA it finds.
pub fn decode_first_ip_record(
    msg: &[u8],
    expected_id: u16,
    qtype: u16,
) -> Result<IpRecord, DnsError> {
    if msg.len() < HEADER_LEN {
        return Err(DnsError::TruncatedHeader(msg.len()));
    }
    let id = u16::from_be_bytes([msg[0], msg[1]]);
    if id != expected_id {
        return Err(DnsError::IdMismatch {
            expected: expected_id,
            got: id,
        });
    }
    let flags = u16::from_be_bytes([msg[2], msg[3]]);
    if flags & 0x8000 == 0 {
        return Err(DnsError::NotResponse);
    }
    let rcode = (flags & 0x000F) as u8;
    if rcode != 0 {
        return Err(DnsError::Rcode(rcode));
    }

    let qdcount = u16::from_be_bytes([msg[4], msg[5]]);
    let ancount = u16::from_be_bytes([msg[6], msg[7]]);
    // nscount / arcount at [8..12] — not consulted.

    if ancount == 0 {
        return Err(DnsError::NoAnswer(record_type_name(qtype)));
    }

    // Skip the question section.
    let mut offset = HEADER_LEN;
    for _ in 0..qdcount {
        offset = skip_name(msg, offset)?;
        // qtype (2) + qclass (2)
        if offset + 4 > msg.len() {
            return Err(DnsError::Truncated(offset));
        }
        offset += 4;
    }

    // Walk answer RRs until we find the first matching type.
    for _ in 0..ancount {
        offset = skip_name(msg, offset)?;
        if offset + 10 > msg.len() {
            return Err(DnsError::Truncated(offset));
        }
        let rtype = u16::from_be_bytes([msg[offset], msg[offset + 1]]);
        let _rclass = u16::from_be_bytes([msg[offset + 2], msg[offset + 3]]);
        let ttl = u32::from_be_bytes([
            msg[offset + 4],
            msg[offset + 5],
            msg[offset + 6],
            msg[offset + 7],
        ]);
        let rdlength = u16::from_be_bytes([msg[offset + 8], msg[offset + 9]]) as usize;
        offset += 10;
        if offset + rdlength > msg.len() {
            return Err(DnsError::Truncated(offset));
        }
        if rtype == qtype {
            let rec = parse_rdata(qtype, &msg[offset..offset + rdlength], ttl)?;
            return Ok(rec);
        }
        offset += rdlength;
    }

    Err(DnsError::NoAnswer(record_type_name(qtype)))
}

fn parse_rdata(qtype: u16, rdata: &[u8], ttl: u32) -> Result<IpRecord, DnsError> {
    match qtype {
        TYPE_A => {
            if rdata.len() != 4 {
                return Err(DnsError::BadRdLength);
            }
            Ok(IpRecord {
                addr: IpAddr::V4(Ipv4Addr::new(rdata[0], rdata[1], rdata[2], rdata[3])),
                ttl,
            })
        }
        TYPE_AAAA => {
            if rdata.len() != 16 {
                return Err(DnsError::BadRdLength);
            }
            let mut octets = [0u8; 16];
            octets.copy_from_slice(rdata);
            Ok(IpRecord {
                addr: IpAddr::V6(Ipv6Addr::from(octets)),
                ttl,
            })
        }
        other => Err(DnsError::UnsupportedQueryType(other)),
    }
}

fn record_type_name(qtype: u16) -> &'static str {
    match qtype {
        TYPE_A => "A",
        TYPE_AAAA => "AAAA",
        _ => "?",
    }
}

/// Encode a domain name into DNS wire format (length-prefixed labels
/// followed by a zero byte).
fn encode_name(buf: &mut Vec<u8>, name: &str) -> Result<(), DnsError> {
    let trimmed = name.strip_suffix('.').unwrap_or(name);
    if trimmed.is_empty() {
        buf.push(0);
        return Ok(());
    }

    let start = buf.len();
    for label in trimmed.split('.') {
        if label.is_empty() {
            return Err(DnsError::LabelTooLong(0));
        }
        if label.len() > MAX_LABEL_LEN {
            return Err(DnsError::LabelTooLong(label.len()));
        }
        for c in label.chars() {
            if !(c.is_ascii_alphanumeric() || c == '-' || c == '_') {
                return Err(DnsError::InvalidLabelChar(c));
            }
        }
        buf.push(label.len() as u8);
        buf.extend_from_slice(label.as_bytes());
    }
    buf.push(0);

    if buf.len() - start > MAX_NAME_LEN {
        return Err(DnsError::NameTooLong(buf.len() - start));
    }
    Ok(())
}

/// Advance past a name starting at `offset`, returning the offset of
/// the first byte after the name. Handles compression pointers by
/// stopping at the pointer — we only need the final position in the
/// current stream, not the pointed-at bytes.
fn skip_name(msg: &[u8], mut offset: usize) -> Result<usize, DnsError> {
    let mut hops = 0;
    loop {
        if offset >= msg.len() {
            return Err(DnsError::Truncated(offset));
        }
        let len = msg[offset];
        if len == 0 {
            return Ok(offset + 1);
        }
        if len & 0xC0 == 0xC0 {
            // Pointer: 2 bytes total, we don't follow it, just skip past.
            if offset + 2 > msg.len() {
                return Err(DnsError::Truncated(offset));
            }
            return Ok(offset + 2);
        }
        if len & 0xC0 != 0 {
            // Reserved / EDNS — we don't speak it.
            return Err(DnsError::Truncated(offset));
        }
        let next = offset + 1 + len as usize;
        if next > msg.len() {
            return Err(DnsError::Truncated(offset));
        }
        offset = next;
        hops += 1;
        if hops > MAX_POINTER_HOPS {
            return Err(DnsError::PointerLoop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_simple_a_query() {
        let q = encode_query(0x1234, "foo.example.com", TYPE_A).unwrap();
        assert_eq!(&q[0..2], &[0x12, 0x34]);
        assert_eq!(&q[2..4], &[0x01, 0x00]);
        assert_eq!(&q[4..6], &[0x00, 0x01]);
        assert_eq!(q[12], 3);
        assert_eq!(&q[13..16], b"foo");
        assert_eq!(q[16], 7);
        assert_eq!(&q[17..24], b"example");
        assert_eq!(q[24], 3);
        assert_eq!(&q[25..28], b"com");
        assert_eq!(q[28], 0);
        assert_eq!(&q[29..31], &[0x00, 0x01]); // A
        assert_eq!(&q[31..33], &[0x00, 0x01]); // IN
    }

    #[test]
    fn encode_simple_aaaa_query() {
        let q = encode_query(0x9999, "foo.example", TYPE_AAAA).unwrap();
        assert_eq!(&q[0..2], &[0x99, 0x99]);
        // qtype at the very end of the message should be AAAA (28 = 0x1c).
        assert_eq!(&q[q.len() - 4..q.len() - 2], &[0x00, 0x1c]);
        assert_eq!(&q[q.len() - 2..], &[0x00, 0x01]);
    }

    #[test]
    fn encode_trailing_dot_stripped() {
        let q = encode_query(1, "foo.", TYPE_A).unwrap();
        assert_eq!(q[12], 3);
        assert_eq!(&q[13..16], b"foo");
        assert_eq!(q[16], 0);
    }

    #[test]
    fn encode_kubernetes_service() {
        let q = encode_query(0xabcd, "hydra.ory.svc.cluster.local", TYPE_A).unwrap();
        let question = &q[12..];
        assert_eq!(question[0], 5);
        assert_eq!(&question[1..6], b"hydra");
    }

    #[test]
    fn encode_rejects_empty_label() {
        let err = encode_query(1, "foo..bar", TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::LabelTooLong(0)));
    }

    #[test]
    fn encode_rejects_label_over_63() {
        let long = "a".repeat(64);
        let err = encode_query(1, &long, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::LabelTooLong(64)));
    }

    #[test]
    fn encode_rejects_invalid_chars() {
        let err = encode_query(1, "foo!bar", TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::InvalidLabelChar('!')));
    }

    #[test]
    fn encode_allows_underscore() {
        encode_query(1, "srv_tcp.example", TYPE_A).unwrap();
    }

    #[test]
    fn encode_empty_name_is_root() {
        let q = encode_query(1, "", TYPE_A).unwrap();
        assert_eq!(q[12], 0);
    }

    #[test]
    fn encode_rejects_unsupported_qtype() {
        let err = encode_query(1, "foo", 15).unwrap_err(); // MX
        assert!(matches!(err, DnsError::UnsupportedQueryType(15)));
    }

    /// Helper: build a minimal DNS response with one record in the answer.
    fn mk_response(id: u16, name: &str, rtype: u16, rdata: &[u8], ttl: u32) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&id.to_be_bytes());
        msg.extend_from_slice(&0x8180u16.to_be_bytes()); // QR=1, RD=1, RA=1, RCODE=0
        msg.extend_from_slice(&1u16.to_be_bytes()); // qdcount
        msg.extend_from_slice(&1u16.to_be_bytes()); // ancount
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());

        // Question: name + rtype + IN
        let qname_start = msg.len();
        encode_name(&mut msg, name).unwrap();
        msg.extend_from_slice(&rtype.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());

        // Answer: compression pointer to the question name
        let pointer = (0xC000 | qname_start as u16).to_be_bytes();
        msg.extend_from_slice(&pointer);
        msg.extend_from_slice(&rtype.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        msg.extend_from_slice(&ttl.to_be_bytes());
        msg.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
        msg.extend_from_slice(rdata);

        msg
    }

    #[test]
    fn decode_a_happy_path() {
        let resp = mk_response(0xbeef, "hydra.ory.svc.cluster.local", TYPE_A, &[10, 43, 1, 23], 30);
        let rec = decode_first_ip_record(&resp, 0xbeef, TYPE_A).unwrap();
        assert_eq!(rec.addr, IpAddr::V4(Ipv4Addr::new(10, 43, 1, 23)));
        assert_eq!(rec.ttl, 30);
    }

    #[test]
    fn decode_aaaa_happy_path() {
        let v6: [u8; 16] = [
            0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0, 0xab, 0x12, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x01,
        ];
        let resp = mk_response(0xbeef, "svc.cluster.local", TYPE_AAAA, &v6, 60);
        let rec = decode_first_ip_record(&resp, 0xbeef, TYPE_AAAA).unwrap();
        assert_eq!(rec.addr, IpAddr::V6(Ipv6Addr::from(v6)));
        assert_eq!(rec.ttl, 60);
    }

    #[test]
    fn decode_rejects_id_mismatch() {
        let resp = mk_response(1, "foo", TYPE_A, &[1, 2, 3, 4], 60);
        let err = decode_first_ip_record(&resp, 2, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::IdMismatch { expected: 2, got: 1 }));
    }

    #[test]
    fn decode_rejects_rcode_nxdomain() {
        let mut resp = mk_response(1, "foo", TYPE_A, &[0, 0, 0, 0], 0);
        resp[3] |= 0x03;
        let err = decode_first_ip_record(&resp, 1, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::Rcode(3)));
    }

    #[test]
    fn decode_rejects_truncated_header() {
        let resp = vec![0u8; 5];
        let err = decode_first_ip_record(&resp, 0, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::TruncatedHeader(5)));
    }

    #[test]
    fn decode_rejects_not_a_response() {
        let mut resp = mk_response(1, "foo", TYPE_A, &[1, 2, 3, 4], 60);
        resp[2] = 0x01;
        let err = decode_first_ip_record(&resp, 1, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::NotResponse));
    }

    #[test]
    fn decode_no_answer_a() {
        let mut msg = Vec::new();
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0x8180u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        encode_name(&mut msg, "foo").unwrap();
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());

        let err = decode_first_ip_record(&msg, 1, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::NoAnswer("A")));
    }

    #[test]
    fn decode_no_answer_aaaa_when_only_a_present() {
        // Dual-stack service where only A records exist: the AAAA
        // decode should cleanly return NoAnswer("AAAA"), not claim the
        // A record.
        let resp = mk_response(1, "foo", TYPE_A, &[1, 2, 3, 4], 60);
        let err = decode_first_ip_record(&resp, 1, TYPE_AAAA).unwrap_err();
        assert!(matches!(err, DnsError::NoAnswer("AAAA")));
    }

    #[test]
    fn decode_skips_non_matching_records() {
        // Hand-build: CNAME answer first, then an AAAA. Querying for
        // AAAA should skip the CNAME.
        let mut msg = Vec::new();
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0x8180u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&2u16.to_be_bytes()); // ancount=2
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());

        let qname_start = msg.len();
        encode_name(&mut msg, "foo.example").unwrap();
        msg.extend_from_slice(&TYPE_AAAA.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());

        let ptr = (0xC000 | qname_start as u16).to_be_bytes();
        // CNAME answer (type 5).
        msg.extend_from_slice(&ptr);
        msg.extend_from_slice(&5u16.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        msg.extend_from_slice(&60u32.to_be_bytes());
        msg.extend_from_slice(&5u16.to_be_bytes());
        msg.extend_from_slice(&[3, b'b', b'a', b'r', 0]);

        // AAAA answer
        let v6: [u8; 16] = [
            0xfd, 0x7a, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x05,
        ];
        msg.extend_from_slice(&ptr);
        msg.extend_from_slice(&TYPE_AAAA.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        msg.extend_from_slice(&120u32.to_be_bytes());
        msg.extend_from_slice(&16u16.to_be_bytes());
        msg.extend_from_slice(&v6);

        let rec = decode_first_ip_record(&msg, 1, TYPE_AAAA).unwrap();
        assert_eq!(rec.addr, IpAddr::V6(Ipv6Addr::from(v6)));
        assert_eq!(rec.ttl, 120);
    }

    #[test]
    fn decode_handles_compression_pointer_in_answer() {
        let resp = mk_response(42, "a.b.c", TYPE_A, &[172, 16, 0, 5], 300);
        let rec = decode_first_ip_record(&resp, 42, TYPE_A).unwrap();
        assert_eq!(rec.addr, IpAddr::V4(Ipv4Addr::new(172, 16, 0, 5)));
    }

    #[test]
    fn decode_rejects_bad_rdlength_for_a_record() {
        let mut msg = Vec::new();
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0x8180u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&1u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        msg.extend_from_slice(&0u16.to_be_bytes());
        let qname_start = msg.len();
        encode_name(&mut msg, "foo").unwrap();
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());

        let ptr = (0xC000 | qname_start as u16).to_be_bytes();
        msg.extend_from_slice(&ptr);
        msg.extend_from_slice(&TYPE_A.to_be_bytes());
        msg.extend_from_slice(&CLASS_IN.to_be_bytes());
        msg.extend_from_slice(&30u32.to_be_bytes());
        msg.extend_from_slice(&6u16.to_be_bytes()); // wrong!
        msg.extend_from_slice(&[1, 2, 3, 4, 5, 6]);

        let err = decode_first_ip_record(&msg, 1, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::BadRdLength));
    }

    #[test]
    fn decode_rejects_bad_rdlength_for_aaaa_record() {
        // AAAA rdata must be exactly 16 bytes.
        let resp = mk_response(1, "foo", TYPE_AAAA, &[0u8; 8], 30);
        let err = decode_first_ip_record(&resp, 1, TYPE_AAAA).unwrap_err();
        assert!(matches!(err, DnsError::BadRdLength));
    }

    #[test]
    fn decode_rejects_truncated_answer() {
        let resp = mk_response(1, "foo", TYPE_A, &[1, 2, 3, 4], 60);
        let truncated = &resp[..resp.len() - 2];
        let err = decode_first_ip_record(truncated, 1, TYPE_A).unwrap_err();
        assert!(matches!(err, DnsError::Truncated(_)));
    }
}
