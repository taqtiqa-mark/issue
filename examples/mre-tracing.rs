use futures::StreamExt;
use std::io::{Read, Write};
use tracing::instrument;
use tracing::{self, debug, error, info, info_span, span, trace, trace_span, warn,};
//use tracing_attributes::instrument;
use tracing_subscriber::prelude::*;

//use opentelemetry::api::Provider;
use opentelemetry::global;
use opentelemetry::trace::Tracer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Build on mre.rs by adding debug tooling.

/// Table of Contents
///
///     LoC Description
///   16-38 Parameters
///   38-92 Setup Concurrent and Parallel HTTP GET
///  93-153 Start Server and Client
/// 154-185 Utility Code
/// 186-351 Server Code
///

impl<T: 'static> Default for Client<T> {
    fn default() -> Self {
        let nrequests = 100;
        let cpus = num_cpus::get(); // 2 on initial dev system
        let nclients = cpus; // Clients to start (parallelism)
        let concurrency = 10; // Concurrent requests
        let nservers = cpus; // Servers to start
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

#[tracing::instrument]
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

#[tracing::instrument]
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

#[tracing::instrument]
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
#[tracing::instrument]
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

use tracing_subscriber::fmt;
#[tokio::main]
async fn main() {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 6831));
    // Build a Jaeger batch span processor
    let jaeger_processor = opentelemetry::sdk::trace::BatchSpanProcessor::builder(
        opentelemetry_jaeger::new_pipeline()
            .with_service_name("mre-jaeger")
            .with_agent_endpoint(addr)
            .with_tags(vec![
                opentelemetry::KeyValue::new("exporter", "jaeger"),
                opentelemetry::KeyValue::new("service.name", "my-service2"),
                opentelemetry::KeyValue::new("service.namespace", "my-namespace2")
                ])
            .with_trace_config(opentelemetry::sdk::trace::config()
                .with_resource(opentelemetry::sdk::Resource::new(vec![
                    opentelemetry::KeyValue::new("service.name", "my-service"),
                    opentelemetry::KeyValue::new("service.namespace", "my-namespace"),
            ])))
            .init_async_exporter(opentelemetry::runtime::Tokio).expect("Jaeger Tokio async exporter"),
        opentelemetry::runtime::Tokio,
    )
    .build();

    // Suspended pending Zipkin community feedback.
    //
    // // build a Zipkin exporter ()
    // let zipkin_exporter = opentelemetry_zipkin::new_pipeline()
    //     .with_service_name("mre")
    //     //.with_service_address("127.0.0.1:8080".parse()?)
    //     .with_collector_endpoint("http://localhost:9411/api/v2/spans")
    //     //.install_batch(opentelemetry::runtime::Tokio)?
    //     .init_exporter().expect("Zipkin exporter");

    // Setup Tracer Provider
    let provider = opentelemetry::sdk::trace::TracerProvider::builder()
        // We can build a span processor and pass it into provider.
        .with_span_processor(jaeger_processor)
        // For batch span processor, we can also provide the exporter and runtime and use this
        // helper function to build a batch span processor
        //  // .with_batch_exporter(zipkin_exporter, opentelemetry::runtime::Tokio)
        // Same helper function is also available to build a stdout span processor.
        // this can be useful when debugging flows between OpenTelemetry
        // and end points like Zipkin or Jaegar
        // .with_simple_exporter(opentelemetry::sdk::export::trace::stdout::Exporter::new(
        //     std::io::stdout(),
        //     true,
        // ))
        .build();

    // Get new Tracer from TracerProvider
    let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "my_app", None);
    // Create a layer with the configured tracer
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    tracing_subscriber::registry()
                        .with(telemetry)
                        .try_init()
                        .expect("Global default subscriber.");
    // let _ = opentelemetry::global::set_tracer_provider(provider);

    // let collector = tracing_subscriber::registry()
    //     .with(tracing_subscriber::EnvFilter::from_default_env().add_directive(tracing::Level::TRACE.into()))
    //     .with(tracing_subscriber::fmt::Subscriber::new().with_writer(std::io::stdout));
    // //     .with(tracing_subscriber::fmt::Subscriber::new().with_writer(subscriber));
    //tracing::collect::set_global_default(collector).expect("Unable to set a global collector");

    // It maybe this is not required
    //tracing::subscriber::set_global_default(subscriber).expect("Unable to set a global subscriber");

    // tracing_subscriber::fmt()
    //     .with_max_level(tracing::Level::TRACE)
    //     .try_init()
    //     .expect("Tracing subscriber in mre-jaeger");


    debug!("Running on thread {:?}", std::thread::current().id());
    let mut client = Client::<String>::new();
    let mut servers = vec![];
    println!("Initializing servers");
    for i in 0..client.nservers {
        let addr_span = span!(tracing::Level::INFO, "server_address", idx = i).entered();
        let address = client.add_address();
        println!("  - Added address: {}", address);
        info!("  - Added address info: {}", address);
        warn!("  - Added address warn: {}", address);
        debug!("  - Added address debug: {}", address);
        trace!("  - Added address trace: {}", address);

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
    // Trace executed code
    //tracing::subscriber::with_default(subscriber, || async {
        // Spans will be sent to the configured OpenTelemetry exporter
        let root = span!(tracing::Level::TRACE, "app_start", work_units = 2);
        let _enter = root.enter();
        trace!("some event data here.");
        let eg = trace_span!("some span data here.").entered();
        let client = std::sync::Arc::new(client);
        // let tokio_executor = tokio::runtime::Builder::new_multi_thread()
        //     .enable_all()
        //     .worker_threads(120)
        //     .thread_name("mre-client-hangs")
        //     .thread_stack_size(4 * 1024 * 1024)
        //     .build()
        //     .unwrap();
        // tokio_executor.block_on(async {
            //init_tracer().expect("Tracer setup failed");
            // let tracer = opentelemetry::global::tracer("jaeger-and-zipkin");

            // let span = tracer.start("first span");
            // let _guard = opentelemetry::trace::mark_span_as_active(span);
            capacity(client.clone()).await;

        // });

        error!("This event will be logged in the root span.");
    //}).await;

    info!("Terminating servers");
    for s in servers.iter() {
        s.send(Msg::Stop).unwrap();
    }
    // Send remaining spans
    opentelemetry::global::shutdown_tracer_provider();
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

fn init_tracer() -> Result<(), opentelemetry::trace::TraceError> {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 6831));
    // build a Jaeger batch span processor
    let jaeger_processor = opentelemetry::sdk::trace::BatchSpanProcessor::builder(
        opentelemetry_jaeger::new_pipeline()
            .with_service_name("mre")
            .with_agent_endpoint(addr)
            .with_tags(vec![opentelemetry::KeyValue::new("exporter", "jaeger")])
            .init_async_exporter(opentelemetry::runtime::Tokio)?,
        opentelemetry::runtime::Tokio,
    )
    .build();

    // build a Zipkin exporter ()
    let zipkin_exporter = opentelemetry_zipkin::new_pipeline()
        .with_service_name("mre")
        //.with_service_address("127.0.0.1:8080".parse()?)
        .with_collector_endpoint("http://localhost:9411/api/v2/spans")
        //.install_batch(opentelemetry::runtime::Tokio)?
        .init_exporter()?;

    let provider = opentelemetry::sdk::trace::TracerProvider::builder()
        // We can build a span processor and pass it into provider.
        .with_span_processor(jaeger_processor)
        // For batch span processor, we can also provide the exporter and runtime and use this
        // helper function to build a batch span processor
        .with_batch_exporter(zipkin_exporter, opentelemetry::runtime::Tokio)
        // Same helper function is also available to build a stdout span processor.
        .with_simple_exporter(opentelemetry::sdk::export::trace::stdout::Exporter::new(
            std::io::stdout(),
            true,
        ))
        .build();
    // Get new Tracer from TracerProvider
    let tracer = opentelemetry::trace::TracerProvider::tracer(&provider, "my_app", None);
    // Create a layer with the configured tracer
    let telemetry = tracing_opentelemetry::layer().with_tracer(tracer);

    let subscriber = tracing_subscriber::Registry::default().with(telemetry);
    let _ = opentelemetry::global::set_tracer_provider(provider);

    Ok(())
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
