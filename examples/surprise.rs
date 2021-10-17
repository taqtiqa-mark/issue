
use futures::StreamExt;

async fn is() {
    let concurrent_limit = 3;
    let his = vec!["Hi"; 2];

    futures::stream::iter(his)
        .map(|h| {
            let h = &h;
            async move {
                println!("{}",h);
            }
        })
        .buffer_unordered(concurrent_limit);
}

#[tokio::main]
fn main() {
    not_parallel();
    is_parallel();
}
