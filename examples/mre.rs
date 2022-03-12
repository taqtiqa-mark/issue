use futures::StreamExt;
use std::io::{Read, Write};
use tracing::instrument;
use tracing::{self, debug};

/// Table of Contents
///
///     LoC Description
///   16-38 Parameters
///   38-92 Setup concurrent and parallel HTTP GET
///  93-153 Start Server and Client
/// 154-185 Utility Code
/// 186-351 Server Code
///

// A `ulimit -Sn 512` should trigger a hang with these parameters
impl<T: 'static> Default for Client<T> {
    fn default() -> Self {
        let nrequests = 1_000_000;
        let cpus = num_cpus::get(); // 2 on initial dev system
        let nclients = 40; // Clients to start (parallelism)
        let concurrency = 128; // Concurrent requests
        let nservers = 40; // Servers to start
        let nstreamed = nrequests / nclients; // Requests per client
        Client {
            addresses: vec![],
            session: hyper::Client::new(),
            concurrency,
            count: 1,
            counted: 0,
            nclients,
            nservers,
            nstreamed,
            nrequests,
        }
    }
}

/// Setup concurrent and parallel HTTP GET
///
/// Invoke client to get URL, return a stream of response durations.
/// Allocate each client `get` to one of the servers started.
/// Note: We do *not* spawn new threads.
/// Here, requests are concurrent not parallel.
/// Async code runs on the caller thread - where parallelism occurs.
///
#[instrument]
async fn make_stream<'client>(
    client: &'client Client<String>,
) -> std::vec::Vec<Result<hyper::Response<hyper::Body>, hyper::Error>> {
    let it = client
        .addresses
        .iter()
        .cycle()
        .take(client.nstreamed)
        .cloned();
    let vec = it.collect::<Vec<String>>();
    let urls = vec!["http://".to_owned(); client.nstreamed];
    let urls = urls.into_iter().zip(vec).map(|(s, t)| s + &t);
    let urls = urls.into_iter().map(|u| u.parse::<hyper::Uri>().unwrap());

    urls.map(|url| async move { client.session.get(url) })
        .collect::<futures::stream::futures_unordered::FuturesUnordered<_>>()
        .buffer_unordered(client.concurrency)
        .collect::<Vec<_>>()
        .await
}

#[instrument]
async fn run_stream<'client>(client: std::sync::Arc<Client<String>>) -> std::vec::Vec<hyper::Body> {
    println!("Run Stream. Thread: {:?}", std::thread::current().id());

    let client = client.as_ref();
    let responses: std::vec::Vec<Result<hyper::Response<hyper::Body>, hyper::Error>> =
        make_stream(client).await;

    responses
        .into_iter()
        .map(read_body)
        .collect::<futures::stream::FuturesUnordered<_>>()
        .collect::<Vec<hyper::Body>>()
        .await
}

async fn read_body(result: Result<hyper::Response<hyper::Body>, hyper::Error>) -> hyper::Body {
    match result {
        Ok(r) => r.into_body(),
        Err(_e) => hyper::Body::empty(),
    }
}

////////////////////////////////////////////////////////////////////////////////
//
// Start Server and Client
//
// Start HTTP Server, Setup the HTTP client and Run the Stream
//
#[instrument]
async fn capacity(client: std::sync::Arc<Client<String>>) {
    let clients = vec![client.clone(); client.nclients];
    let mut handles = vec![];
    for c in clients.into_iter() {
        handles.push(tokio::spawn(run_stream(c)))
    }
    let benchmark_start = tokio::time::Instant::now();
    println!("Awaiting clients");
    futures::future::join_all(handles).await;
    let elapsed = benchmark_start.elapsed().as_micros() as f64;
    println!(
        "Throughput: {:.1} request/s [{} in {}]",
        1000000.0 * 2. * client.nrequests as f64 / elapsed,
        client.nrequests,
        elapsed
    );
}

fn main() {
    debug!("Running on thread {:?}", std::thread::current().id());
    let mut client = Client::<String>::new();
    let mut servers = vec![];
    println!("Initializing servers");
    for _ in 0..client.nservers {
        let address = client.add_address();
        println!("  - Added address: {}", address);
        servers.push(spawn_server(address));
        if let Some(server) = servers.last() {
            server.send(Msg::Start).unwrap();
            debug!("    - The server WAS spawned!");
        } else {
            debug!("    - The server was NOT spawned!");
        };
    }
    let secs = std::time::Duration::from_millis(2000);
    std::thread::sleep(secs);
    let client = std::sync::Arc::new(client);
    let tokio_executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(120)
        .thread_name("mre-client-hangs")
        .thread_stack_size(4 * 1024 * 1024)
        .build()
        .unwrap();
    tokio_executor.block_on(async { capacity(client.clone()).await });
    println!("Terminating servers");
    for s in servers.iter() {
        s.send(Msg::Stop).unwrap();
    }
}

////////////////////////////////////////////////////////////////////////////////
// Utility Code
//
#[derive(Clone, Debug)]
struct Client<T> {
    addresses: std::vec::Vec<T>,
    session: hyper::Client<hyper::client::HttpConnector>,
    concurrency: usize,
    count: usize,
    counted: usize,
    nclients: usize,
    nservers: usize,
    nstreamed: usize,
    nrequests: usize,
}

