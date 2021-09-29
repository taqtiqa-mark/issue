# Issue: Reproducible Hang

This benchmark demonstrates a reproducible hang in Hyper or Tokio or Futures
Listening on address: `127.0.0.1:8888`

```rust
RUST_LOG=trace cargo bench --bench mre -Z unstable-options --profile release -- calibrate-limit --nocapture &> bench-trace.log
```

Flamegraph

```bash
echo -1 | sudo tee /proc/sys/kernel/perf_event_paranoid
cargo bench --bench mre -- calibrate-limit --nocapture --profile-time 130
go tool pprof -http=:8080 ./target/criterion/Calibrate/calibrate-limit/100000/profile/profile.pb

go tool pprof -svg profile300.gz ./../target/criterion/Calibrate/calibrate-limit/100000/profile/profile.pb
```
