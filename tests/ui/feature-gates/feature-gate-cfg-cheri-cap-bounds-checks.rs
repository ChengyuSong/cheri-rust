#![crate_type = "lib"]

pub fn cheri_cap_bounds_checks_are_enabled() -> bool {
    cfg!(cheri_cap_bounds_checks) //~ ERROR `cfg(cheri_cap_bounds_checks)` is experimental
}
