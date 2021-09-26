// All bin and lib files are empty:
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use pprof::criterion::{Output, PProfProfiler};

use async_trait::async_trait;
use deadpool::managed;
use futures::StreamExt;
use lazy_static::lazy_static;
use tracing::instrument;
use tracing::{self, debug, info, warn};

lazy_static! {
    static ref URL: surf::Url = surf::Url::parse("http://127.0.0.1:8888").expect("URL");
}

// const URI: &str = "https://127.0.0.1";

extern crate futures;
extern crate hyper;
use hyper::{client::HttpConnector, Client as HyperClient};
extern crate num_cpus;

use std::io;
use std::net::SocketAddr;
use std::time;

use hyper::header::{CONTENT_LENGTH, CONTENT_TYPE};
use tokio::net::TcpStream;

static HELLO: &[u8] = b"Hello World!";

#[async_trait]
pub trait HttpClient: Send + Sync + Clone + 'static {
    async fn get_url(&self) -> hyper::Uri;
}

#[derive(Clone)]
pub struct Client {
    pub client: HyperClient<HttpConnector>,
}

impl Client {
    pub fn new() -> Self {
        Self {
            //client_tls: https_client(),
            client: http_client(),
        }
    }
}

fn http_client() -> HyperClient<hyper::client::HttpConnector> {
    HyperClient::new()
}

/// Deadpool managed connection pool for HTTP client.
// #[derive(Debug)]
// enum Error {
//     Fail,
// }

// pub struct PoolClient {
//     conn: hyper::Client<hyper::client::HttpConnector>,
// }

// impl PoolClient {
//     async fn get_url(&self) {
//         // let req = Request::new(Method::Get, "http://example.org");
//         // self.conn.send(req).await.unwrap()
//     }
// }

// struct Manager {}

// #[async_trait]
// impl managed::Manager for Manager {
//     type Type = PoolClient;
//     type Error = Error;

//     // async fn create(&self) -> Result<PoolClient, Error> {
//     //     // let conf = http_client::Config::new();
//     //     // conf.set_http_keep_alive(true);
//     //     // conf.set_tcp_no_delay(true);
//     //     // conf.set_timeout(std::time::Duration::from_secs(5));
//     //     // conf.set_tcp_no_delay(true);
//     //     // conf.set_max_connections_per_host(1000);
//     //     // let conn = http_client::h1::H1Client::new();
//     //     // Ok(PoolClient { conn: conn.set_config(conf) })
//     // }

//     async fn recycle(&self, _: &mut PoolClient) -> managed::RecycleResult<Error> {
//         Ok(())
//     }
// }

// type Pool = managed::Pool<Manager>;

// #[tokio::main]
// async fn main() {
//     let mgr = Manager {};
//     let pool = Pool::builder(mgr).build().unwrap();
//     let mut conn = pool.get().await.unwrap();
//     let resp = conn.get_url().await;
//     assert_eq!(resp, 42);
// }

/// This is the main function, evolved from this gist:
/// https://gist.github.com/klausi/f94b9aff7d36a1cb4ebbca746f0a099f
#[instrument]
async fn init_real_server() {
    let addr: SocketAddr = ([127, 0, 0, 1], 8888).into();
    let mut channels = Vec::new();
    let num_threads = num_cpus::get();
    for _i in 0..num_threads {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        channels.push(tx);
        tokio::spawn(async move {
            worker(rx).await;
        });
    }
    let listener = reuse_listener(&addr).await.expect("couldn't bind to addr");
    println!("Listening on address: {}", addr);
    let mut next = 0;
    let accepting = async move {
        loop {
            let socket = listener.accept().await;
            debug!("Entered while let socket accept loop");
            match socket.as_ref() {
                Ok((stream, _)) => {
                    debug!("Stream processing...");
                    stream
                        .set_nodelay(true)
                        .expect("Set no delay on TCP stream.");
                    stream.readable().await.expect("A readable TCP stream.");
                    let (stream, _) = socket.unwrap();
                    debug!("Sending to channel #: {}", next);
                    // Skip when socket closed
                    if !channels[next].is_closed() {
                        let _v = match channels[next].send(stream) {
                            Ok(_v) => {
                                next = (next + 1) % channels.len();
                            }
                            Err(e) => return warn!("Send failed ({})", e),
                        };
                    }
                }
                Err(e) if connection_error(e) => {
                    debug!("Connection error: {}", e);
                    continue;
                }
                // Ignore socket errors like "Too many open files" on the OS
                // level. We need to sleep for a bit because the socket is not
                // removed from the accept queue in this case. A direct continue
                // would spin the CPU really hard with the same error again and
                // again.
                Err(e2) => {
                    println!("Catchall error e.g. Too many open files: {}", e2);
                    let ten_millis = time::Duration::from_millis(10);
                    std::thread::sleep(ten_millis);
                    continue;
                }
            }
        }
    };
    let _handle = tokio::spawn(async { accepting.await });
    debug!("Exiting init_real_server");
}

