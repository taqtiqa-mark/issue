# Issue: Reproducible Hang

This benchmark demonstrates a reproducible hang in Hyper or Tokio or Futures

```rust
cargo bench --bench mre -Z unstable-options --profile release -- calibrate-limit --nocapture &> bench-trace.log
```
