use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::os::fd::{FromRawFd, RawFd};
use std::thread;

use nix::sys::socket::SockType;
use nix::sys::socket::sockopt::SockType as SockTypeOpt;

use crate::error::RunError;

#[cfg(any(target_os = "macos", test))]
pub(crate) mod host;

/// Lib-level input for [`execute_dns_stub`]. The CLI layer builds this
/// from its `clap`-derived args struct.
#[derive(Debug, Clone)]
pub struct DnsStubInput {
    /// UDP/TCP DNS listen address reachable by the sandboxed agent process.
    /// Ignored when [`Self::inherited_udp_fd`]/[`Self::inherited_tcp_fd`]
    /// are both set.
    pub listen: SocketAddr,
    /// A UDP socket already bound (at `listen`) and inherited across
    /// `exec` from a trusted parent process — `HakoniwaBackend` only, see
    /// `DEC-012` in `docs/architecture/hakoniwa-backend-plan.md`. Must be
    /// set together with [`Self::inherited_tcp_fd`] or not at all;
    /// [`execute_dns_stub`] rejects the mixed case rather than guessing.
    pub inherited_udp_fd: Option<RawFd>,
    /// As [`Self::inherited_udp_fd`], for the TCP listener.
    pub inherited_tcp_fd: Option<RawFd>,
}

const DNS_HEADER_LEN: usize = 12;
const DNS_RCODE_REFUSED: u8 = 5;

/// Run the internal sandbox-local DNS stub.
///
/// The stub provides an explicit resolver endpoint for structurally confined
/// bwrap sandboxes. It refuses all queries deterministically instead of
/// forwarding to the host ambient resolver.
///
/// # Errors
///
/// Returns an error if UDP or TCP DNS listeners cannot bind (or, when
/// `inherited_udp_fd`/`inherited_tcp_fd` are set, if either fd is not a
/// valid socket of the expected type), or if exactly one of
/// `inherited_udp_fd`/`inherited_tcp_fd` is set without the other.
pub fn execute_dns_stub(args: &DnsStubInput) -> Result<i32, RunError> {
    let (udp, tcp) = match (args.inherited_udp_fd, args.inherited_tcp_fd) {
        (Some(udp_fd), Some(tcp_fd)) => (
            inherited_udp_socket(udp_fd)?,
            inherited_tcp_listener(tcp_fd)?,
        ),
        (None, None) => (
            UdpSocket::bind(args.listen).map_err(|error| {
                RunError::Spawn(format!(
                    "failed to bind sandbox DNS UDP stub at {}: {error}",
                    args.listen
                ))
            })?,
            TcpListener::bind(args.listen).map_err(|error| {
                RunError::Spawn(format!(
                    "failed to bind sandbox DNS TCP stub at {}: {error}",
                    args.listen
                ))
            })?,
        ),
        (udp_fd, tcp_fd) => {
            return Err(RunError::Spawn(format!(
                "DNS stub requires both --inherited-udp-fd and --inherited-tcp-fd together, \
                 or neither; got udp={udp_fd:?} tcp={tcp_fd:?}"
            )));
        }
    };

    thread::Builder::new()
        .name("firma-run-dns-udp".to_string())
        .spawn(move || run_udp(&udp))
        .map_err(|error| RunError::Spawn(format!("failed to spawn DNS UDP stub: {error}")))?;

    run_tcp(&tcp)
}

/// Reconstructs an already-bound `UdpSocket` from a fd inherited across
/// `exec` (`DEC-012`), verifying it is actually a datagram socket before
/// trusting it as one — a bare `RawFd` carries no type information, so a
/// caller that accidentally passes the TCP listener's fd here (or any
/// other fd) is rejected explicitly rather than silently misbehaving at
/// first use.
fn inherited_udp_socket(fd: RawFd) -> Result<UdpSocket, RunError> {
    // SAFETY: `fd` is passed by firma-hakoniwa-runner, a trusted parent
    // process, specifically as an already-open, already-bound socket
    // inherited across `exec` for this purpose (`DEC-012`) — not
    // attacker-controlled input. This process takes exclusive ownership;
    // its actual socket type is verified immediately below, not assumed.
    #[expect(
        unsafe_code,
        reason = "FromRawFd::from_raw_fd is the only way to reconstruct a socket \
                  inherited across exec from a trusted parent process (DEC-012); \
                  its validity and type are verified immediately below, not assumed"
    )]
    let socket = unsafe { UdpSocket::from_raw_fd(fd) };
    match nix::sys::socket::getsockopt(&socket, SockTypeOpt) {
        Ok(SockType::Datagram) => Ok(socket),
        Ok(other) => Err(RunError::Spawn(format!(
            "--inherited-udp-fd {fd} is not a datagram socket (got {other:?})"
        ))),
        Err(error) => Err(RunError::Spawn(format!(
            "--inherited-udp-fd {fd} is not a valid socket: {error}"
        ))),
    }
}

