use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some(code) = translation_lsp::anthropic_key_command() {
        return code;
    }
    if let Err(error) = translation_lsp::run_stdio() {
        eprintln!("translation preview: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
