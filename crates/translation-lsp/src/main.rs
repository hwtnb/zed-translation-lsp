fn main() {
    if let Err(error) = zed_code_translation_lsp::run_stdio() {
        eprintln!("translation preview: {error}");
        std::process::exit(1);
    }
}
