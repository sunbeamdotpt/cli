use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{IpAddress, IpCidr};
use tokio::sync::mpsc;

/// Virtual TCP/IP network backed by WireGuard.
pub(crate) struct VirtualNetwork {
    iface: Interface,
    sockets: SocketSet<'static>,
    device: ChannelDevice,
}

/// A smoltcp Device backed by mpsc channels.
///
/// The `try_recv` / `try_send` methods are used because smoltcp's Device trait
/// is synchronous and poll-based.
struct ChannelDevice {
    rx: mpsc::Receiver<Vec<u8>>,
    tx: mpsc::Sender<Vec<u8>>,
    mtu: usize,
}

impl Device for ChannelDevice {
    type RxToken<'a>
        = ChannelRxToken
    where
        Self: 'a;
    type TxToken<'a>
        = ChannelTxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self.rx.try_recv() {
            Ok(data) => {
                let rx = ChannelRxToken { data };
                let tx = ChannelTxToken { sender: &self.tx };
                Some((rx, tx))
            }
            Err(_) => None,
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(ChannelTxToken { sender: &self.tx })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = self.mtu;
        caps
    }
}

struct ChannelRxToken {
    data: Vec<u8>,
}

impl RxToken for ChannelRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.data)
    }
}

struct ChannelTxToken<'a> {
    sender: &'a mpsc::Sender<Vec<u8>>,
}

impl<'a> TxToken for ChannelTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        // Best-effort send — if the channel is full, drop the packet.
        let _ = self.sender.try_send(buf);
        result
    }
}

/// Handle to a TCP socket within the virtual network.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TcpSocketHandle(SocketHandle);

impl VirtualNetwork {
    /// Create a new virtual network with the given local IP (from tailnet assignment).
    ///
    /// `rx_from_wg` receives decrypted IP packets from the WireGuard tunnel.
    /// `tx_to_wg` sends IP packets back to the WireGuard tunnel for encryption.
    pub fn new(
        local_ip: IpAddress,
        prefix_len: u8,
        rx_from_wg: mpsc::Receiver<Vec<u8>>,
        tx_to_wg: mpsc::Sender<Vec<u8>>,
    ) -> crate::Result<Self> {
        let mut device = ChannelDevice {
            rx: rx_from_wg,
            tx: tx_to_wg,
            mtu: 1420, // Standard WireGuard MTU
        };

        let config = Config::new(smoltcp::wire::HardwareAddress::Ip);
        let now = Instant::from_millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        );

        let mut iface = Interface::new(config, &mut device, now);
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(local_ip, prefix_len))
                .expect("interface IP address capacity exceeded");
        });

        let sockets = SocketSet::new(vec![]);

        Ok(Self {
            iface,
            sockets,
            device,
        })
    }

    /// Poll the network stack. Returns true if any socket state may have changed.
    pub fn poll(&mut self) -> bool {
        let now = Instant::from_millis(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        );
        self.iface.poll(now, &mut self.device, &mut self.sockets) == PollResult::SocketStateChanged
    }

    /// Open a TCP connection to a remote address through the virtual network.
    pub fn tcp_connect(&mut self, remote: std::net::SocketAddr) -> crate::Result<TcpSocketHandle> {
        let rx_buf = tcp::SocketBuffer::new(vec![0u8; 65536]);
        let tx_buf = tcp::SocketBuffer::new(vec![0u8; 65536]);
        let mut socket = tcp::Socket::new(rx_buf, tx_buf);

        let remote_ep = match remote {
            std::net::SocketAddr::V4(v4) => {
                smoltcp::wire::IpEndpoint::new(IpAddress::Ipv4(*v4.ip()), v4.port())
            }
            std::net::SocketAddr::V6(v6) => {
                smoltcp::wire::IpEndpoint::new(IpAddress::Ipv6(*v6.ip()), v6.port())
            }
        };

        let cx = self.iface.context();
        // Use an ephemeral local port.
        let local_port = ephemeral_port();
        socket
            .connect(cx, remote_ep, local_port)
            .map_err(|e| crate::Error::WireGuard(format!("TCP connect: {e:?}")))?;

        let handle = self.sockets.add(socket);
        Ok(TcpSocketHandle(handle))
    }

    /// Check if a TCP connection is established and ready.
    pub fn tcp_is_active(&self, handle: TcpSocketHandle) -> bool {
        let socket = self.sockets.get::<tcp::Socket>(handle.0);
        socket.is_active()
    }

    /// Read data from a TCP socket.
    ///
    /// Returns `Ok(n)` with the number of bytes read (possibly 0 if no
    /// data is available right now). Returns `Err(...)` only when the
    /// socket has actually finished receiving (FIN seen, drained) — not
    /// merely when the socket is in a transient state like SynSent.
    pub fn tcp_recv(&mut self, handle: TcpSocketHandle, buf: &mut [u8]) -> crate::Result<usize> {
        let socket = self.sockets.get_mut::<tcp::Socket>(handle.0);
        // Not ready to receive yet (SynSent, Listen, Closed) — return 0,
        // don't propagate as a fatal error. The poll loop will retry.
        if !socket.may_recv() {
            // If recv side is fully closed (Finished), tell the caller so
            // they stop polling.
            if socket.state() == tcp::State::Closed
                || socket.state() == tcp::State::CloseWait
                || socket.state() == tcp::State::TimeWait
                || socket.state() == tcp::State::Closing
                || socket.state() == tcp::State::LastAck
            {
                // Recv side may still have buffered data; try one drain.
                if socket.recv_queue() > 0 {
                    return socket
                        .recv_slice(buf)
                        .map_err(|e| crate::Error::WireGuard(format!("TCP recv: {e:?}")));
                }
                return Err(crate::Error::WireGuard("TCP recv: closed".into()));
            }
            return Ok(0);
        }
        socket
            .recv_slice(buf)
            .map_err(|e| crate::Error::WireGuard(format!("TCP recv: {e:?}")))
    }

    /// Write data to a TCP socket.
    pub fn tcp_send(&mut self, handle: TcpSocketHandle, data: &[u8]) -> crate::Result<usize> {
        let socket = self.sockets.get_mut::<tcp::Socket>(handle.0);
        socket
            .send_slice(data)
            .map_err(|e| crate::Error::WireGuard(format!("TCP send: {e:?}")))
    }
}

/// Simple ephemeral port allocator using a thread-local counter.
fn ephemeral_port() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static PORT: AtomicU16 = AtomicU16::new(49152);
    let port = PORT.fetch_add(1, Ordering::Relaxed);
    if port == 0 {
        PORT.store(49153, Ordering::Relaxed);
        49152
    } else {
        port
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_virtual_network_creation() {
        let (tx_to_wg, _rx) = mpsc::channel(16);
        let (_tx, rx_from_wg) = mpsc::channel(16);

        let net = VirtualNetwork::new(IpAddress::v4(100, 64, 0, 1), 32, rx_from_wg, tx_to_wg);
        assert!(net.is_ok());
    }

    #[test]
    fn test_poll_empty() {
        let (tx_to_wg, _rx) = mpsc::channel(16);
        let (_tx, rx_from_wg) = mpsc::channel(16);

        let mut net =
            VirtualNetwork::new(IpAddress::v4(100, 64, 0, 1), 32, rx_from_wg, tx_to_wg).unwrap();

        // Polling with no packets should return false (no state changes).
        let changed = net.poll();
        assert!(!changed);
    }
}
