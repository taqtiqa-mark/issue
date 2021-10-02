// Limit simultaneous connections, backpressure tokens, spawn
// https://book.async.rs/patterns/accept-loop.html#async-listen-crate
//
//  $ wrk -d 30s -t 4 -c 128 http://127.0.0.1:7879/
// Running 30s test @ http://127.0.0.1:7879/
//   4 threads and 128 connections
//   Thread Stats   Avg      Stdev     Max   +/- Stdev
//     Latency     5.81ms    5.89ms  67.65ms   84.71%
//     Req/Sec     2.96k   783.08     5.67k    63.28%
//   352359 requests in 30.09s, 29.57MB read
//   Socket errors: connect 0, read 41043, write 311315, timeout 0
// Requests/sec:  11711.94
// Transfer/sec:      0.98MB

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
        accept_loop("127.0.0.1:7879")
            .await
            .expect("Accept connections")
    })
}

async fn accept_loop(addr: impl async_std::net::ToSocketAddrs) -> async_std::io::Result<()> {
    let listener = async_std::net::TcpListener::bind(addr).await?;
    let mut incoming = listener
        .incoming();
        // .log_warnings(|e| eprintln!("Listening error: {}", e))
        // .handle_errors(std::time::Duration::from_millis(100)) // 1
        // .backpressure(1000);
    while let socket = incoming.next().await.unwrap() {
        // 2
        task::spawn(async move {
            //  handle_connection(&token, stream).await; // 3
            process(socket.expect("TCP stream")).await.unwrap();
        });
    }
    Ok(())
}

async fn process(
    mut stream: async_std::net::TcpStream,
) -> async_std::io::Result<()> {
    // debug!("Accepted from: {}", stream.peer_addr()?);
    //let mut writer = stream;
    stream.write_all(RESPONSE.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    Ok(())
}
