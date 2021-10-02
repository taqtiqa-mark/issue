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

## TCP v1: MIO (40k)

```bash
$ cargo run --example tcp --profile release -Z unstable-options
$ wrk -d 30s -t 4 -c 128 http://127.0.0.1:9000/
Running 30s test @ http://127.0.0.1:9000/
  4 threads and 128 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency     3.54ms    3.83ms  52.38ms   88.34%
    Req/Sec    10.66k     4.53k   20.50k    59.73%
  1276169 requests in 30.06s, 107.10MB read
Requests/sec:  42447.04
Transfer/sec:      3.56MB
```

## TCP v2: Async-std (10k r/s)

```bash
$ wrk -d 30s -t 4 -c 128 http://127.0.0.1:8080/
Running 30s test @ http://127.0.0.1:8080/
  4 threads and 128 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency    12.22ms   10.40ms  89.82ms   74.17%
    Req/Sec     2.07k   662.36     4.04k    67.98%
  247422 requests in 30.08s, 20.76MB read
  Socket errors: connect 0, read 42866, write 204556, timeout 0
Requests/sec:   8224.50
Transfer/sec:    706.79KB
```
