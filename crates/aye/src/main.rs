//! The `aye` binary: [`aye::main`], over the process's arguments.

fn main() -> std::process::ExitCode {
    aye::main(std::env::args_os())
}
