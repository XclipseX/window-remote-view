//! `rv-host`: CLI / debug entry point. The release app (RemoteView.exe) runs
//! the same engine through `RemoteView.exe --host-worker`.

fn main() {
    std::process::exit(rv_host_win::engine::run_cli(std::env::args().skip(1).collect()));
}
