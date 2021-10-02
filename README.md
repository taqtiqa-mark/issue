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

## TCP v3: StdLib sync, backpressure (9k)

```bash
$ wrk -d 30s -t 4 -c 128 http://127.0.0.1:7878/
Running 30s test @ http://127.0.0.1:7878/
  4 threads and 128 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency     6.91ms    7.14ms  67.68ms   85.71%
    Req/Sec     2.49k   807.17     5.62k    67.05%
  296466 requests in 30.08s, 24.88MB read
  Socket errors: connect 0, read 34818, write 261634, timeout 0
Requests/sec:   9854.39
Transfer/sec:    846.86KB```
```

## TCP v4: StdLib sync, no-backpressure (11k)

```bash
$ wrk -d 30s -t 4 -c 128 http://127.0.0.1:7878/
Running 30s test @ http://127.0.0.1:7879/
  4 threads and 128 connections
  Thread Stats   Avg      Stdev     Max   +/- Stdev
    Latency     5.81ms    5.89ms  67.65ms   84.71%
    Req/Sec     2.96k   783.08     5.67k    63.28%
  352359 requests in 30.09s, 29.57MB read
  Socket errors: connect 0, read 41043, write 311315, timeout 0
Requests/sec:  11711.94
Transfer/sec:      0.98MB
```
