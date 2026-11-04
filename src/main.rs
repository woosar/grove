mod data_structures;
mod helpers;

fn main() -> std::process::ExitCode {
    use clap::Parser;
    match data_structures::App::launch(data_structures::Cli::parse()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("grove: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
