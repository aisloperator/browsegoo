//! A SOCKS5 proxy that WebKit connects through, which looks up names itself,
//! and refuses to connect to a local address for a name that isn't a local
//! one, so that a public site's name can't be made to point at this computer
//! or the local network, as with DNS rebinding.
//!
//! Addresses given as such, and local names, are allowed here, as the content
//! blockers from `local_network` decide which pages can reach them.
//!
//! It only listens on 127.0.0.1, and only serves clients with its user name
//! and password, which are made up each time BrowseGoo starts.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs,
};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::local_network::{is_local_name, is_public_ip};

/// How long a client has to say where to connect.
const HANDSHAKE_TIME_LIMIT: Duration = Duration::from_secs(20);
/// How long to try connecting to each address.
const CONNECT_TIME_LIMIT: Duration = Duration::from_secs(30);

const VERSION: u8 = 5;
const USER_PASSWORD: u8 = 2;
const NO_ACCEPTABLE_METHOD: u8 = 0xff;
const CONNECT: u8 = 1;

/// SOCKS5 replies.
const SUCCEEDED: u8 = 0;
const GENERAL_FAILURE: u8 = 1;
const HOST_UNREACHABLE: u8 = 4;
const COMMAND_NOT_SUPPORTED: u8 = 7;
const ADDRESS_TYPE_NOT_SUPPORTED: u8 = 8;

/// Where a client asks to connect.
#[derive(Debug, PartialEq)]
pub enum Destination {
    Address(SocketAddr),
    Name(String, u16),
}

/// Why a connection was refused.
#[derive(Debug, PartialEq)]
pub enum Refusal {
    /// A name that isn't a local one is for a local address.
    PublicNameForLocalAddress,
    /// The name couldn't be looked up.
    NotFound,
}

/// The addresses to try for `destination`, using `resolve` to look up names,
/// or why not to connect.
pub fn addresses_for(
    destination: &Destination,
    resolve: impl Fn(&str, u16) -> io::Result<Vec<SocketAddr>>,
) -> Result<Vec<SocketAddr>, Refusal> {
    match destination {
        Destination::Address(address) => Ok(vec![*address]),
        Destination::Name(name, port) => {
            let addresses = resolve(name, *port).map_err(|_| Refusal::NotFound)?;
            if addresses.is_empty() {
                return Err(Refusal::NotFound);
            }
            let reaches_local = addresses.iter().any(|address| !is_public_ip(address.ip()));
            if reaches_local && !is_local_name(name) {
                return Err(Refusal::PublicNameForLocalAddress);
            }
            Ok(addresses)
        }
    }
}

fn resolve(name: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
    Ok((name, port).to_socket_addrs()?.collect())
}

/// Names, with ports, that the proxy refused, to say why pages at them didn't
/// load.
type Refused = Arc<Mutex<HashSet<(String, u16)>>>;

/// The proxy, which serves clients on its own threads.
pub struct SocksProxy {
    /// For WebKit's proxy settings, with the user name and password.
    pub uri: String,
    refused: Refused,
}

impl SocksProxy {
    pub fn start() -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        let user = random_hex(16)?;
        let password = random_hex(16)?;
        let uri = format!("socks5://{user}:{password}@127.0.0.1:{port}");
        let credentials = (user.into_bytes(), password.into_bytes());
        let refused = Refused::default();
        let refusing = refused.clone();
        thread::Builder::new()
            .name("socks-proxy".into())
            .spawn(move || {
                for client in listener.incoming().flatten() {
                    let credentials = credentials.clone();
                    let refused = refusing.clone();
                    let _ = thread::Builder::new()
                        .name("socks-client".into())
                        .spawn(move || serve(client, &credentials, &refused));
                }
            })?;
        Ok(Self { uri, refused })
    }

    /// Whether a connection to `name` and `port` was refused, as a public
    /// name for a local address.
    pub fn refused(&self, name: &str, port: u16) -> bool {
        let refused = self
            .refused
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        refused.contains(&(name.to_ascii_lowercase(), port))
    }
}

