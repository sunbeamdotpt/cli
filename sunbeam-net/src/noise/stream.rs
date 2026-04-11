use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};

use bytes::BytesMut;
use chacha20poly1305::{AeadInPlace, ChaCha20Poly1305, Nonce, Tag};
use futures::stream::StreamExt;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_util::codec::Framed;

use super::framing::{FRAME_TYPE_DATA, MAX_FRAME_SIZE, NoiseFrame, NoiseFrameCodec};

/// AEAD tag size for ChaCha20Poly1305.
const TAG_SIZE: usize = 16;

/// Maximum plaintext chunk size, leaving room for the AEAD tag (16 bytes).
const MAX_PLAINTEXT_CHUNK: usize = MAX_FRAME_SIZE - TAG_SIZE;

/// An encrypted, framed stream using the controlbase protocol over TCP.
///
/// Implements `AsyncRead + AsyncWrite` so it can be used transparently by `h2`
/// and other async I/O consumers.
///
/// Generic over the underlying stream so it can wrap either a plain
/// TcpStream or a TLS-wrapped one (`tokio_rustls::client::TlsStream`).
pub struct NoiseStream<S = TcpStream> {
    inner: Framed<S, NoiseFrameCodec>,
    tx_cipher: ChaCha20Poly1305,
    rx_cipher: ChaCha20Poly1305,
    read_nonce: AtomicU64,
    write_nonce: AtomicU64,
    read_buf: BytesMut,
    // Encrypted frames ready to be sent, accumulated during poll_write.
    pending_write: BytesMut,
    // Whether we have an error from a previous operation.
    write_err: Option<std::io::Error>,
}

// SAFETY: ChaCha20Poly1305 is Send, all other fields are Send.
unsafe impl<S: Send> Send for NoiseStream<S> {}

impl<S: Unpin> Unpin for NoiseStream<S> {}

impl<S> std::fmt::Debug for NoiseStream<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoiseStream")
            .field("read_nonce", &self.read_nonce.load(Ordering::Relaxed))
            .field("write_nonce", &self.write_nonce.load(Ordering::Relaxed))
            .finish()
    }
}

/// Build a 12-byte nonce: first 4 bytes zero, last 8 bytes little-endian u64 counter.
fn build_nonce(counter: u64) -> Nonce {
    let mut nonce = [0u8; 12];
    nonce[4..12].copy_from_slice(&counter.to_be_bytes());
    Nonce::from(nonce)
}

impl<S> NoiseStream<S>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    /// Wrap a stream with controlbase encryption using the given transport ciphers.
    /// `leftover` contains any bytes already read from the underlying stream that
    /// belong to the first Noise transport record (from the handshake buffer overflow).
    pub fn new(
        stream: S,
        tx_cipher: ChaCha20Poly1305,
        rx_cipher: ChaCha20Poly1305,
        leftover: Vec<u8>,
    ) -> Self {
        let mut framed = Framed::new(stream, NoiseFrameCodec);
        // Prepend leftover bytes so the codec can parse them as part of the first frame.
        if !leftover.is_empty() {
            framed.read_buffer_mut().extend_from_slice(&leftover);
        }
        Self {
            inner: framed,
            tx_cipher,
            rx_cipher,
            read_nonce: AtomicU64::new(0),
            write_nonce: AtomicU64::new(0),
            read_buf: BytesMut::with_capacity(MAX_FRAME_SIZE),
            pending_write: BytesMut::new(),
            write_err: None,
        }
    }

    /// Read and consume the early payload sent by Headscale after the handshake.
    ///
    /// The early payload consists of multiple Noise records:
    /// 1. 5 bytes: magic `\xff\xff\xffTS`
    /// 2. 4 bytes: BE u32 JSON length
    /// 3. N bytes: JSON payload
    ///
    /// Each part may be a separate Noise record. We read and reassemble them.
    /// If the first record is not an early payload (i.e. it's an HTTP/2 frame),
    /// we buffer it for h2 to read.
    ///
    /// Returns the early payload JSON bytes, or an empty vec if no early payload.
    pub async fn consume_early_payload(&mut self) -> std::io::Result<Vec<u8>> {
        // Accumulate plaintext from multiple records
        let mut accum = Vec::new();

        // Read first frame
        let frame = match self.inner.next().await {
            Some(Ok(frame)) => frame,
            Some(Err(e)) => {
                return Err(std::io::Error::other(e.to_string()));
            }
            None => return Ok(vec![]),
        };
        let plaintext = self.decrypt_frame(&frame)?;
        accum.extend_from_slice(&plaintext);

        // Check for early payload magic
        if accum.len() < 5
            || accum[0] != 0xff
            || accum[1] != 0xff
            || accum[2] != 0xff
            || accum[3] != b'T'
            || accum[4] != b'S'
        {
            // Not an early payload — buffer for h2
            self.read_buf.extend_from_slice(&accum);
            return Ok(vec![]);
        }

        // Read more frames until we have the 4-byte length
        while accum.len() < 9 {
            let frame = match self.inner.next().await {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    return Err(std::io::Error::other(e.to_string()));
                }
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "early payload truncated",
                    ));
                }
            };
            accum.extend_from_slice(&self.decrypt_frame(&frame)?);
        }

        let json_len = u32::from_be_bytes([accum[5], accum[6], accum[7], accum[8]]) as usize;

        // Read more frames until we have the full JSON
        while accum.len() < 9 + json_len {
            let frame = match self.inner.next().await {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    return Err(std::io::Error::other(e.to_string()));
                }
                None => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "early payload JSON truncated",
                    ));
                }
            };
            accum.extend_from_slice(&self.decrypt_frame(&frame)?);
        }

        let json_bytes = accum[9..9 + json_len].to_vec();
        tracing::debug!("consumed early payload ({} bytes)", json_bytes.len());

        // If there's any leftover data after the early payload, buffer it for h2
        if accum.len() > 9 + json_len {
            self.read_buf.extend_from_slice(&accum[9 + json_len..]);
        }

        Ok(json_bytes)
    }

    /// Decrypt a data frame and return plaintext.
    fn decrypt_frame(&self, frame: &NoiseFrame) -> std::io::Result<Vec<u8>> {
        let counter = self.read_nonce.fetch_add(1, Ordering::Relaxed);
        let nonce = build_nonce(counter);
        tracing::trace!(
            "decrypt_frame: type={:02x}, payload_len={}, nonce={}",
            frame.frame_type,
            frame.payload.len(),
            counter
        );

        if frame.payload.len() < TAG_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "frame too short for AEAD tag",
            ));
        }

        let (ct, tag_bytes) = frame.payload.split_at(frame.payload.len() - TAG_SIZE);
        let tag = Tag::from_slice(tag_bytes);
        let mut plaintext = ct.to_vec();
        self.rx_cipher
            .decrypt_in_place_detached(&nonce, &[], &mut plaintext, tag)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        Ok(plaintext)
    }

    /// Encrypt a chunk and produce a NoiseFrame.
    fn encrypt_chunk(&self, plaintext: &[u8]) -> std::io::Result<NoiseFrame> {
        let counter = self.write_nonce.fetch_add(1, Ordering::Relaxed);
        let nonce = build_nonce(counter);

        let mut ciphertext = plaintext.to_vec();
        let tag = self
            .tx_cipher
            .encrypt_in_place_detached(&nonce, &[], &mut ciphertext)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        ciphertext.extend_from_slice(tag.as_slice());

        Ok(NoiseFrame {
            frame_type: FRAME_TYPE_DATA,
            payload: BytesMut::from(&ciphertext[..]),
        })
    }
}

