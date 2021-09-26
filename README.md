# Issue: Reproducible Hang

This benchmark demonstrates a reproducible hang in Hyper or Tokio or Futures
Listening on address: `127.0.0.1:8888`

```rust
RUST_LOG=trace cargo bench --bench mre -Z unstable-options --profile release -- calibrate-limit --nocapture &> bench-trace.log
```
