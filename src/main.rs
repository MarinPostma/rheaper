use hipptrack::parse_profile;

fn main() {
    let profile_path = std::env::args().nth(1).unwrap();
    let out_path = std::env::args().nth(2).unwrap();

    parse_profile(profile_path, out_path);
}