/// As [`inherited_udp_socket`], for the TCP listener.
fn inherited_tcp_listener(fd: RawFd) -> Result<TcpListener, RunError> {
    // SAFETY: see `inherited_udp_socket` — same trusted-parent contract,
    // same immediate type verification below.
    #[expect(
        unsafe_code,
        reason = "FromRawFd::from_raw_fd is the only way to reconstruct a socket \
                  inherited across exec from a trusted parent process (DEC-012); \
                  its validity and type are verified immediately below, not assumed"
    )]
    let listener = unsafe { TcpListener::from_raw_fd(fd) };
    match nix::sys::socket::getsockopt(&listener, SockTypeOpt) {
        Ok(SockType::Stream) => Ok(listener),
        Ok(other) => Err(RunError::Spawn(format!(
            "--inherited-tcp-fd {fd} is not a stream socket (got {other:?})"
        ))),
        Err(error) => Err(RunError::Spawn(format!(
            "--inherited-tcp-fd {fd} is not a valid socket: {error}"
        ))),
    }
}

fn run_udp(socket: &UdpSocket) {
    let mut buf = [0_u8; 4096];
    loop {
        match socket.recv_from(&mut buf) {
            Ok((len, peer)) => {
                if let Some(response) = refused_response(&buf[..len]) {
                    let _ = socket.send_to(&response, peer);
                }
            }
            Err(error) => tracing::warn!("DNS UDP stub receive failed: {error}"),
        }
    }
}

fn run_tcp(listener: &TcpListener) -> Result<i32, RunError> {
    loop {
        let (stream, peer) = listener
            .accept()
            .map_err(|error| RunError::Spawn(format!("DNS TCP stub accept failed: {error}")))?;

        thread::spawn(move || {
            if let Err(error) = handle_tcp_client(stream) {
                tracing::warn!("DNS TCP stub connection from {peer} failed: {error}");
            }
        });
    }
}

fn handle_tcp_client(mut stream: TcpStream) -> io::Result<()> {
    loop {
        let mut len_buf = [0_u8; 2];
        match stream.read_exact(&mut len_buf) {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::UnexpectedEof
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::BrokenPipe
                ) =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
        }

        let len = u16::from_be_bytes(len_buf) as usize;
        let mut query = vec![0_u8; len];
        stream.read_exact(&mut query)?;

        if let Some(response) = refused_response(&query) {
            let len = u16::try_from(response.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "DNS response too large"))?
                .to_be_bytes();
            stream.write_all(&len)?;
            stream.write_all(&response)?;
        }
    }
}

