fn main() {
    let min_evo = "3.52.0";

    pkg_config::Config::new()
        .atleast_version(min_evo)
        .probe("evolution-shell-3.0")
        .unwrap_or_else(|e| panic!("evolution-shell-3.0 >= {min_evo} not found: {e}"));

    pkg_config::Config::new()
        .atleast_version(min_evo)
        .probe("evolution-mail-3.0")
        .unwrap_or_else(|e| panic!("evolution-mail-3.0 >= {min_evo} not found: {e}"));

    pkg_config::Config::new()
        .atleast_version(min_evo)
        .probe("evolution-calendar-3.0")
        .unwrap_or_else(|e| panic!("evolution-calendar-3.0 >= {min_evo} not found: {e}"));

    pkg_config::Config::new()
        .probe("libecal-2.0")
        .unwrap_or_else(|e| panic!("libecal-2.0 not found: {e}"));
}
