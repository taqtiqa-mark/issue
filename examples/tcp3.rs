// Limit simultaneous connections, backpressure tokens, spawn
// https://book.async.rs/patterns/accept-loop.html#async-listen-crate
//
// $ wrk -d 30s -t 4 -c 128 http://127.0.0.1:7878/
// Running 30s test @ http://127.0.0.1:7878/
//   4 threads and 128 connections
//   Thread Stats   Avg      Stdev     Max   +/- Stdev
//     Latency     6.91ms    7.14ms  67.68ms   85.71%
//     Req/Sec     2.49k   807.17     5.62k    67.05%
//   296466 requests in 30.08s, 24.88MB read
//   Socket errors: connect 0, read 34818, write 261634, timeout 0
// Requests/sec:   9854.39
// Transfer/sec:    846.86KB

use async_listen::{ListenExt};
use async_std::{prelude::*, task};

static RESPONSE: &str = "HTTP/1.1 200 OK
Content-Type: text/html
Connection: keep-alive
Content-Length: 6

hello
";

// #[async_std::main]
fn main() {
    async_std::task::block_on(async {
        // Listen for incoming TCP connections on localhost port 7878
        accept_loop("127.0.0.1:7878")
            .await
            .expect("Accept connections")
    })
}

async fn accept_loop(addr: impl async_std::net::ToSocketAddrs) -> async_std::io::Result<()> {
    let listener = async_std::net::TcpListener::bind(addr).await?;
    let mut incoming = listener
        .incoming()
        .log_warnings(|e| eprintln!("Listening error: {}", e))
        .handle_errors(std::time::Duration::from_millis(500)) // 1
        .backpressure(1000);
    while let Some((token, socket)) = incoming.next().await {
        // 2
        task::spawn(async move {
            //  handle_connection(&token, stream).await; // 3
            process(&token, socket).await.unwrap();
            drop(token);
        });
    }
    Ok(())
}

async fn process(
    _token: &async_listen::backpressure::Token,
    mut stream: async_std::net::TcpStream,
) -> async_std::io::Result<()> {
    // debug!("Accepted from: {}", stream.peer_addr()?);
    //let mut writer = stream;
    stream.write_all(RESPONSE.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    Ok(())
}