impl Client<String> {
    fn new() -> Self {
        Default::default()
    }

    fn add_address(&mut self) -> std::string::String {
        let listener = mio::net::TcpListener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        let mut url: String = "".to_owned();
        url.push_str(&address);
        debug!("Added address: {}", address);
        self.addresses.push(url.clone());
        url
    }
}

////////////////////////////////////////////////////////////////////////////////
// Server Code
//
// This rules out anything Hyper server related.
// This also means the example is self contained, hence reproducible
//
static RESPONSE: &str = "HTTP/1.1 200 OK
Content-Type: text/html
Connection: keep-alive
Content-Length: 13

Hello World!
";

fn is_double_crnl(window: &[u8]) -> bool {
    window.len() >= 4
        && (window[0] == '\r' as u8)
        && (window[1] == '\n' as u8)
        && (window[2] == '\r' as u8)
        && (window[3] == '\n' as u8)
}

// This is a very neat pattern for stopping a thread...
// After starting a thread that holds `rx`, return `tx`, store as `server`:
//     drop(server.take());
//
// https://matklad.github.io/2018/03/03/stopping-a-rust-worker.html
enum Msg {
    Start,
    Stop,
}

fn spawn_server(address: std::string::String) -> std::sync::mpsc::Sender<Msg> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(msg) = &rx.recv() {
            match msg {
                Msg::Start => {
                    debug!("    - The server should start.");
                    init_mio_server(address.clone());
                    debug!("      - The server has started.");
                }
                Msg::Stop => {
                    debug!("    - The server should stop.");
                    return;
                }
            }
        }
        debug!("The server has stopped!");
    });
    tx
}

// We need a server that eliminates Hyper server code as an explanation.
// This is a lean TCP server for responding with Hello World! to a request.
// https://github.com/sergey-melnychuk/mio-tcp-server
fn init_mio_server(address: std::string::String) {
    debug!("Server: {}", address);
    let mut listener =
        reuse_mio_listener(&address.parse().unwrap()).expect("Could not bind to address");
    let mut poll = mio::Poll::new().unwrap();
    poll.registry()
        .register(&mut listener, mio::Token(0), mio::Interest::READABLE)
        .unwrap();

    let mut counter: usize = 0;
    let mut sockets: std::collections::HashMap<mio::Token, mio::net::TcpStream> =
        std::collections::HashMap::new();
    let mut requests: std::collections::HashMap<mio::Token, Vec<u8>> =
        std::collections::HashMap::new();
    let mut buffer = [0; 1024];

    let mut events = mio::Events::with_capacity(1024);
    loop {
        poll.poll(&mut events, None).unwrap();
        for event in &events {
            match event.token() {
                mio::Token(0) => loop {
                    match listener.accept() {
                        Ok((mut socket, _)) => {
                            counter += 1;
                            let token = mio::Token(counter);

                            poll.registry()
                                .register(&mut socket, token, mio::Interest::READABLE)
                                .unwrap();

                            sockets.insert(token, socket);
                            requests.insert(token, Vec::with_capacity(8192));
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(_) => break,
                    }
                },
                token if event.is_readable() => {
                    loop {
                        let read = sockets.get_mut(&token).unwrap().read(&mut buffer);
                        match read {
                            Ok(0) => {
                                sockets.remove(&token);
                                break;
                            }
                            Ok(n) => {
                                let req = requests.get_mut(&token).unwrap();
                                for b in &buffer[0..n] {
                                    req.push(*b);
                                }
                            }
                            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                            Err(_) => break,
                        }
                    }

                    let ready = requests
                        .get(&token)
                        .unwrap()
                        .windows(4)
                        .find(|window| is_double_crnl(*window))
                        .is_some();

                    if ready {
                        poll.registry()
                            .reregister(
                                sockets.get_mut(&token).unwrap(),
                                token,
                                mio::Interest::WRITABLE,
                            )
                            .unwrap();
                    }
                }
                token if event.is_writable() => {
                    requests.get_mut(&token).unwrap().clear();
                    sockets
                        .get_mut(&token)
                        .unwrap()
                        .write_all(RESPONSE.as_bytes())
                        .unwrap();

                    // Re-use existing connection ("keep-alive") - switch back to reading
                    poll.registry()
                        .reregister(
                            sockets.get_mut(&token).unwrap(),
                            token,
                            mio::Interest::READABLE,
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
        }
    }
}

// Make server startup robust to existing listener on the same address.
fn reuse_mio_listener(
    addr: &std::net::SocketAddr,
) -> Result<mio::net::TcpListener, std::convert::Infallible> {
    let builder = match *addr {
        std::net::SocketAddr::V4(_) => mio::net::TcpSocket::new_v4().expect("TCP v4"),
        std::net::SocketAddr::V6(_) => mio::net::TcpSocket::new_v6().expect("TCP v6"),
    };
    builder.set_reuseport(true).expect("Reusable port");
    builder.set_reuseaddr(true).expect("Reusable address");
    builder.bind(*addr).expect("TCP socket");
    Ok(builder.listen(1024).expect("TCP listener"))
}
