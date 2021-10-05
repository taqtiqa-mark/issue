use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use futures::StreamExt;
use lazy_static::lazy_static; // 1.4.0
use pprof::criterion::{Output, PProfProfiler};
use std::io::{Read, Write};
use std::sync::Mutex;
use tracing::instrument;
use tracing::{self, debug};

/// Table of Contents
///
///     LoC Description
///   19-69 Setup Streams for HTTP GET
///  70-109 Start Server and Client
/// 110-152 Criterion Setup
/// 153-187 Utility Code
/// 188-305 Server Code
///

/// Setup Streams for HTTP GET
///
/// Invoke client to get URL, return a stream of response durations.
/// Note: Does *not* spawn new threads. Requests are concurrent not parallel.
/// Async code runs on the caller thread.
///
/// For a detailed description of this setup, see:
/// https://pkolaczk.github.io/benchmarking-cassandra-with-rust-streams/

#[instrument]
fn make_stream<'client>(client: &'client mut Client<String>) -> impl futures::Stream<Item=tokio::time::Duration> + 'client {
    let concurrency_limit = 100;

    let stream = async_stream::stream! {
        // Allocate each stream to one of the servers started
        let it = client.addresses.iter().cycle().take(client.count).cloned();
        let vec = it.collect::<Vec<String>>();
        let urls = vec!["http://".to_owned(); client.count];
        let urls = urls.into_iter()
                        .zip(vec)
                        .map(|(s,t)|s+&t)
                        .collect::<Vec<String>>();
        for url in urls.iter() {
            let query_start = tokio::time::Instant::now();
            let url_parsed = &url.parse::<hyper::Uri>().unwrap();
            debug!("URL String: {} URL Parsed: {}", &url, url_parsed);
            let mut response = client.session.get(url_parsed.clone()).await.expect("Hyper response");
            let body = hyper::body::to_bytes(response.body_mut()).await.expect("Body");
            // let (parts, body) = response.into_parts();
            // This is for Surf client use case
            //let mut response = session.get("/").await.expect("Surf response");
            //let body = response.body_string().await.expect("Surf body");
            debug!("\nSTATUS:{:?}\nBODY:\n{:?}", response.status(), body);
            yield futures::future::ready(query_start.elapsed());
        }
    };
    stream.buffer_unordered(concurrency_limit)
}

#[instrument]
async fn run_stream<'client>(client: &'client mut Client<String>) {
    let num = 1*num_cpus::get();
    // {
        let mut counted = 0;
        let default = client.clone();
        let mut clients = vec![default.clone(); num];
        //let it = clients.into_iter();
    // let it = clients.iter_mut();
    for i in 0..clients.len() {
        // let local = tokio::task::LocalSet::new();
        let mut client = clients[i].clone();
        // local
            // .run_until(async move {
                // tokio::task::spawn_local(async move {
                let sub_total = tokio::task::spawn(async move {

                    debug!("About to make stream");
                    let stream = make_stream(&mut client);
                    futures_util::pin_mut!(stream);
                    while let Some(_duration) = stream.next().await {
                        debug!("Stream next polled.");
                        counted += 1;
                    }
                    // let mut track = *client;
                    //client.counted = counted;
                    counted
                })
                .await
                .expect("Client task");
            clients[i].counted += sub_total;
            debug!("Total client requests: {} Cummulative: {:?}", sub_total, clients[i])

            // })
            // .await;
    // }
    };
    let mut total = 0;
    let it = clients.into_iter();
    for client in it {
        total += client.counted;
        println!("Cummulative Total: {}", total);
    }
    client.counted = total;
}

////////////////////////////////////////////////////////////////////////////////
//
// Start Server and Client
//
// Start HTTP Server, Setup the HTTP client and Run the Stream
//
#[instrument]
async fn capacity(mut client: Client<String>) {
    println!("Initializing servers");
    let mut servers = vec![];
    for _ in 0..2*num_cpus::get() {
        let address = client.add_address();
        println!("  - Added address: {}", address);
        servers.push(spawn_server(address));
        if let Some(server) = servers.last() {
            // Any String will start the server...
            server.send(Msg::Start).unwrap();
            let secs = tokio::time::Duration::from_millis(2000);
            tokio::time::sleep(secs).await;
            debug!("    - The server WAS spawned!");
        } else {
            println!("    - The server was NOT spawned!");
        };
    };
    let benchmark_start = tokio::time::Instant::now();
    let ftr = run_stream(&mut client);
    ftr.await;
    for s in servers.iter() {
        s.send(Msg::Stop).unwrap();
    }
    println!(
        "Throughput: {:.1} request/s",
        1000000.0 * client.counted as f64 / benchmark_start.elapsed().as_micros() as f64
    );
}

////////////////////////////////////////////////////////////////////////////////
// Criterion Setup
//
// The hang behavior is intermittent.
// We use Criterion to run 100 iterations which should be sufficient to
// generate at least one hang across different users/machines.
//
fn calibrate_limit(c: &mut Criterion) {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .expect("Tracing subscriber in benchmark");
    debug!("Running on thread {:?}", std::thread::current().id());
    let mut group = c.benchmark_group("Calibrate");
    let mut client = Client::<String>::new();
    client.count = 50_000;
    let tokio_executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(8)
        .thread_name("calibrate-limit")
        .thread_stack_size(4 * 1024 * 1024)
        .build()
        .unwrap();
    group.bench_with_input(
        BenchmarkId::new("calibrate-limit", client.count),
        &client,
        |b, c| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&tokio_executor).iter(|| capacity(c.clone()));
        },
    );
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(3, Output::Protobuf));
    targets = calibrate_limit
}

criterion_main!(benches);

////////////////////////////////////////////////////////////////////////////////
// Utility Code
//
#[derive(Clone, Debug)]
struct Client<T> {
    addresses: std::vec::Vec<T>,
    session: hyper::Client<hyper::client::HttpConnector>,
    count: usize,
    counted: usize,
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

impl<T: 'static> Default for Client<T> {
    fn default() -> Self {
        Client {
            addresses: vec![],
            session: hyper::Client::new(),
            count: 1,
            counted: 0,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
// Server Code
//
// This rules out anything Hyper server related.
// This also means the example is self contained, hence reproducible
//
lazy_static! {
    static ref ADDR_VEC: Mutex<Vec<String>> = Mutex::new(vec![]);
}
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
                },
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
