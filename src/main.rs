fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some(
            command @ ("register" | "task" | "state" | "run" | "status" | "cancel" | "log"
            | "export" | "server" | "mcp"),
        ) => {
            use std::os::unix::process::CommandExt;
            let script = concat!(env!("CARGO_MANIFEST_DIR"), "/headless/kernel.py");
            let python = std::env::var_os("AN_PYTHON").unwrap_or_else(|| {
                let local = concat!(env!("CARGO_MANIFEST_DIR"), "/target/mcp-env/bin/python");
                if std::path::Path::new(local).exists() {
                    local.into()
                } else if std::path::Path::new("/opt/homebrew/bin/python3").exists() {
                    "/opt/homebrew/bin/python3".into()
                } else {
                    "python3".into()
                }
            });
            let error = std::process::Command::new(python)
                .arg(script)
                .arg(command)
                .args(args)
                .env(
                    "AN_EXECUTABLE",
                    std::env::current_exe().expect("executable"),
                )
                .exec();
            eprintln!("an: {error}");
            std::process::exit(1);
        }
        _ => {
            eprintln!("usage: an mcp --task ID | server register/tools | register IMAGE | task new/resume | state/run/status/cancel/log --task ID | export ID DIR");
            std::process::exit(2);
        }
    }
}
