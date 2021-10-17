use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
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

impl<T: 'static> Default for Client<T> {
    fn default() -> Self {
        let requests = 1_000_000;
        let cpus = num_cpus::get(); // Server tasks to start
        let clients = cpus * 50; // Client tasks to start
        let servers = cpus * 50;
        let iterations = 100; // Criteron samples
        Client {
            addresses: vec![],
            session: hyper::Client::new(),
            concurrency: 80,
            count: 1,
            counted: 0,
            duration: std::time::Duration::new(0, 0),
            nclients: clients,
            nservers: servers,
            nstreamed: requests / clients,
            niterations: iterations,
            nrequests: requests,
        }
    }
}

////////////////////////////////////////////////////////////////////////////////
///
///  Client setup
///

/// Setup Streams for HTTP GET
///
/// Allocate each iteration to one of the servers started - ensures a server is
/// not a bottle neck.
/// Invoke client to get URL, return a stream of response durations.
/// Note: Does *not* spawn new threads.
/// Requests are made concurrently (not parallel) by this part of the code.
/// Async code runs on the caller thread, where parallelism is introduced.
///
/// For a description of this use of streams, see:
/// https://pkolaczk.github.io/benchmarking-cassandra-with-rust-streams/
#[instrument]
fn make_stream<'client>(
    client: &'client Client<String>,
) -> impl futures::Stream<Item = usize> + 'client {
    let stream = async_stream::stream! {
        let it = client.addresses.iter().cycle().take(client.nstreamed).cloned();
        let vec = it.collect::<Vec<String>>();
        let urls = vec!["http://".to_owned(); client.nstreamed];
        let urls = urls.into_iter()
                        .zip(vec)
                        .map(|(s,t)|s+&t)
                        .collect::<Vec<String>>();
        let urls = urls.into_iter()
                        .map(|u|u.parse::<hyper::Uri>().unwrap())
                        .collect::<Vec<hyper::Uri>>();
        for url in urls.iter() {
            let query_start = tokio::time::Instant::now();
            debug!("URL Parsed: {}",  url);
            let mut response = client.session.get(url.clone()).await.expect("Hyper client response");
            let body = hyper::body::to_bytes(response.body_mut()).await.expect("Body");
            // let (parts, body) = response.into_parts();
            // This is for Surf client use case
            //let mut response = session.get("/").await.expect("Surf response");
            //let body = response.body_string().await.expect("Surf body");
            debug!("\nSTATUS:{:?}\nBODY:\n{:?}", response.status(), body);

            yield futures::future::ready(1);
        }
    };
    stream.buffer_unordered(client.concurrency)
}

#[instrument]
async fn run_stream(client: std::sync::Arc<Client<String>>) -> () {
    let client = client.as_ref();
    let stream = make_stream(client);
    futures_util::pin_mut!(stream);
    while let Some(_client) = stream.next().await {
        debug!(
            "Stream next polled. Thread: {:?}",
            std::thread::current().id()
        );
    }
}

////////////////////////////////////////////////////////////////////////////////
//
// Start Server and Client
//
// Start HTTP Server, Setup the HTTP client and Run the Stream
//
#[instrument]
fn launch_servers(client: &mut Client<String>) -> Vec<std::sync::mpsc::Sender<Msg>> {
    let mut servers = vec![];
    println!("Initializing servers");
    for _ in 0..client.nservers {
        let address = client.add_address();
        println!("  - Added address: {}", address);
        servers.push(spawn_server(address));
        if let Some(server) = servers.last() {
            // Any String will start the server...
            server.send(Msg::Start).unwrap();
            debug!("    - The server WAS spawned!");
        } else {
            debug!("    - The server was NOT spawned!");
        };
    }
    servers
}

#[instrument]
async fn launch_clients(mut client: std::sync::Arc<Client<String>>) {
    let mut clients = vec![client.clone();client.nclients];
    let mut handles = vec![];
    for c in clients.into_iter() {
        handles.push(tokio::spawn(run_stream(c)))
    }
    println!("Awaiting clients");
    let results = futures::future::join_all(handles).await;
}

