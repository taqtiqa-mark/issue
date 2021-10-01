use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use futures::StreamExt;
use lazy_static::lazy_static; // 1.4.0
use pprof::criterion::{Output, PProfProfiler};
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time;
use tracing::{self, debug};
use tracing::instrument;

/// Table of Contents
///
///     LoC Description
///   26-62 Setup Streams for HTTP client
///   63-99 Start Server and Client
/// 100-142 Criterion Setup
/// 143-305 Server Code
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
fn make_stream<'a>(
    client: &'a Client<String>,
) -> impl futures::Stream + 'a {

    let concurrency_limit = 1000;

    let it = client.addresses.iter().cycle().take(client.count).cloned();
    let vec = it.collect::<Vec<String>>().to_vec();
    let v: Vec<&'static str> = vec.iter().map(std::ops::Deref::deref).collect::<Vec<&'static str>>().to_vec();
    let strm = async_stream::stream! {
        for i in 0..client.count {
            let query_start = tokio::time::Instant::now();
            let response = client.session.get(hyper::Uri::from_static(v[i])).await.expect("Hyper response");
            let (parts, body) = response.into_parts();
            // This is for Surf client use case
            //let mut response = session.get("/").await.expect("Surf response");
            //let body = response.body_string().await.expect("Surf body");
            debug!("\nSTATUS:{:?}\nBODY:\n{:?}", parts.status, body);
            yield futures::future::ready(query_start.elapsed());
        }
    };
    strm.buffer_unordered(concurrency_limit)
    // let strm = futures::stream::iter(0..count)
    //     .map(move |i| async move {
    //         debug!("Concurrently iterating client code as future");
    //         let query_start = tokio::time::Instant::now();
    //         // This is for Hyper client use case
    //         // let address = it.next().unwrap().clone();
    //         // let a = address.clone().as_str().clone();
    //         let response = session.get(hyper::Uri::from_static(v[i])).await.expect("Hyper response");
    //         let (parts, body) = response.into_parts();
    //         // This is for Surf client use case
    //         //let mut response = session.get("/").await.expect("Surf response");
    //         //let body = response.body_string().await.expect("Surf body");
    //         debug!("\nSTATUS:{:?}\nBODY:\n{:?}", parts.status, body);
    //         query_start.elapsed()
    //     })
    //     .buffer_unordered(concurrency_limit);
    //     strm
}

#[instrument]
async fn run_stream(client: &'static Client<String>,) {
    let local = tokio::task::LocalSet::new();
    local.run_until(async move {
        tokio::task::spawn(async move {
            debug!("About to make stream");
            let stream = make_stream(client);
            futures_util::pin_mut!(stream);
            while let Some(_duration) = stream.next().await {
                debug!("Stream next polled.");
            }
        }).await.expect("Client task");
    }).await;
}

// Start HTTP Server, Setup the HTTP client and Run the Stream
#[instrument]
async fn capacity(count: usize) {
    let mut server = Some(spawn_server());
    if let Some(ref server) = server {
        // Any String will start the server...
        server.send(Msg::Echo("Start".to_string())).unwrap();
        let secs = time::Duration::from_millis(2000);
        std::thread::sleep(secs);
        println!("The server WAS spawned!");
    } else {
        println!("The server was NOT spawned!");
    };
    debug!("About to init client");
    let client = Client::<String>::new();
    // use std::convert::TryInto;
    // let session: surf::Client = surf::Config::new()
    //     .set_http_client(http_client::h1::H1Client::new())
    //     .set_base_url(surf::Url::parse("http://127.0.0.1:8888").unwrap())
    //     .set_timeout(Some(std::time::Duration::from_secs(5)))
    //     .set_tcp_no_delay(true)
    //     .set_http_keep_alive(true)
    //     .set_max_connections_per_host(50)
    //     .try_into()
    //     .unwrap();
    let benchmark_start = tokio::time::Instant::now();
    let ftr = run_stream(&client);
    ftr.await;
    // let ftr2 = run_stream(&client);
    // ftr2.await;
    // Stop the server thread using the channel pattern...
    // https://matklad.github.io/2018/03/03/stopping-a-rust-worker.html
    drop(server.take().expect("MIO server stopped"));
    drop(client);
    println!(
        "Throughput: {:.1} request/s",
        1000000.0 * count as f64 / benchmark_start.elapsed().as_micros() as f64
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
    let count = 100000;
    let tokio_executor = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(8)
        .thread_name("calibrate-limit")
        .thread_stack_size(4 * 1024 * 1024)
        .build()
        .unwrap();
    group.bench_with_input(
        BenchmarkId::new("calibrate-limit", count),
        &count,
        |b, &_s| {
            // Insert a call to `to_async` to convert the bencher to async mode.
            // The timing loops are the same as with the normal bencher.
            b.to_async(&tokio_executor).iter(|| capacity(count));
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
#[derive(Clone,Debug)]
struct Client<T> {
    addresses: std::vec::Vec<T>,
    session: hyper::Client<hyper::client::HttpConnector>,
    count: usize,
}

impl Client<String> {
    fn new() -> Self {
        Default::default()
    }

    fn add_address(&mut self) -> std::vec::Vec<String> {
        let listener = mio::net::TcpListener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let address = listener.local_addr().unwrap().to_string();
        self.addresses.push(address);
        self.addresses
    }
}

impl<T: 'static> Default for Client<T> {
    fn default() -> Self {
        Client {
            addresses: vec![],
            session: hyper::Client::new(),
            count: 1,
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
    Echo(String),
}

fn spawn_server() -> std::sync::mpsc::Sender<Msg> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Echo(_msg) => {
                    init_mio_server();
                },
            }
        }
        println!("The server has stopped!");
    });
    tx
}

// We need a server that eliminates Hyper server code as an explanation.
// This is a lean TCP server for responding with Hello World! to a request.
// https://github.com/sergey-melnychuk/mio-tcp-server
fn init_mio_server() {
    let address = "";
    //ADDR_VEC.lock().unwrap().push(address.clone());
    let mut listener = reuse_mio_listener(&address.parse().unwrap()).expect("Could not bind to addr");
    let mut poll = mio::Poll::new().unwrap();
    poll.registry().register(
        &mut listener,
        mio::Token(0),
        mio::Interest::READABLE
    )
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

                            poll.registry().register(
                                &mut socket,
                                token,
                                mio::Interest::READABLE
                            )
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
                        poll.registry().reregister(
                            sockets.get_mut(&token).unwrap(),
                            token,
                            mio::Interest::WRITABLE
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
                    poll.registry().reregister(
                        sockets.get_mut(&token).unwrap(),
                        token,
                        mio::Interest::READABLE
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
