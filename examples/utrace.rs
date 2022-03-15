use futures::StreamExt;
#[cfg(feature = "traceable")]
use minitrace::prelude::*;
use std::io::{Read, Write};

#[derive(Clone, Debug)]
struct Sa<T> {
    sfa: std::vec::Vec<T>,
}

impl<T: 'static> Default for Sa<T> {
    fn default() -> Self {
        Sa {
            sfa: vec![],
        }
    }
}

impl Sa<String> {
    fn new() -> Self {
        Default::default()
    }

    fn sa(&mut self) -> std::string::String {
        self.sfa.push(".".to_string());
        ".".to_string()
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync + 'static>> {
    #[cfg(feature = "traceable")]
    let (span, collector) = minitrace::Span::root("root");

    let mut va = Sa::<String>::new();
    let mut vb: std::vec::Vec<String> = vec![];

    // #[cfg(feature = "traceable")]
    // {
    //     let f = async {
    //         a(va, vb).await;
    //     };
    //     tokio::spawn(f.in_span(span)).await.unwrap();
    // }
    Ok(())
}