use std::io;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use base64::{Engine as _, prelude::BASE64_STANDARD};
use url::Url;

/// Basic proxy credentials are never sent to the destination or written to logs.
pub(crate) fn authorization(proxy: &Url) -> Option<String> {
    if proxy.username().is_empty() {
        return None;
    }
    Some(format!("Basic {}", BASE64_STANDARD.encode(format!("{}:{}", proxy.username(), proxy.password().unwrap_or_default()))))
}

pub async fn proxy_connect<T: AsyncRead + AsyncWrite + Unpin>(
    mut proxy_connection: T,
    connect_host: &str,
    connect_port: &str,
    proxy: &Url,
) -> io::Result<T> {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(b"CONNECT ");
    buffer.extend_from_slice(connect_host.as_bytes());
    buffer.push(b':');
    buffer.extend_from_slice(connect_port.as_bytes());
    buffer.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    buffer.extend_from_slice(connect_host.as_bytes());
    buffer.push(b':');
    buffer.extend_from_slice(connect_port.as_bytes());
    if let Some(auth) = authorization(proxy) {
        buffer.extend_from_slice(b"\r\nProxy-Authorization: ");
        buffer.extend_from_slice(auth.as_bytes());
    }
    buffer.extend_from_slice(b"\r\n\r\n");

    proxy_connection.write_all(buffer.as_ref()).await?;

    buffer.resize(buffer.capacity(), 0);

    let mut offset = 0;
    loop {
        let bytes_read = proxy_connection.read(&mut buffer[offset..]).await?;
        if bytes_read == 0 {
            return Err(io::Error::other("Early EOF from proxy"));
        }
        offset += bytes_read;

        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut response = httparse::Response::new(&mut headers);

        let status = response
            .parse(&buffer[..offset])
            .map_err(io::Error::other)?;

        if status.is_complete() {
            return match response.code {
                Some(200) => Ok(proxy_connection), // Proxy says all is well
                Some(code) => {
                    let reason = response.reason.unwrap_or("no reason");
                    let msg = format!("Proxy responded with {code}: {reason}");
                    Err(io::Error::other(msg))
                }
                None => Err(io::Error::other("Malformed response from proxy")),
            };
        }

        if offset >= buffer.len() {
            buffer.resize(buffer.len() + 100, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{io::{Read as _, Write as _}, net::TcpListener, thread, time::{Duration, Instant}};
    use url::Url;

    #[test]
    fn native_proxy_tunnel_receives_client_hello_without_credentials() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = Url::parse(&format!("http://user:pass@{}", listener.local_addr().unwrap())).unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until, "native connection bypassed the proxy");
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("{error}"),
                }
            };
            // Windows accepted sockets inherit the nonblocking listener's mode.
            // The tunnel exchange uses blocking reads with a timeout, not polling.
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.starts_with("CONNECT synthetic.invalid:443 HTTP/1.1\r\n"));
            assert!(request.contains("Host: synthetic.invalid:443\r\n"));
            assert!(request.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"));
            stream.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").unwrap();
            let mut hello = [0; 2];
            stream.read_exact(&mut hello).unwrap();
            assert_eq!(hello, [0, 4], "native ClientHello crossed the CONNECT tunnel");
            // Close before any authentication. No Spotify server or account is contacted.
        });
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        assert!(runtime.block_on(crate::connection::connect("synthetic.invalid", 443, Some(&proxy))).is_err());
        server.join().unwrap();
    }
}