// Set content-type and content-length headers and return response.
async fn hello(
    _: hyper::Request<hyper::Body>,
) -> std::result::Result<hyper::Response<hyper::Body>, std::convert::Infallible> {
    let mut resp = hyper::Response::new(hyper::Body::from(HELLO));
    let head = resp.headers_mut();
    head.insert(
        CONTENT_LENGTH,
        hyper::header::HeaderValue::from_str("12").unwrap(),
    );
    head.insert(
        CONTENT_TYPE,
        hyper::header::HeaderValue::from_static("text/plain"),
    );
    Ok(resp)
}

/// Represents one worker thread of the server that receives TCP connections from
/// the main server thread.
/// This is the `worker` function from the gist related to `init_real_server`.
async fn worker(mut rx: tokio::sync::mpsc::UnboundedReceiver<tokio::net::TcpStream>) {
    debug!("Entering worker function.");
    while let Some(socket) = rx.recv().await {
        debug!("Channel receiver polled.");
        // A new task is spawned for each inbound socket. The socket is
        // moved to the new task and processed there.
        tokio::spawn(async move {
            process(socket).await;
        });
    }
    debug!("The worker has stopped!");
}

async fn process(socket: TcpStream) {
    debug!(
        "Serving to {:?} using thread {:?}",
        socket.peer_addr(),
        std::thread::current().id()
    );

    let mut http = hyper::server::conn::Http::new();
    http.http1_only(true);
    let serve = http.serve_connection(socket, hyper::service::service_fn(hello));
    if let Err(e) = serve.await {
        debug!("server connection error: {}", e);
    }
}

async fn reuse_listener(
    addr: &std::net::SocketAddr,
) -> Result<tokio::net::TcpListener, std::convert::Infallible> {
    let builder = match *addr {
        std::net::SocketAddr::V4(_) => tokio::net::TcpSocket::new_v4().expect("TCP v4"),
        std::net::SocketAddr::V6(_) => tokio::net::TcpSocket::new_v6().expect("TCP v6"),
    };
    builder.set_reuseport(true).expect("Reusable port");
    builder.set_reuseaddr(true).expect("Reusable address");
    builder.bind(*addr).expect("TCP socket");
    Ok(builder.listen(1024).expect("TCP listener"))
}

/// This function defines errors that are per-connection. Which basically
/// means that if we get this error from `accept()` system call it means
/// next connection might be ready to be accepted.
///
/// All other errors will incur a timeout before next `accept()` is performed.
/// The timeout is useful to handle resource exhaustion errors like ENFILE
/// and EMFILE. Otherwise, could enter into tight loop.
fn connection_error(e: &io::Error) -> bool {
    e.kind() == io::ErrorKind::ConnectionRefused
        || e.kind() == io::ErrorKind::ConnectionAborted
        || e.kind() == io::ErrorKind::ConnectionReset
}

