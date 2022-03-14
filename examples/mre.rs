use futures::StreamExt;
use std::io::{Read, Write};
#[cfg(feature = "traceable")]
use {
    opentelemetry::global,
    opentelemetry::global::shutdown_tracer_provider,
    opentelemetry::sdk::trace::Config,
    opentelemetry::sdk::{
        metrics::PushController,
        trace::{self, Sampler},
        Resource,
    },
    opentelemetry::trace::TraceError,
    opentelemetry::{
        baggage::BaggageExt,
        metrics::{MetricsError, ObserverResult},
        trace::{TraceContextExt, Tracer},
        Context, Key, KeyValue,
    },
    tracing::{self, debug},
    tracing_subscriber::prelude::*,
    tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Registry},
};

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
        let nrequests = 750;
        let cpus = num_cpus::get(); // 2 on initial dev system
        #[cfg(feature = "ok")]
        let nclients = 5; // Clients to start (parallelism)
        #[cfg(feature = "hang")]
        let nclients = 40; // Clients to start (parallelism)
        let concurrency = 128; // Concurrent requests
        #[cfg(feature = "ok")]
        let nservers = 5; // Servers to start (parallelism)
        #[cfg(feature = "hang")]
        let nservers = 40; // Servers to start (parallelism)
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
#[cfg_attr(feature = "traceable", tracing::instrument)]
async fn make_stream<'client>(client: &'client Client<String>) -> std::vec::Vec<hyper::Body> {
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

    urls.into_iter()
        .map(|uri| async move {
            let r = client.session.get(uri).await;
            read_body(r).await
        })
        .collect::<futures::stream::futures_unordered::FuturesUnordered<_>>()
        // .buffer_unordered(client.concurrency)
        // .map(|result| read_body(result))
        // .buffer_unordered(client.concurrency)
        .collect::<Vec<hyper::Body>>()
        .await
}

#[cfg_attr(feature = "traceable", tracing::instrument)]
async fn run_stream<'client>(client: std::sync::Arc<Client<String>>) -> std::vec::Vec<hyper::Body> {
    #[cfg(feature = "traceable")]
    debug!("Run Stream. Thread: {:?}", std::thread::current().id());

    let client = client.as_ref();
    make_stream(client).await
}

#[cfg_attr(feature = "traceable", tracing::instrument(skip(result)))]
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
#[cfg_attr(feature = "traceable", tracing::instrument)]
async fn capacity(client: std::sync::Arc<Client<String>>) {
    let clients = vec![client.clone(); client.nclients];
    let mut handles = vec![];
    for c in clients.into_iter() {
        handles.push(tokio::spawn(run_stream(c)))
    }
    let benchmark_start = tokio::time::Instant::now();
    #[cfg(feature = "traceable")]
    debug!("Awaiting clients");
    futures::future::join_all(handles).await;
    let elapsed = benchmark_start.elapsed().as_micros() as f64;
    println!(
        "Throughput: {:.1} request/s [{} in {}]",
        1000000.0 * 2. * client.nrequests as f64 / elapsed,
        client.nrequests,
        elapsed
    );
}

fn init_tracer() -> Result<opentelemetry::sdk::trace::Tracer, TraceError> {
    // tracing_subscriber::fmt::init();
    opentelemetry_jaeger::new_pipeline()
        .with_service_name("mre-0.4.13")
        .with_trace_config(trace::config().with_sampler(Sampler::AlwaysOn))
        // .with_trace_config(Config::default().with_resource(Resource::new(vec![
        //     KeyValue::new("service.name", "mre-0.4.7"),
        //     KeyValue::new("exporter", "otel-jaeger"),
        // ])))
        .install_batch(opentelemetry::runtime::Tokio)
}

// Skip first immediate tick from tokio, not needed for async_std.
fn delayed_interval(
    duration: std::time::Duration,
) -> impl futures::stream::Stream<Item = tokio::time::Instant> {
    opentelemetry::util::tokio_interval_stream(duration).skip(1)
}

