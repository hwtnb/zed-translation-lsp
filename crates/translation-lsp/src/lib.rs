mod config;
mod language;
mod lsp;
mod providers;
mod text;

pub use lsp::server::run_stdio;
pub use providers::api_key::command::from_arguments as key_command;