fn random_hex(bytes: usize) -> io::Result<String> {
    let mut random = vec![0; bytes];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    Ok(random.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// Compares secrets in a time that doesn't depend on where they differ.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0, |differ, (x, y)| differ | (x ^ y)) == 0
}

fn serve(mut client: TcpStream, credentials: &(Vec<u8>, Vec<u8>), refused: &Refused) {
    let _ = client.set_read_timeout(Some(HANDSHAKE_TIME_LIMIT));
    let destination = match handshake(&mut client, credentials) {
        Ok(Some(destination)) => destination,
        _ => return,
    };
    let addresses = match addresses_for(&destination, resolve) {
        Ok(addresses) => addresses,
        Err(refusal) => {
            if let (Refusal::PublicNameForLocalAddress, Destination::Name(name, port)) =
                (&refusal, &destination)
            {
                eprintln!("Refused to connect to {name}:{port}, a public name for a local address");
                let mut refused = refused
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                refused.insert((name.to_ascii_lowercase(), *port));
            }
            // Not "not allowed by ruleset", which WebKit doesn't end a page's
            // load for.
            let _ = reply(&mut client, HOST_UNREACHABLE);
            return;
        }
    };
    let Some(server) = addresses
        .iter()
        .find_map(|address| TcpStream::connect_timeout(address, CONNECT_TIME_LIMIT).ok())
    else {
        let _ = reply(&mut client, HOST_UNREACHABLE);
        return;
    };
    if reply(&mut client, SUCCEEDED).is_err() || client.set_read_timeout(None).is_err() {
        return;
    }
    relay(client, server);
}

/// Reads the client's greeting, user name and password, and request, and
/// returns where it asks to connect, or `None` once it's been refused.
fn handshake(
    client: &mut TcpStream,
    (user, password): &(Vec<u8>, Vec<u8>),
) -> io::Result<Option<Destination>> {
    let [version, method_count] = read_array(client)?;
    let methods = read_vec(client, method_count.into())?;
    if version != VERSION || !methods.contains(&USER_PASSWORD) {
        client.write_all(&[VERSION, NO_ACCEPTABLE_METHOD])?;
        return Ok(None);
    }
    client.write_all(&[VERSION, USER_PASSWORD])?;

    let [_, user_length] = read_array(client)?;
    let given_user = read_vec(client, user_length.into())?;
    let [password_length] = read_array(client)?;
    let given_password = read_vec(client, password_length.into())?;
    let accepted = same_secret(&given_user, user) & same_secret(&given_password, password);
    client.write_all(&[1, if accepted { 0 } else { 1 }])?;
    if !accepted {
        return Ok(None);
    }

    let [version, command, _, address_type] = read_array(client)?;
    if version != VERSION || command != CONNECT {
        reply(client, COMMAND_NOT_SUPPORTED)?;
        return Ok(None);
    }
    let host = match address_type {
        1 => Host::Address(IpAddr::V4(Ipv4Addr::from(read_array::<4>(client)?))),
        4 => Host::Address(IpAddr::V6(Ipv6Addr::from(read_array::<16>(client)?))),
        3 => {
            let [length] = read_array(client)?;
            let name = read_vec(client, length.into())?;
            match String::from_utf8(name) {
                Ok(name) => Host::Name(name),
                Err(_) => {
                    reply(client, GENERAL_FAILURE)?;
                    return Ok(None);
                }
            }
        }
        _ => {
            reply(client, ADDRESS_TYPE_NOT_SUPPORTED)?;
            return Ok(None);
        }
    };
    let port = u16::from_be_bytes(read_array(client)?);
    Ok(Some(match host {
        Host::Address(address) => Destination::Address(SocketAddr::new(address, port)),
        // A name that's an address, such as one given in brackets.
        Host::Name(name) => match name.trim_matches(['[', ']']).parse::<IpAddr>() {
            Ok(address) => Destination::Address(SocketAddr::new(address, port)),
            Err(_) => Destination::Name(name, port),
        },
    }))
}

enum Host {
    Address(IpAddr),
    Name(String),
}

fn read_array<const N: usize>(stream: &mut TcpStream) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_vec(stream: &mut TcpStream, length: usize) -> io::Result<Vec<u8>> {
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn reply(client: &mut TcpStream, code: u8) -> io::Result<()> {
    // The bound address isn't needed by clients, so it's left empty.
    client.write_all(&[VERSION, code, 0, 1, 0, 0, 0, 0, 0, 0])
}

/// Copies what each side sends to the other, until both are done.
fn relay(client: TcpStream, server: TcpStream) {
    let (Ok(client_reader), Ok(server_writer)) = (client.try_clone(), server.try_clone()) else {
        return;
    };
    let upload = thread::Builder::new()
        .name("socks-upload".into())
        .spawn(move || copy_then_close(client_reader, server_writer));
    copy_then_close(server, client);
    if let Ok(upload) = upload {
        let _ = upload.join();
    }
}

fn copy_then_close(mut from: TcpStream, mut to: TcpStream) {
    let _ = io::copy(&mut from, &mut to);
    let _ = to.shutdown(Shutdown::Write);
    let _ = from.shutdown(Shutdown::Read);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolving_to(
        addresses: &'static [&'static str],
    ) -> impl Fn(&str, u16) -> io::Result<Vec<SocketAddr>> {
        move |_, port| {
            Ok(addresses
                .iter()
                .map(|address| SocketAddr::new(address.parse().unwrap(), port))
                .collect())
        }
    }

    fn name(name: &str) -> Destination {
        Destination::Name(name.to_owned(), 80)
    }

    #[test]
    fn public_names_cant_reach_local_addresses() {
        assert_eq!(
            addresses_for(&name("rebind.example.com"), resolving_to(&["127.0.0.1"])),
            Err(Refusal::PublicNameForLocalAddress)
        );
        assert_eq!(
            addresses_for(
                &name("mixed.example.com"),
                resolving_to(&["93.184.216.34", "192.168.1.1"])
            ),
            Err(Refusal::PublicNameForLocalAddress)
        );
        assert_eq!(
            addresses_for(&name("v6.example.com"), resolving_to(&["::ffff:10.0.0.1"])),
            Err(Refusal::PublicNameForLocalAddress)
        );
        assert!(addresses_for(&name("example.com"), resolving_to(&["93.184.216.34"])).is_ok());
    }

    #[test]
    fn local_names_and_addresses_are_left_to_the_content_blockers() {
        assert!(addresses_for(&name("localhost"), resolving_to(&["127.0.0.1"])).is_ok());
        assert!(addresses_for(&name("printer.local"), resolving_to(&["192.168.1.9"])).is_ok());
        let address = Destination::Address("127.0.0.1:8000".parse().unwrap());
        assert_eq!(
            addresses_for(&address, resolving_to(&[])),
            Ok(vec!["127.0.0.1:8000".parse().unwrap()])
        );
        assert_eq!(
            addresses_for(&name("nowhere.example"), resolving_to(&[])),
            Err(Refusal::NotFound)
        );
    }

    #[test]
    fn secrets_compare() {
        assert!(same_secret(b"abc", b"abc"));
        assert!(!same_secret(b"abc", b"abd"));
        assert!(!same_secret(b"abc", b"abcd"));
    }

    /// A whole connection through the proxy, and a refused one.
    #[test]
    fn proxies_with_credentials() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        let target_port = target.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut connection, _) = target.accept().unwrap();
            connection.write_all(b"hello").unwrap();
        });
        let proxy = SocksProxy::start().unwrap();
        let rest = proxy.uri.strip_prefix("socks5://").unwrap();
        let (user_password, address) = rest.split_once('@').unwrap();
        let (user, password) = user_password.split_once(':').unwrap();

        let connect = |user: &str, password: &str| {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(&[5, 1, 2]).unwrap();
            assert_eq!(read_array::<2>(&mut stream).unwrap(), [5, 2]);
            let mut login = vec![1, user.len() as u8];
            login.extend(user.as_bytes());
            login.push(password.len() as u8);
            login.extend(password.as_bytes());
            stream.write_all(&login).unwrap();
            let status = read_array::<2>(&mut stream).unwrap();
            (stream, status)
        };

        let (_, status) = connect(user, "wrong");
        assert_eq!(status, [1, 1]);

        let (mut stream, status) = connect(user, password);
        assert_eq!(status, [1, 0]);
        let mut request = vec![5, 1, 0, 1, 127, 0, 0, 1];
        request.extend(target_port.to_be_bytes());
        stream.write_all(&request).unwrap();
        assert_eq!(read_array::<10>(&mut stream).unwrap()[1], SUCCEEDED);
        let mut greeting = String::new();
        stream.read_to_string(&mut greeting).unwrap();
        assert_eq!(greeting, "hello");
    }
}