/// Invokes client get URL, return a stream of response durations.
/// Note: Does *not* spawn new threads. Requests are concurrent not parallel.
/// Async code runs on the caller thread.
#[instrument]
fn make_stream<'a>(
    session: &'a surf::Client,
    statement: &'a URL,
    count: usize,
) -> impl futures::Stream + 'a {
    let concurrency_limit = 100;

    futures::stream::iter(0..count)
        .map(move |_| async move {
            //let statement = statement;
            debug!("Concurrently iterating client code as future");
            let query_start = tokio::time::Instant::now();
            let mut response = session.get("/").await.expect("Surf response");
            //let (parts, body) = response.unwrap().into_parts();
            // Concatenate the body stream into a single buffer...
            let body = response.body_string().await.expect("Surf body");
            debug!("\nSTATUS:{:?}\nBODY:\n{:?}", response.status(), body);
            query_start.elapsed()
        })
        // This will run up to `concurrency_limit` futures at a time:
        .buffer_unordered(concurrency_limit)
}

impl std::fmt::Debug for URL {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::result::Result<(), std::fmt::Error> {
        f.debug_struct("URL")
            //.field("other_field", &self.other_field)
            .finish()
    }
}

#[instrument]
async fn run_stream_ct(session: surf::Client, statement: &'static URL, count: usize) {
    // Construct a local task set that can run `!Send` futures.
    let local = tokio::task::LocalSet::new();
    // Run the local task set.
    local
        .run_until(async move {
            let _task = tokio::task::spawn_local(async move {
                let session = &session;
                let statement = statement;
                debug!("About to make stream");
                let mut stream = make_stream(session, statement, count);
                while let Some(_duration) = stream.next().await {
                    debug!("Stream next polled.");
                }
            })
            .await;
            //.unwrap();
        })
        .await;
    //task.await;
}

#[instrument]
async fn capacity(count: usize) {
    debug!("About to init server");
    init_real_server().await;
    debug!("About to init client");
    // hyper::Client::builder() halves throughput, compared to hyper::Client::new().
    // let session = hyper::Client::builder()
    //     .pool_max_idle_per_host(0)
    //     .pool_idle_timeout(std::time::Duration::from_secs(5))
    //     .build_http::<hyper::Body>();
    // // let session = Client::new().client;
    // let conf = http_client::Config::new();
    // // connection pooling
    // conf.set_http_keep_alive(true);
    // conf.set_tcp_no_delay(true);
    // conf.set_timeout(Some(std::time::Duration::from_secs(5)));
    // conf.set_tcp_no_delay(true);
    // conf.set_max_connections_per_host(1000);
    use std::convert::TryInto;
    let session: surf::Client = surf::Config::new()
        .set_base_url(URL.clone())
        .set_timeout(Some(std::time::Duration::from_secs(5)))
        .set_tcp_no_delay(true)
        .set_http_keep_alive(true)
        .try_into()
        .unwrap();
    // session.set_config(conf);
    let statement = &URL;
    let benchmark_start = tokio::time::Instant::now();
    debug!("Client: About to spawn blocking (Tokio)");
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Handle::current();
        debug!("Client: About to block on (Tokio)");
        rt.block_on(async {
            let local = tokio::task::LocalSet::new();
            debug!("Client: About to run until (local set)");
            local
                .run_until(async move {
                    debug!("Client: About to spawn local (Tokio)");
                    tokio::task::spawn_local(run_stream_ct(session.clone(), statement, count / 2))
                        .await
                        .expect("Tokio spawn local (streams for clients)");
                })
                .await;
        });
    })
    .await
    .unwrap();
    println!(
        "Throughput: {:.1} request/s",
        1000000.0 * count as f64 / benchmark_start.elapsed().as_micros() as f64
    );
}

fn calibrate_limit(c: &mut Criterion) {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .try_init()
        .expect("Tracing subscriber in benchmark");
    debug!("Running on thread {:?}", std::thread::current().id());
    let mut group = c.benchmark_group("Calibrate");
    let count = 100000;
    let tokio_executor = tokio::runtime::Runtime::new().expect("initializing tokio runtime");
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
