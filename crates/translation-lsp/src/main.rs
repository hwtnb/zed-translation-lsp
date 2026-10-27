use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(code) = translation_lsp::key_command() {
        return code;
    }
    if let Err(error) = translation_lsp::run_stdio() {
        eprintln!("translation-lsp: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