fn init_meter() -> PushController {
    opentelemetry::sdk::export::metrics::stdout(tokio::spawn, delayed_interval)
        .with_formatter(|batch| {
            serde_json::to_value(batch)
                .map(|value| value.to_string())
                .map_err(|err| MetricsError::Other(err.to_string()))
        })
        .init()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    #[cfg(feature = "traceable")]
    {
        // Jaeger instance address
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 6831));
        // First create propagators
        let baggage_propagator = opentelemetry::sdk::propagation::BaggagePropagator::new();
        let trace_context_propagator =
            opentelemetry::sdk::propagation::TraceContextPropagator::new();
        let jaeger_propagator = opentelemetry_jaeger::Propagator::new();

        // Second compose propagators
        let _composite_propagator =
            opentelemetry::sdk::propagation::TextMapCompositePropagator::new(vec![
                Box::new(baggage_propagator),
                Box::new(trace_context_propagator),
                Box::new(jaeger_propagator),
            ]);
        // Third create Jaeger pipeline
        let tracer = opentelemetry_jaeger::new_pipeline()
            .with_service_name("mre-0.4.13")
            // .with_trace_config(opentelemetry::sdk::trace::Config::default().with_sampler(
            //     opentelemetry::sdk::trace::Sampler::ParentBased(Box::new(
            //         opentelemetry::sdk::trace::Sampler::TraceIdRatioBased(1.25),
            //     )),
            // ))
            // .with_trace_config(
            //     opentelemetry::sdk::trace::Config::default()
            //         .with_sampler(opentelemetry::sdk::trace::Sampler::AlwaysOn),
            // )
            // .install_batch(opentelemetry::runtime::Tokio)
            .install_simple()
            .unwrap();
        // Initialize `tracing` using `opentelemetry-tracing` and configure stdout logging
        tracing_subscriber::Registry::default()
            .with(tracing_subscriber::EnvFilter::new("TRACE"))
            .with(tracing_opentelemetry::layer().with_tracer(tracer))
            .with(tracing_subscriber::fmt::layer())
            .init();
    }

    #[cfg(feature = "traceable")]
    debug!("Running on thread {:?}", std::thread::current().id());
    let mut client = Client::<String>::new();
    let mut servers = vec![];

    #[cfg(feature = "traceable")]
    {
        // Trace executed (async) code. Create a span, returning a guard....
        let root_span = tracing::span!(tracing::Level::TRACE, "root_span");
        let traceable = tracing_futures::WithSubscriber::with_current_subscriber(async {
            run_servers_clients(client, servers).await;
        });
        tracing_futures::Instrument::instrument(traceable, root_span).await;
    }

    #[cfg(not(feature = "traceable"))]
    run_servers_clients(client, servers).await;

    #[cfg(feature = "traceable")]
    // Send remaining spans
    opentelemetry::global::shutdown_tracer_provider();
    Ok(())
}

async fn run_servers_clients(
    mut client: Client<String>,
    mut servers: Vec<std::sync::mpsc::Sender<Msg>>,
) {
    setup_servers(&mut client, &mut servers);
    // let secs = std::time::Duration::from_millis(2000);
    // std::thread::sleep(secs);
    let client = std::sync::Arc::new(client);
    capacity(client.clone()).await;
    println!("Terminating servers");
    for s in servers.iter() {
        s.send(Msg::Stop).unwrap();
    }
}

fn setup_servers(client: &mut Client<String>, servers: &mut Vec<std::sync::mpsc::Sender<Msg>>) {
    println!("Initializing servers");
    for _ in 0..client.nservers {
        let address = client.add_address();
        println!("  - Added address: {}", address);
        servers.push(spawn_server(address));
        if let Some(server) = servers.last() {
            server.send(Msg::Start).unwrap();
            #[cfg(feature = "traceable")]
            debug!("    - The server WAS spawned!");
        } else {
            #[cfg(feature = "traceable")]
            debug!("    - The server was NOT spawned!");
        };
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
        #[cfg(feature = "traceable")]
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
Content-Length: 14

Hello, World!
";

// Consider replacing with byte literals, e.g.  b'\r', etc.
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

#[cfg_attr(feature = "traceable", tracing::instrument)]
fn spawn_server(address: std::string::String) -> std::sync::mpsc::Sender<Msg> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(msg) = &rx.recv() {
            match msg {
                Msg::Start => {
                    #[cfg(feature = "traceable")]
                    debug!("    - The server should start.");
                    init_mio_server(address.clone());
                    #[cfg(feature = "traceable")]
                    debug!("      - The server has started.");
                }
                Msg::Stop => {
                    #[cfg(feature = "traceable")]
                    debug!("    - The server should stop.");
                    return;
                }
            }
        }
        #[cfg(feature = "traceable")]
        debug!("The server has stopped!");
    });
    tx
}

// We need a server that eliminates Hyper server code as an explanation.
// This is a lean TCP server for responding with Hello World! to a request.
// https://github.com/sergey-melnychuk/mio-tcp-server
#[cfg_attr(feature = "traceable", tracing::instrument)]
fn init_mio_server(address: std::string::String) {
    #[cfg(feature = "traceable")]
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
#[cfg_attr(feature = "traceable", tracing::instrument)]
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