impl<S> AsyncRead for NoiseStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        // If we have buffered plaintext, return it.
        if !this.read_buf.is_empty() {
            let to_copy = std::cmp::min(this.read_buf.len(), buf.remaining());
            buf.put_slice(&this.read_buf[..to_copy]);
            let _ = this.read_buf.split_to(to_copy);
            return Poll::Ready(Ok(()));
        }

        // Poll the inner framed stream for the next frame.
        match this.inner.poll_next_unpin(cx) {
            Poll::Ready(Some(Ok(frame))) => {
                if frame.frame_type != FRAME_TYPE_DATA {
                    // Skip non-data frames; wake to try again.
                    cx.waker().wake_by_ref();
                    return Poll::Pending;
                }
                match this.decrypt_frame(&frame) {
                    Ok(plaintext) => {
                        let to_copy = std::cmp::min(plaintext.len(), buf.remaining());
                        buf.put_slice(&plaintext[..to_copy]);
                        if to_copy < plaintext.len() {
                            this.read_buf.extend_from_slice(&plaintext[to_copy..]);
                        }
                        Poll::Ready(Ok(()))
                    }
                    Err(e) => Poll::Ready(Err(e)),
                }
            }
            Poll::Ready(Some(Err(e))) => Poll::Ready(Err(e)),
            Poll::Ready(None) => Poll::Ready(Ok(())), // EOF
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> AsyncWrite for NoiseStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();

        if let Some(e) = this.write_err.take() {
            return Poll::Ready(Err(e));
        }

        // Encrypt in chunks and buffer the frames.
        let mut written = 0;
        for chunk in buf.chunks(MAX_PLAINTEXT_CHUNK) {
            match this.encrypt_chunk(chunk) {
                Ok(frame) => {
                    // Manually serialize the frame into pending_write.
                    let len = frame.payload.len() as u16;
                    this.pending_write.extend_from_slice(&[
                        frame.frame_type,
                        (len >> 8) as u8,
                        len as u8,
                    ]);
                    this.pending_write.extend_from_slice(&frame.payload);
                    written += chunk.len();
                }
                Err(e) => {
                    if written > 0 {
                        // Report what we encrypted so far; save error for next call.
                        this.write_err = Some(e);
                        return Poll::Ready(Ok(written));
                    }
                    return Poll::Ready(Err(e));
                }
            }
        }

        Poll::Ready(Ok(written))
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();

        // Flush pending_write bytes through the underlying stream.
        if !this.pending_write.is_empty() {
            let inner_stream = this.inner.get_mut();
            loop {
                if this.pending_write.is_empty() {
                    break;
                }
                match Pin::new(&mut *inner_stream).poll_write(cx, &this.pending_write) {
                    Poll::Ready(Ok(n)) => {
                        let _ = this.pending_write.split_to(n);
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                }
            }
        }

        Pin::new(this.inner.get_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(this.inner.get_mut()).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_plaintext_chunk_leaves_room_for_tag() {
        const { assert!(MAX_PLAINTEXT_CHUNK + 16 <= MAX_FRAME_SIZE) }
    }
}
