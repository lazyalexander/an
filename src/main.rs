fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("serve") => {
            let root = args
                .next()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
            if let Err(error) = an::serve(&root) {
                eprintln!("an serve: {error}");
                std::process::exit(1);
            }
        }
        _ => {
            eprintln!("usage: an serve [root]");
            std::process::exit(2);
        }
    }
}
