fn main() {
    if let Err(error) = orc_desktop_lib::run() {
        eprintln!("Orc desktop startup failed: {error:#}");
        std::process::exit(1);
    }
}