////////////////////////////////////////////////////////////////////////////////
// Criterion Setup
//
// The hang behavior is intermittent.
// We use Criterion to run 100 iterations which should be sufficient to
// generate at least one hang across different users/machines.
//
fn calibrate_limit(c: &mut Criterion<RequestsPerSecond>) {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .expect("Tracing subscriber in benchmark");
    debug!("Running on thread {:?}", std::thread::current().id());
    let mut group = c.benchmark_group("Calibrate");
    let mut client = Client::<String>::new();
    let nrequests = client.nrequests;
    let servers = launch_servers(&mut client);
    // Let the OS catch its breath.
    let secs = std::time::Duration::from_millis(2000);
    std::thread::sleep(secs);
    let client = std::sync::Arc::new(client);
    let tokio_executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(1500)
        .thread_name("calibrate-limit")
        .thread_stack_size(1 * 1024 * 1024)
        .build()
        .unwrap();
    group
        .throughput(Throughput::Elements(client.nrequests as u64))
        .bench_with_input(
            BenchmarkId::new("calibrate-limit", client.nrequests),
            &client.clone(),
            |b, c| {
                // Insert a call to `to_async` to convert the bencher to async mode.
                // The timing loops are the same as with the normal bencher.
                b.to_async(&tokio_executor)
                    .iter(|| launch_clients(client.clone()));
            },
        );
    println!("Terminating servers");
    for s in servers.iter() {
        s.send(Msg::Stop).unwrap();
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = alternate_config();
    targets = calibrate_limit
}

criterion_main!(benches);

fn alternate_config() -> Criterion<RequestsPerSecond> {
    Criterion::default()
        .with_profiler(PProfProfiler::new(3, Output::Protobuf))
        .with_measurement(RequestsPerSecond)
        .nresamples(2)
        .sample_size(100)
}

// Measure requests per second.
pub const NANOS_PER_SEC: u64 = 1_000_000_000;
struct RequestsPerSecond;
impl criterion::measurement::Measurement for RequestsPerSecond {
    type Intermediate = std::time::Instant;
    type Value = std::time::Duration;

    fn start(&self) -> Self::Intermediate {
        std::time::Instant::now()
    }
    fn end(&self, i: Self::Intermediate) -> Self::Value {
        i.elapsed()
    }
    fn add(&self, v1: &Self::Value, v2: &Self::Value) -> Self::Value {
        *v1 + *v2
    }
    fn zero(&self) -> Self::Value {
        std::time::Duration::from_secs(0)
    }
    fn to_f64(&self, val: &Self::Value) -> f64 {
        let nanos = val.as_secs() * NANOS_PER_SEC + u64::from(val.subsec_nanos());
        nanos as f64
    }
    fn formatter(&self) -> &dyn criterion::measurement::ValueFormatter {
        &RequestsPerSecondFormatter
    }
}
struct RequestsPerSecondFormatter;
impl criterion::measurement::ValueFormatter for RequestsPerSecondFormatter {
    fn format_value(&self, value: f64) -> String {
        // The value will be in nanoseconds so we have to convert to seconds.
        format!("{} s", value * 10f64.powi(-9))
    }

    fn format_throughput(&self, throughput: &criterion::Throughput, value: f64) -> String {
        match *throughput {
            criterion::Throughput::Bytes(bytes) => {
                format!("{} b/sec", bytes as f64 / (value * 10f64.powi(-9)))
            }
            criterion::Throughput::Elements(elems) => {
                format!("{} req/sec", elems as f64 / (value * 10f64.powi(-9)))
            }
            criterion::Throughput::Bytes(_) => todo!(),
            criterion::Throughput::Elements(_) => todo!(),
        }
    }

    fn scale_values(&self, ns: f64, values: &mut [f64]) -> &'static str {
        for val in values {
            *val *= 10f64.powi(-9);
        }
        "sec"
    }

    fn scale_throughputs(
        &self,
        _typical: f64,
        throughput: &Throughput,
        values: &mut [f64],
    ) -> &'static str {
        match *throughput {
            Throughput::Bytes(bytes) => {
                // Convert nanoseconds/iteration to bytes/second.
                for val in values {
                    *val = (bytes as f64) / (*val * 10f64.powi(-9))
                }
                "b/sec"
            }
            Throughput::Elements(elems) => {
                for val in values {
                    *val = (elems as f64) / (*val * 10f64.powi(-9))
                }
                "req/sec"
            }
        }
    }

    fn scale_for_machines(&self, values: &mut [f64]) -> &'static str {
        // Convert values in nanoseconds to seconds.
        for val in values {
            *val *= 10f64.powi(-9);
        }

        "sec"
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
    duration: std::time::Duration,
    nclients: usize,
    nservers: usize,
    nstreamed: usize,
    niterations: usize,
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
// A simple TCP server rules out anything Hyper server related.
// Client stress examples are self contained, hence reproducible
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

// Connect the client session to a random server ip:port (String).
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

// This server eliminates Hyper server code as an explanation for
// any client behavior while stressed.
// A lean TCP server for that always responds with Hello World! to a request.
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

                    // Use existing connection ("keep-alive") - go back to reading
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

// Make server startup robust to existing listener on the same ip:port.
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
