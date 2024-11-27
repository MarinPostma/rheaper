use std::path::PathBuf;

use clap::Parser as _;
use rheaper::parse_profile;

#[derive(Debug, clap::Parser)]
/// Transforms profile data collected by the rheaper lib into a SQLite3 database
struct Command {
    /// Path pointing the the `rip-*` profile data
    profile_data: PathBuf,
    /// Path where the analyzed SQLite3 database containing the analyzed data will be written
    analyzed_db: PathBuf,
}

fn main() {
    let cmd = Command::parse();

    parse_profile(cmd.profile_data, cmd.analyzed_db);
}
