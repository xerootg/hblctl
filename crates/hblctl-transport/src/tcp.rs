// SPDX-FileCopyrightText: 2026 Haley Grandle
// SPDX-License-Identifier: LGPL-3.0-or-later

//! TCP transport to the camera's diagnostic port.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::{Error, Transport};

/// The port `camera-gui` listens on.
pub const DEFAULT_PORT: u16 = 30303;
/// Default address of the camera on its own Wi-Fi hotspot and USB network link.
pub const DEFAULT_HOST: &str = "192.168.42.2";

/// A TCP connection with a per-read timeout.
#[derive(Debug)]
pub struct TcpTransport {
    stream: TcpStream,
    read_timeout: Duration,
}

impl TcpTransport {
    /// Connect to `addr` (`host:port`), failing after `connect_timeout`.
    pub async fn connect(
        addr: &str,
        connect_timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self, Error> {
        let stream = tokio::time::timeout(connect_timeout, TcpStream::connect(addr))
            .await
            .map_err(|_| Error::Timeout(connect_timeout))??;
        stream.set_nodelay(true)?;
        tracing::debug!(target: "hblctl::transport", %addr, "connected");
        Ok(Self { stream, read_timeout })
    }
}

impl Transport for TcpTransport {
    async fn send(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.stream.write_all(bytes).await?;
        self.stream.flush().await?;
        Ok(())
    }

    async fn recv(&mut self) -> Result<Option<Vec<u8>>, Error> {
        let mut buf = vec![0u8; 64 * 1024];
        let n = tokio::time::timeout(self.read_timeout, self.stream.read(&mut buf))
            .await
            .map_err(|_| Error::Timeout(self.read_timeout))??;
        if n == 0 {
            return Ok(None);
        }
        buf.truncate(n);
        Ok(Some(buf))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn loopback_echo_and_close() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut b = [0u8; 3];
            s.read_exact(&mut b).await.unwrap();
            s.write_all(&b).await.unwrap();
        });
        let mut t = TcpTransport::connect(&addr, Duration::from_secs(2), Duration::from_secs(2))
            .await
            .unwrap();
        t.send(&[1, 2, 3]).await.unwrap();
        let mut got = Vec::new();
        while got.len() < 3 {
            got.extend(t.recv().await.unwrap().unwrap());
        }
        assert_eq!(got, vec![1, 2, 3]);
        server.await.unwrap();
        assert_eq!(t.recv().await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let _keep = tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(s);
        });
        let mut t = TcpTransport::connect(&addr, Duration::from_secs(2), Duration::from_millis(50))
            .await
            .unwrap();
        assert!(matches!(t.recv().await, Err(Error::Timeout(_))));
    }
}