fn refused_response(query: &[u8]) -> Option<Vec<u8>> {
    if query.len() < DNS_HEADER_LEN {
        return None;
    }

    let mut response = query.to_vec();
    response[2] |= 0x80;
    response[3] = (response[3] & 0xF0) | DNS_RCODE_REFUSED;
    response[6] = 0;
    response[7] = 0;
    response[8] = 0;
    response[9] = 0;
    response[10] = 0;
    response[11] = 0;
    Some(response)
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream, UdpSocket};
    use std::os::fd::IntoRawFd;
    use std::time::Duration;

    use super::{
        DNS_RCODE_REFUSED, DnsStubInput, execute_dns_stub, handle_tcp_client,
        host::HostDnsStubHandle, refused_response,
    };
    use crate::error::RunError;

    fn sample_query() -> Vec<u8> {
        vec![
            0xAB, 0xCD, // transaction id
            0x01, 0x00, // flags: standard query
            0x00, 0x01, // 1 question
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // no answers/authority/additional
            0x07, b'e', b'x', b'a', b'm', b'p', b'l', b'e', 0x03, b'c', b'o', b'm', 0x00, 0x00,
            0x01, // QTYPE A
            0x00, 0x01, // QCLASS IN
        ]
    }

    #[test]
    fn refused_response_preserves_query_id_and_question() {
        let query = [
            0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07, b'e',
            b'x', b'a', b'm', b'p', b'l', b'e', 0x03, b'c', b'o', b'm', 0x00, 0x00, 0x01, 0x00,
            0x01,
        ];

        let response = refused_response(&query).expect("valid DNS query");

        assert_eq!(&response[0..2], &[0x12, 0x34]);
        assert_ne!(response[2] & 0x80, 0, "response bit must be set");
        assert_eq!(response[3] & 0x0F, DNS_RCODE_REFUSED);
        assert_eq!(&response[4..6], &[0x00, 0x01]);
        assert_eq!(&response[12..], &query[12..]);
    }

    #[test]
    fn refused_response_rejects_malformed_header() {
        assert!(refused_response(&[0_u8; 11]).is_none());
    }

    #[test]
    fn tcp_client_receives_length_prefixed_refused_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_tcp_client(stream).expect("handle tcp client");
        });

        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("read timeout");
        let query = sample_query();
        let len = u16::try_from(query.len())
            .expect("sample query fits u16")
            .to_be_bytes();
        client.write_all(&len).expect("write len");
        client.write_all(&query).expect("write query");

        let mut response_len = [0_u8; 2];
        client.read_exact(&mut response_len).expect("read len");
        let response_len = u16::from_be_bytes(response_len) as usize;
        let mut response = vec![0_u8; response_len];
        client.read_exact(&mut response).expect("read response");
        drop(client);
        server.join().expect("server thread");

        assert_eq!(&response[..2], &[0xAB, 0xCD]);
        assert_ne!(response[2] & 0x80, 0, "QR bit must be set");
        assert_eq!(response[3] & 0x0F, DNS_RCODE_REFUSED);
        assert_eq!(&response[12..], &query[12..]);
    }

    #[test]
    fn tcp_client_skips_malformed_query_without_response() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            handle_tcp_client(stream).expect("handle tcp client");
        });

        let mut client = TcpStream::connect(addr).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_millis(200)))
            .expect("read timeout");
        client.write_all(&5_u16.to_be_bytes()).expect("write len");
        client.write_all(&[0_u8; 5]).expect("write malformed query");

        let mut response_len = [0_u8; 2];
        let error = client
            .read_exact(&mut response_len)
            .expect_err("malformed query must not produce a TCP response");
        drop(client);
        server.join().expect("server thread");

        assert!(
            matches!(
                error.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
            "unexpected read error: {error}"
        );
    }

    // ── execute_dns_stub ──────────────────────────────────────────────────────

    /// `PLAN-020`: a real positive control for `BwrapBackend`'s own,
    /// unmodified call site — asserts `execute_dns_stub` still binds
    /// `listen` itself and serves real queries when neither inherited fd
    /// is set, not just that unrelated helper-function tests keep passing.
    #[test]
    fn execute_dns_stub_binds_and_serves_when_no_fd_is_inherited() {
        // Reserve a genuinely free ephemeral port via a throwaway bind,
        // then release it immediately — execute_dns_stub binds `listen`
        // itself and reports nothing back, so the test must know the
        // address in advance.
        let probe = TcpListener::bind("127.0.0.1:0").expect("reserve ephemeral port");
        let listen = probe.local_addr().expect("addr");
        drop(probe);

        let input = DnsStubInput {
            listen,
            inherited_udp_fd: None,
            inherited_tcp_fd: None,
        };
        std::thread::spawn(move || {
            let _ = execute_dns_stub(&input);
        });
        std::thread::sleep(Duration::from_millis(100));

        let client = UdpSocket::bind("127.0.0.1:0").expect("client bind");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout");
        let query = sample_query();
        client.send_to(&query, listen).expect("send query");
        let mut buf = [0_u8; 512];
        let (_, _) = client.recv_from(&mut buf).expect("recv response");
        assert_eq!(&buf[..2], &[0xAB, 0xCD], "transaction id must be preserved");
        assert_eq!(buf[3] & 0x0F, DNS_RCODE_REFUSED);
    }

    /// The inherited-fd path (`DEC-012`, `HakoniwaBackend` only) exercises
    /// the exact same `refused_response`/`run_udp`/`run_tcp` logic as the
    /// bind path — proven by constructing real sockets here (standing in
    /// for `firma-hakoniwa-runner`'s own pre-bound ones) and passing only
    /// their raw fd numbers, the same way production code would.
    #[test]
    fn execute_dns_stub_serves_over_inherited_fds() {
        let udp = UdpSocket::bind("127.0.0.1:0").expect("bind udp");
        let listen = udp.local_addr().expect("addr");
        let tcp = TcpListener::bind(listen).expect("bind tcp on the same port");
        let udp_fd = udp.into_raw_fd();
        let tcp_fd = tcp.into_raw_fd();

        let input = DnsStubInput {
            listen: "127.0.0.1:0".parse().expect("unused placeholder addr"),
            inherited_udp_fd: Some(udp_fd),
            inherited_tcp_fd: Some(tcp_fd),
        };
        std::thread::spawn(move || {
            let _ = execute_dns_stub(&input);
        });
        std::thread::sleep(Duration::from_millis(100));

        let client = UdpSocket::bind("127.0.0.1:0").expect("client bind");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout");
        let query = sample_query();
        client.send_to(&query, listen).expect("send query");
        let mut buf = [0_u8; 512];
        let (_, _) = client.recv_from(&mut buf).expect("recv response");
        assert_eq!(&buf[..2], &[0xAB, 0xCD], "transaction id must be preserved");
        assert_eq!(buf[3] & 0x0F, DNS_RCODE_REFUSED);
    }

    /// `PLAN-022`: a swapped fd pair (the UDP-shaped argument actually
    /// pointing at the TCP listener's fd, and vice versa) must be rejected
    /// deterministically, not silently misbehave — both fds are real,
    /// valid sockets, just the wrong type for the argument they're passed
    /// as, so only the `SO_TYPE` check (not fd validity) can catch this.
    #[test]
    fn execute_dns_stub_rejects_a_swapped_fd_pair() {
        let udp = UdpSocket::bind("127.0.0.1:0").expect("bind udp");
        let listen = udp.local_addr().expect("addr");
        let tcp = TcpListener::bind(listen).expect("bind tcp on the same port");
        let udp_fd = udp.into_raw_fd();
        let tcp_fd = tcp.into_raw_fd();

        let input = DnsStubInput {
            listen: "127.0.0.1:0".parse().expect("unused placeholder addr"),
            inherited_udp_fd: Some(tcp_fd),
            inherited_tcp_fd: Some(udp_fd),
        };
        let error = execute_dns_stub(&input).expect_err("a swapped fd pair must be rejected");
        assert!(matches!(error, RunError::Spawn(_)));
    }

    /// Exactly one of the two inherited-fd arguments being set is treated
    /// as a caller error, not a fallback to binding — mixing "inherit one,
    /// bind the other" would leave the DNS stub in an inconsistent,
    /// untested configuration.
    #[test]
    fn execute_dns_stub_rejects_exactly_one_inherited_fd_being_set() {
        let input = DnsStubInput {
            listen: "127.0.0.1:0".parse().expect("unused placeholder addr"),
            inherited_udp_fd: Some(3),
            inherited_tcp_fd: None,
        };
        let error =
            execute_dns_stub(&input).expect_err("exactly one inherited fd must be rejected");
        assert!(matches!(error, RunError::Spawn(_)));
    }

    // ── HostDnsStubHandle ─────────────────────────────────────────────────────

    #[test]
    fn host_dns_stub_responds_refused_over_udp_and_tcp() {
        let stub = HostDnsStubHandle::start().expect("stub start");
        let addr = stub.listen_addr();

        let client = UdpSocket::bind("127.0.0.1:0").expect("client bind");
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout");

        let query = sample_query();
        client.send_to(&query, addr).expect("send query");

        let mut buf = [0_u8; 512];
        let (len, _) = client.recv_from(&mut buf).expect("recv response");
        let response = &buf[..len];

        // Transaction ID preserved.
        assert_eq!(&response[..2], &[0xAB, 0xCD]);
        // QR bit set (response).
        assert_ne!(response[2] & 0x80, 0, "QR bit must be set");
        // RCODE = REFUSED.
        assert_eq!(
            response[3] & 0x0F,
            DNS_RCODE_REFUSED,
            "RCODE must be REFUSED"
        );

        let mut tcp_client = TcpStream::connect(addr).expect("TCP connect");
        tcp_client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set TCP timeout");
        tcp_client
            .write_all(
                &u16::try_from(query.len())
                    .expect("query fits u16")
                    .to_be_bytes(),
            )
            .expect("write TCP query length");
        tcp_client.write_all(&query).expect("write TCP query");

        let mut response_len = [0_u8; 2];
        tcp_client
            .read_exact(&mut response_len)
            .expect("read TCP response length");
        let mut response = vec![0_u8; u16::from_be_bytes(response_len) as usize];
        tcp_client
            .read_exact(&mut response)
            .expect("read TCP response");
        assert_eq!(&response[..2], &[0xAB, 0xCD]);
        assert_ne!(response[2] & 0x80, 0, "QR bit must be set");
        assert_eq!(response[3] & 0x0F, DNS_RCODE_REFUSED);
    }

    #[test]
    fn host_dns_stub_drops_cleanly() {
        let stub = HostDnsStubHandle::start().expect("stub start");
        let addr = stub.listen_addr();
        drop(stub);
        // After drop the port should be released; a new stub can bind there if
        // the OS reuses ephemeral ports — or just verify it doesn't hang.
        let _ = addr;
    }

    #[test]
    fn host_dns_stub_listen_addr_is_loopback() {
        let stub = HostDnsStubHandle::start().expect("stub start");
        assert_eq!(
            stub.listen_addr().ip(),
            std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            "stub must bind on loopback"
        );
    }
}
