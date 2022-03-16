use futures::StreamExt;
#[cfg(feature = "traceable")]
use minitrace::prelude::*;
use std::io::{Read, Write};

#[derive(Clone, Debug)]
struct Sa<T> {
    sfa: std::vec::Vec<T>,
}

impl<T: 'static> Default for Sa<T> {
    #[minitrace::trace("Sa::default", enter_on_poll = false)]
    fn default() -> Self {
        Sa { sfa: vec![] }
    }
}

impl Sa<String> {
    #[minitrace::trace("Sa::new", enter_on_poll = false)]
    fn new() -> Self {
        Default::default()
    }

    #[minitrace::trace("Sa::sa", enter_on_poll = false)]
    fn sa(&mut self) -> std::string::String {
        let l = mio::net::TcpListener::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let a = l.local_addr().unwrap().to_string();
        let mut u: String = "".to_owned();
        u.push_str(&a);
        self.sfa.push(u.clone());
        u
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    #[cfg(feature = "traceable")]
    let (span, collector) = minitrace::Span::root("root");

    let mut va = Sa::<String>::new();
    let mut vb = vec![];

    #[cfg(feature = "traceable")]
    {
        let f = async {
            a(va, vb).await;
        };
        tokio::spawn(f.in_span(span)).await.unwrap();
    }

    #[cfg(feature = "traceable")]
    {
        // Send remaining spans
        let spans = collector.collect().await;
        // Report to Jaeger
        let bytes =
            minitrace_jaeger::encode("utrace-0.1.0".to_owned(), rand::random(), 0, 0, &spans)
                .unwrap();
        minitrace_jaeger::report("127.0.0.1:6831".parse().unwrap(), &bytes)
            .await
            .ok();
    }
    Ok(())
}

enum Ea {
    Start,
    Stop,
}

#[cfg_attr(feature = "traceable", minitrace::trace("fn-a", enter_on_poll = true))]
async fn a(va: Sa<String>, vb: Vec<std::sync::mpsc::Sender<Ea>>) {
    let mut va = va;
    let mut vb = vb;
    b(&mut va, &mut vb);
    // some work here
    let secs = std::time::Duration::from_millis(1000);
    std::thread::sleep(secs);
    let va = std::sync::Arc::new(va);
    // capacity(va.clone()).await;
    // println!("Terminating vb");
    // for s in vb.iter() {
    //     s.send(Msg::Stop).unwrap();
    // }
}

#[cfg_attr(feature = "traceable", minitrace::trace("fn-b"))]
fn b(va: &mut Sa<String>, vb: &mut Vec<std::sync::mpsc::Sender<Ea>>) {
    //println!("Initializing servers");
    for _ in 0..5 {
        let va = va.sa();
        // println!("  - Added address: {}", va);
        // vb.push(c(va));
        // if let Some(vb) = vb.last() {
        //     vb.send(Msg::Start).unwrap();
        //     // #[cfg(feature = "traceable")]
        //     // debug!("    - The fn-b WAS spawned!");
        // } else {
        //     // #[cfg(feature = "traceable")]
        //     // debug!("    - The fn-b was NOT spawned!");
        // };
    }
}

#[cfg_attr(feature = "traceable", minitrace::trace("fn-c"))]
fn c(va: std::string::String) -> std::sync::mpsc::Sender<Ea> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(msg) = &rx.recv() {
            match msg {
                Ea::Start => {
                    // d(va.clone());
                }
                Ea::Stop => {
                    return;
                }
            }
        }
    });
    tx
}

#[cfg_attr(feature = "traceable", minitrace::trace("fn-d"))]
fn d(address: std::string::String) {
    let mut listener = e(&address.parse().unwrap()).expect("Could not bind to address");
}

#[cfg_attr(feature = "traceable", minitrace::trace("fn-e"))]
fn e(va: &std::net::SocketAddr) -> Result<mio::net::TcpListener, std::convert::Infallible> {
    let builder = match *va {
        std::net::SocketAddr::V4(_) => mio::net::TcpSocket::new_v4().expect("TCP v4"),
        std::net::SocketAddr::V6(_) => mio::net::TcpSocket::new_v6().expect("TCP v6"),
    };
    builder.set_reuseport(true).expect("Reusable port");
    builder.set_reuseaddr(true).expect("Reusable address");
    builder.bind(*va).expect("TCP socket");
    Ok(builder.listen(1024).expect("TCP listener"))
}
