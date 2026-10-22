//! The gateway as a standalone binary; equivalent to `openmhs serve`.

fn main() -> std::process::ExitCode {
    openmhs_cli::openmhsd_main()
}
