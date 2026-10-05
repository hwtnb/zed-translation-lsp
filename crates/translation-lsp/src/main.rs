fn main() {
    if let Err(error) = translation_lsp::run_stdio() {
        eprintln!("translation preview: {error}");
        std::process::exit(1);
    }
}
