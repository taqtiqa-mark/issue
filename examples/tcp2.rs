//! TCP echo server, accepting connections both on both ipv4 and ipv6 sockets.
//! https://github.com/async-rs/async-std/blob/master/examples/tcp-echo.rs
//!
//! To send messages, do:
//!
//! ```sh
//! $ nc 127.0.0.1 8080
//! $ nc ::1 8080
//! ```
// $ wrk -d 30s -t 4 -c 128 http://127.0.0.1:8080/
// Running 30s test @ http://127.0.0.1:8080/
//   4 threads and 128 connections
//   Thread Stats   Avg      Stdev     Max   +/- Stdev
//     Latency    12.22ms   10.40ms  89.82ms   74.17%
//     Req/Sec     2.07k   662.36     4.04k    67.98%
//   247422 requests in 30.08s, 20.76MB read
//   Socket errors: connect 0, read 42866, write 204556, timeout 0
// Requests/sec:   8224.50
// Transfer/sec:    706.79KB

use async_std::io;
use async_std::net::{TcpListener, TcpStream};
use async_std::prelude::*;
use async_std::task;

static RESPONSE: &str = "HTTP/1.1 200 OK
Content-Type: text/html
Connection: keep-alive
Content-Length: 6

hello
";

async fn process(stream: TcpStream) -> io::Result<()> {
    // debug!("Accepted from: {}", stream.peer_addr()?);
    let mut writer = stream;
    writer.write(RESPONSE.as_bytes()).await.unwrap();
    Ok(())
}

fn main() -> io::Result<()> {
    task::block_on(async {
        let ipv4_listener = TcpListener::bind("127.0.0.1:8080").await?;
        println!("Listening on {}", ipv4_listener.local_addr()?);
        let mut ipv4_incoming = ipv4_listener.incoming();
        while let Some(stream) = ipv4_incoming.next().await {
            let stream = stream?;
            task::spawn(async {
                process(stream).await.unwrap();
            });
        }
        Ok(())
    })
}