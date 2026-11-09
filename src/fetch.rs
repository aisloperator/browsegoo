//! Downloads files over HTTPS, for filter lists.
//!
//! WebKit does the networking for pages, in its own process. This is the
//! minimum of HTTP/1.1 needed on top of rustls to fetch a list, with
//! certificates checked against the system's trusted roots.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

use rustls_platform_verifier::ConfigVerifierExt;
use url::Url;

const MAX_REDIRECTS: usize = 5;
const MAX_SIZE: usize = 32 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(30);

/// Downloads `url`, which must be HTTPS, following redirects.
pub fn get(url: &Url, user_agent: &str) -> Result<Vec<u8>, String> {
    if url.scheme() != "https" {
        return Err(format!("not an HTTPS URL: {url}"));
    }
    let config = rustls::ClientConfig::with_platform_verifier()
        .map_err(|error| format!("could not set up TLS: {error}"))?;
    let config = Arc::new(config);

    let mut url = url.clone();
    for _ in 0..=MAX_REDIRECTS {
        let response = request(&config, &url, user_agent)?;
        let response = parse_response(&response)?;
        match response.status {
            200 => return Ok(response.body),
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .location
                    .ok_or_else(|| format!("redirect with no location from {url}"))?;
                url = url
                    .join(&location)
                    .map_err(|error| format!("bad redirect location {location:?}: {error}"))?;
            }
            status => return Err(format!("HTTP status {status} from {url}")),
        }
    }
    Err(format!("too many redirects, at {url}"))
}

fn request(
    config: &Arc<rustls::ClientConfig>,
    url: &Url,
    user_agent: &str,
) -> Result<Vec<u8>, String> {
    if url.scheme() != "https" {
        return Err(format!("not an HTTPS URL: {url}"));
    }
    let host = url.host_str().ok_or_else(|| format!("no host in {url}"))?;
    let port = url.port_or_known_default().unwrap_or(443);
    let server_name = rustls::pki_types::ServerName::try_from(host.to_owned())
        .map_err(|error| format!("bad host name {host:?}: {error}"))?;

    let connect = |error: io::Error| format!("could not connect to {host}: {error}");
    let address = (host, port);
    let socket = TcpStream::connect(address).map_err(connect)?;
    socket.set_read_timeout(Some(TIMEOUT)).map_err(connect)?;
    socket.set_write_timeout(Some(TIMEOUT)).map_err(connect)?;
    let connection = rustls::ClientConnection::new(config.clone(), server_name)
        .map_err(|error| format!("could not start TLS with {host}: {error}"))?;
    let mut stream = rustls::StreamOwned::new(connection, socket);

    let mut path = url.path().to_owned();
    if let Some(query) = url.query() {
        path.push('?');
        path.push_str(query);
    }
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: {user_agent}\r\nAccept: */*\r\n\
         Accept-Encoding: identity\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|error| format!("could not send request to {host}: {error}"))?;

    let mut response = Vec::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                response.extend_from_slice(&buffer[..count]);
                if response.len() > MAX_SIZE {
                    return Err(format!("response from {host} is too large"));
                }
            }
            // Some servers close without TLS's close_notify. Whether the response is
            // complete is checked from its length or chunking, when it's parsed.
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(format!("could not read response from {host}: {error}")),
        }
    }
    Ok(response)
}

#[derive(Debug, PartialEq)]
struct Response {
    status: u16,
    location: Option<String>,
    body: Vec<u8>,
}

fn parse_response(response: &[u8]) -> Result<Response, String> {
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or("incomplete response headers")?;
    let head = std::str::from_utf8(&response[..head_end]).map_err(|_| "bad response headers")?;
    let body = &response[head_end + 4..];

    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .ok_or_else(|| format!("bad status line {status_line:?}"))?;

    let mut location = None;
    let mut content_length = None;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "location" => location = Some(value.to_owned()),
            "content-length" => {
                content_length = Some(value.parse::<usize>().map_err(|_| "bad content length")?)
            }
            "transfer-encoding" => chunked = value.eq_ignore_ascii_case("chunked"),
            "content-encoding" if !value.eq_ignore_ascii_case("identity") => {
                return Err(format!("unexpected content encoding {value:?}"));
            }
            _ => {}
        }
    }

    let body = if chunked {
        decode_chunked(body)?
    } else if let Some(length) = content_length {
        if body.len() < length {
            return Err("response ended early".into());
        }
        body[..length].to_vec()
    } else {
        body.to_vec()
    };
    Ok(Response {
        status,
        location,
        body,
    })
}

fn decode_chunked(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let line_end = data
            .windows(2)
            .position(|window| window == b"\r\n")
            .ok_or("response ended early")?;
        let size_line = std::str::from_utf8(&data[..line_end]).map_err(|_| "bad chunk size")?;
        let size_text = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_text, 16).map_err(|_| "bad chunk size")?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(body);
        }
        if data.len() < size + 2 {
            return Err("response ended early".into());
        }
        body.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_response_with_length() {
        let response =
            parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        assert_eq!(
            response,
            Response {
                status: 200,
                location: None,
                body: b"hello".to_vec()
            }
        );
    }

    #[test]
    fn chunked_response() {
        let response = parse_response(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n7;x=y\r\n, world\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(response.body, b"hello, world");
    }

    #[test]
    fn redirect() {
        let response =
            parse_response(b"HTTP/1.1 301 Moved\r\nlocation: /new\r\ncontent-length: 0\r\n\r\n")
                .unwrap();
        assert_eq!(response.status, 301);
        assert_eq!(response.location.as_deref(), Some("/new"));
    }

    #[test]
    fn truncated_responses_are_errors() {
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nhello").is_err());
        assert!(
            parse_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n")
                .is_err()
        );
        assert!(parse_response(b"HTTP/1.1 200 OK\r\nContent-Le").is_err());
    }

    #[test]
    fn compressed_responses_are_errors() {
        let response = b"HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: 1\r\n\r\nx";
        assert!(parse_response(response).is_err());
    }

    #[test]
    fn only_https() {
        let url = Url::parse("http://example.com/list.txt").unwrap();
        assert!(get(&url, "test").unwrap_err().contains("not an HTTPS URL"));
    }
}
