# Issue: Reproducible Hyper Client Hang

This benchmark demonstrates a reproducible hang in Hyper client.
To see if the default settings reproduce a hang on your system:

## Release Test

```bash
cargo run --example mre -Z unstable-options --profile release -- --nocapture &> mre-notrace.log
```

## Trace Log

Set the tracing subscriber level to `TRACE`

```rust
fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
```

Then run the command:

```bash
RUST_LOG=trace HYPER=trace cargo run --example mre -Z unstable-options --profile dev -- --nocapture &> mre-trace.log
```

## Strace Log

To use the [`strace-parser`](https://gitlab.com/gitlab-com/support/toolbox/strace-parser)
(expect in the order of 18M lines, 60MB, of strace output for a release build):

```bash
strace -fttTyyy -s 1024 target/release/examples/mre 2>&1 |xz --threads=0 --compress --extreme /tmp/mre-release-strace.log.xz
```

You will need `ctl-c` once the output stream halts, producing a compressed
file `xz` cannot decompress.
To extract the decompressed data:

```bash
xzcat --ignore-check /tmp/mre-release-strace.log.xz >/tmp/mre-release-strace.log
```

For `dev` builds with/without `HYPER=trace` and `RUST_LOG=trace` sizes and runtimes
increase substantially.

## Overview

There are two components to this MRE: server and client.

The server is a  a simple TCP server (mio crate) that always responds with
the HTTP 200 hello world string (stored in memory).

Several servers are started to help generate the required request load.
This should allow the hang behavior to be replicated with needing to adjust
system level configuration, e.g. the time a port must be idle before being
reused, etc.

Several clients are started to generate the required request load.
Parallelism is achieved by running client via `tokio::task::spawn`.
Concurrency is controlled by using `buffer_unordered` on a collection of
`FuturesUnordered`.

## Settings

If the default settings do not trigger a hang immediately, you may have to
increase some of the configurable values.

Configurable values are set as `Client` defaults, located at the top
of `[examples/mre.rs](examples/mre.rs)`:

Example:

| Setting       | MRE | MRE-OK |
|---------------+-----+--------|
| `nrequests`   | 1M  | 1M     |
| `nclients`    | 40  | 10     |
| `nservers`    | 40  | 10     |
| `concurrency` | 128 | 128    |

## Baseline: Correct behavior

The example `mre-ok.rs` illustrates expected behavior, and can be useful in
establishing a baseline, e.g. relative numbers of function calls, etc.

```bash
$ cargo run --example mre-ok -Z unstable-options --profile release -- --nocapture &> mre-ok-notrace.log
   Compiling mre v0.2.0 (/home/hedge/src/issue)
    Finished release [optimized] target(s) in 1m 23s
     Running `target/release/examples/mre-ok --nocapture`
Initializing servers
  - Added address: 127.0.0.1:24777
  - Added address: 127.0.0.1:15393
  - Added address: 127.0.0.1:25073
  - Added address: 127.0.0.1:36457
  - Added address: 127.0.0.1:17921
  - Added address: 127.0.0.1:32145
  - Added address: 127.0.0.1:28547
  - Added address: 127.0.0.1:19611
  - Added address: 127.0.0.1:20227
  - Added address: 127.0.0.1:23647
Awaiting clients
Run Stream. Thread: ThreadId(88)
Run Stream. Thread: ThreadId(26)
Run Stream. Thread: ThreadId(89)
Run Stream. Thread: ThreadId(55)
Run Stream. Thread: ThreadId(90)
Run Stream. Thread: ThreadId(56)
Run Stream. Thread: ThreadId(91)
Run Stream. Thread: ThreadId(57)
Run Stream. Thread: ThreadId(92)
Run Stream. Thread: ThreadId(15)
Throughput: 36489.9 request/s [1000000 in 54809630]
Terminating servers
```

## Observations

### Too many files open (transitory?)

There is one idiosyncratic behavior I have observed that might be a clue to
what is going wrong - but could be a false lead:

For some MRE parameters that generate a hang less reliably, e.g. one out of
three runs, I did observe a 'too many files open' error logged to std out
(anywhere from 100 to 400 such messages) - then they stopped and the hang
occurred. Inspection via `lsof` and `ls /proc/.../fd` did not show too many
files open once messages stopped.  This may be a transitory state?

For parameter values that reliably trigger the hang there, so far, have been
no such messages - just straight to the hang behavior after a short period
of running.
